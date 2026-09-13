//! B2 诊断采集口验收：受理失败 / 管理操作 / 订单完成三类事件的采集，
//! 以及终止按钮独立控制路径的故障分类（docs/architecture/02 §诊断事件、
//! 03「看门狗和终止按钮不排在 Game 请求队列后」）。

mod common;

use common::{fixture, host_bin};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use ztw_api::harness::{DiagTapKind, FaultClass, OutcomeKind, Session, SessionConfig};
use ztw_model::{OrderSide, Position};
use ztw_sim::World;

fn diag_world() -> World {
    let mut w = World::new_empty(12, 8, 200_000);
    w.add_robot(Position::new(1, 1));
    w.add_wall(Position::new(1, 2)); // 向南移动必被静态障碍拒绝
    w.add_port(Position::new(0, 4));
    w.add_listing(OrderSide::Sell, "battery", 1, 5_000);
    w
}

#[test]
fn accept_failures_and_init_phase_are_tapped() {
    let mut s = Session::new(SessionConfig::new(host_bin()), diag_world());
    // 初始化阶段提交动作：INIT_PHASE 拒绝应有采集记录。
    let init_code = "const r = Game.robots()[0]; r.move(Game.SOUTH); function loop() {}";
    assert!(s.load_code(init_code).ok);
    let taps = s.take_diag();
    assert_eq!(taps.len(), 1, "初始化拒绝应有一条采集：{taps:?}");
    assert_eq!(taps[0].kind, DiagTapKind::AcceptFail);
    assert_eq!(taps[0].op, "robot.move");
    assert_eq!(taps[0].code, "INIT_PHASE");
    assert_eq!(taps[0].tick, 0);

    // 执行期撞墙：CELL_BLOCKED 受理失败；成功受理的移动不产生采集。
    assert!(
        s.load_code("function loop() { Game.robots()[0].move(Game.SOUTH); }")
            .ok
    );
    assert!(s.take_diag().is_empty(), "热重载本身不产生动作采集");
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let taps = s.take_diag();
    assert_eq!(taps.len(), 1, "仅受理失败进采集：{taps:?}");
    assert_eq!(taps[0].kind, DiagTapKind::AcceptFail);
    assert_eq!(taps[0].code, "CELL_BLOCKED");
    assert!(taps[0].detail.contains("dx=0"));
    // 采集环按 tick 排空：无新事件时为空。
    assert!(s.take_diag().is_empty());
}

#[test]
fn manage_ops_tap_success_and_failure() {
    let mut s = Session::new(SessionConfig::new(host_bin()), diag_world());
    let code = r#"
let done = false;
function loop() {
  if (done) return;
  const o = Game.market.sell_orders()[0];
  Game.log("take1", Game.market.take(o.id));
  Game.log("take2", Game.market.take(o.id));
  done = true;
}
"#;
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let taps = s.take_diag();
    assert_eq!(taps.len(), 2, "成功 + 失败各一条：{taps:?}");
    assert_eq!(taps[0].kind, DiagTapKind::Manage);
    assert_eq!(taps[0].op, "market.take");
    assert_eq!(taps[0].code, "OK");
    assert!(taps[0].detail.contains("接单"), "{}", taps[0].detail);
    assert!(taps[0].detail.contains("装卸口"), "{}", taps[0].detail);
    assert_eq!(taps[1].kind, DiagTapKind::AcceptFail);
    assert_eq!(taps[1].code, "ORDER_GONE");
}

#[test]
fn order_completion_tapped_with_payment() {
    // 复用 B1 闭环世界与脚本：两单完成后各有一条 OrderDone（买单侧收款）。
    let mut w = World::new_empty(12, 8, 200_000);
    w.add_robot(Position::new(1, 1));
    w.add_charger(Position::new(1, 5));
    w.add_port(Position::new(0, 4));
    w.add_listing(OrderSide::Sell, "battery", 1, 4_000);
    w.add_listing(OrderSide::Buy, "battery", 1, 6_000);
    let mut s = Session::new(SessionConfig::new(host_bin()), w);
    assert!(s.load_code(&fixture("cargo_loop.js")).ok);
    let mut done_events = Vec::new();
    for _ in 0..12 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok);
        done_events.extend(
            s.take_diag()
                .into_iter()
                .filter(|t| t.kind == DiagTapKind::OrderDone),
        );
    }
    assert_eq!(done_events.len(), 2, "{done_events:?}");
    let pays: Vec<String> = done_events.iter().map(|t| t.detail.clone()).collect();
    // 卖单（买入）完成不收款；买单（卖出）完成收款 qty × 单价。
    assert!(pays.iter().any(|d| d.contains("收款 0 milli")), "{pays:?}");
    assert!(
        pays.iter().any(|d| d.contains("收款 6000 milli")),
        "{pays:?}"
    );
    // world_revision 随 tick 推进。
    assert_eq!(s.world_revision().tick, 12);
}

#[test]
fn host_control_kills_without_queueing_behind_game_requests() {
    // hang_exec 宿主在 loop() 前挂起；控制柄从另一线程直杀，
    // 不经世界线程命令队列（测试模拟「终止按钮」路径）。
    let cfg = SessionConfig::new(host_bin()).with_fault("hang_exec");
    let (ctl_tx, ctl_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut s = Session::new(cfg, diag_world());
        assert!(s.load_code("function loop() {}").ok);
        let ctl = s.host_control();
        ctl_tx.send(ctl).expect("发送控制柄");
        let out = s.tick(); // 阻塞在挂起的宿主上，直到被外部终止
        let fault = s.fault.clone().map(|f| (f.class, f.code));
        (out, fault)
    });
    let ctl = ctl_rx.recv().expect("控制柄就绪");
    thread::sleep(Duration::from_millis(300));
    assert!(!ctl.is_marked());
    ctl.kill();
    assert!(ctl.is_marked());
    let (out, fault) = worker.join().expect("工作线程退出");
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("killed"))
    );
    let (class, code) = fault.expect("故障已记录");
    assert_eq!(class, FaultClass::HostTerminated("killed"));
    assert_eq!(code, "KILLED_BY_MAIN");
}
