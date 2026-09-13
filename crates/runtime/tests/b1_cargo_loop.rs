//! B1 端到端验收：JS 侧全闭环（低买 → 卸车 → 补电 → 高卖交付）与
//! 已受理 take 在宿主终止后照常结算（docs/architecture/03 故障分级）。

mod common;

use common::{fixture, host_bin};
use std::time::Instant;
use ztw_api::harness::{FaultClass, OutcomeKind, Session, SessionConfig};
use ztw_model::{OrderSide, Position, VehicleKind};
use ztw_sim::World;

fn loop_world() -> World {
    let mut w = World::new_empty(12, 8, 200_000);
    w.add_robot(Position::new(1, 1));
    w.add_charger(Position::new(1, 5)); // 与 (1,4) 相邻：等车空档可补电
    w.add_dock(Position::new(0, 3), (0, 1)); // 第二格 (0,4)：机器人 (1,4) 可交互
    w.add_listing(OrderSide::Sell, "battery", 1, 4_000);
    w.add_listing(OrderSide::Buy, "battery", 1, 6_000);
    w
}

#[test]
fn full_cargo_loop_buys_low_sells_high() {
    let mut s = Session::new(SessionConfig::new(host_bin()), loop_world());
    assert!(s.load_code(&fixture("cargo_loop.js")).ok);

    let t0 = Instant::now();
    for _ in 0..12 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok, "闭环不应有脚本错误");
    }
    assert!(t0.elapsed().as_secs() < 10, "12 tick 应在秒级完成");

    // 闭环终态：两单完成、车辆清空、机器人空载、毛利 2 金币入账。
    assert_eq!(s.world.gold_milli, 200_000 - 4_000 + 6_000);
    assert!(s.world.my_orders.is_empty(), "两单均已了结");
    assert!(s.world.vehicles.is_empty(), "车辆均已离场");
    assert!(s.world.listings.is_empty(), "固定挂单已全部吃掉");
    assert_eq!(s.world.robots[&1].carry, None);
    assert_eq!(s.world.robots[&1].pos, Position::new(1, 4));
    // take(6) + give(5) 耗电、charge(+25) 封顶：100 - 6 + 25 -> 100 - 5 = 95。
    assert_eq!(s.world.robots[&1].energy, 95);
    // 全链路日志齐备。
    let logs: Vec<&String> = s.logs.iter().map(|(_, l)| l).collect();
    for prefix in ["take-sell OK", "take-box OK", "take-buy OK", "give-box OK"] {
        assert!(
            logs.iter().any(|l| l.contains(prefix)),
            "缺日志 {prefix}：{logs:?}"
        );
    }
}

#[test]
fn accepted_take_survives_host_termination() {
    // 已受理 take 在宿主终止后照常结算：卸空车辆同结算内离场。
    // 世界预先推进到车辆到场（等价于上一阶段已完成接单）。
    let mut w = loop_world();
    let sell = w.listings.keys().next().copied().unwrap();
    assert_eq!(w.listings[&sell].side, OrderSide::Sell); // 不依赖 add_listing 的 id 序
    let (code, _) = w.manage_take(sell);
    assert_eq!(code, ztw_model::codes::OK);
    w.end_tick();
    w.boundary_events();
    let vid = w.docks.values().next().unwrap().docked_vehicle.unwrap();
    let bx = w.vehicles[&vid].box_ids[0];
    assert_eq!(w.vehicles[&vid].kind, VehicleKind::In);

    let cfg = SessionConfig::new(host_bin()).with_fault("abort_after_reply:robot.take");
    let mut s = Session::new(cfg, w);
    // 机器人 (1,1) 与车辆交互格 (0,4) 不相邻——先走三步到位再触发故障注入。
    let walk = r#"
function loop() {
  const r = Game.robots()[0];
  if (r.pos.y < 4) { r.move(Game.SOUTH); return; }
  const v = Game.vehicles("in")[0];
  if (v) Game.log("take", r.take(v, v.boxes[0].id));
}
"#;
    assert!(s.load_code(walk).ok);
    for _ in 0..3 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok);
    }
    let out = s.tick(); // 本 tick 触发 abort_after_reply:robot.take
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("crash"))
    );
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.last_op, "robot.take");
    // 已受理 take 照常结算：机器人拿到箱、车辆卸空离场、订单完成。
    assert_eq!(s.world.robots[&1].carry, Some(bx));
    assert!(!s.world.vehicles.contains_key(&vid));
    assert!(s.world.my_orders.is_empty());
    // 重启宿主后结算结果可见。
    assert!(s.restart_host().ok);
    assert!(
        s.load_code(
            r#"
function loop() {
  const r = Game.robots()[0];
  Game.log("res", r.last_result ? r.last_result.code + ":" + r.last_result.action : "none");
}
"#
        )
        .ok
    );
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert!(
        s.logs.back().unwrap().1.contains("OK:take"),
        "上一 tick 的 take 结算结果应可见：{}",
        s.logs.back().unwrap().1
    );
}

#[test]
fn cancel_and_destroy_visible_same_tick_via_delta() {
    // cancel / destroy 的镜像增量同 tick 可见（docs/architecture/03）：
    // my_orders、gold、shelves 在调用后的同 tick 查询立即反映。
    let mut w = World::new_empty(12, 8, 200_000);
    w.add_robot(Position::new(1, 1));
    w.add_shelf(Position::new(3, 1));
    w.add_dock(Position::new(0, 4), (0, 1));
    w.add_listing(OrderSide::Sell, "battery", 1, 5_000);
    let mut s = Session::new(SessionConfig::new(host_bin()), w);
    let code = r#"
function loop() {
  if (Game.tick === 0) {
    const o = Game.market.sell_orders()[0];
    Game.log("take", Game.market.take(o.id));
    Game.log("after-take", Game.my_orders().length, Game.gold);
    Game.log("cancel", Game.market.cancel(o.id));
    Game.log("after-cancel", Game.my_orders().length, Game.gold);
  } else if (Game.tick === 1) {
    const sh = Game.shelves()[0];
    Game.log("destroy", Game.destroy(sh));
    Game.log("after-destroy", Game.shelves().length, Game.gold);
  }
}
"#;
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    // 接单即付 5 → 195；取消退 4.5（手续费 0.5）→ 199.5。
    let logs: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logs.iter().any(|l| l == "after-take 1 195"),
        "take 增量同 tick 可见：{logs:?}"
    );
    assert!(
        logs.iter().any(|l| l == "after-cancel 0 199.5"),
        "cancel 增量同 tick 可见：{logs:?}"
    );
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let logs: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    // 销毁货架退款 87.5 → 287。
    assert!(
        logs.iter().any(|l| l == "after-destroy 0 287"),
        "destroy 增量同 tick 可见：{logs:?}"
    );
    assert_eq!(s.world.shelves.len(), 0);
    assert_eq!(s.world.gold_milli, 287_000);
}
