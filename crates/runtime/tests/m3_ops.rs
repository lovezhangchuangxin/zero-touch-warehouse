//! M3 管理操作贯通 JS 宿主（docs/architecture/03 管理操作当 tick 可见）：
//! buy / borrow / repay 经绑定层往返，镜像增量回放后同 tick 查询立即可见
//! （快照纯度的唯一例外）；初始化阶段照拒 INIT_PHASE；计息随 Session 闭环。

mod common;

use common::{demo_world, fixture};
use ztw_api::harness::{OutcomeKind, Session, SessionConfig};

/// 增量回放后同 tick 可见：buy 的对象 / 资金线、borrow / repay 的欠款线
/// 不等下一次全量镜像（与 a1_matrix 的双语言对账共用 m3_ops.js 序列）。
#[test]
fn manage_ops_visible_same_tick_via_delta_replay() {
    let mut s = Session::new(SessionConfig::new(common::host_bin()), demo_world());
    assert!(
        s.load_code(&fixture("m3_ops.js")).ok,
        "初始化失败：{:?}",
        s.fault
    );
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    // 终态：原货架留存（新购的已销毁）、充电桩 +1、机器人 +1；金币
    // 112.5 gold；欠款 950 gold + 首 tick 利息 0.95。
    assert_eq!(s.world.shelves.len(), 1);
    assert_eq!(s.world.chargers.len(), 2);
    assert_eq!(s.world.robots.len(), 2);
    assert_eq!(s.world.gold_milli, 112_500);
    assert_eq!(s.world.debt_milli, 950_950);
    // 日志锚点：NOT_ON_WALL 经绑定层原样返回（E 表同步的证据点）。
    assert!(
        s.logs.iter().any(|(_, l)| l == "y4:NOT_ON_WALL"),
        "{:?}",
        s.logs
    );
}

/// 初始化阶段禁用管理操作：返回 INIT_PHASE 码（不抛错、初始化照常成功），
/// 世界资金不动。
#[test]
fn manage_ops_rejected_in_init_phase() {
    let mut s = Session::new(SessionConfig::new(common::host_bin()), demo_world());
    let init = s
        .load_code(
            r#"
const seen = [];
seen.push(Game.borrow(10));
seen.push(Game.buy("shelf", 5, 5));
Game.log("init:" + seen.join(","));
function loop() {}
"#,
        )
        .ok;
    assert!(init, "初始化应成功（拒绝是返回码，不是异常）");
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert!(
        s.logs
            .iter()
            .any(|(_, l)| l == "init:INIT_PHASE,INIT_PHASE"),
        "{:?}",
        s.logs
    );
    assert_eq!(s.world.gold_milli, 200_000, "初始化期资金不动");
    assert_eq!(s.world.debt_milli, 0);
    assert_eq!(s.world.shelves.len(), 1, "原货架仍在，未新增");
}

/// 计息经完整 Session 闭环：阶段 5 收尾按欠款 × 利率复利。
#[test]
fn interest_accrues_through_session_tick() {
    let mut s = Session::new(SessionConfig::new(common::host_bin()), demo_world());
    assert!(
        s.load_code("function loop() { if (Game.tick === 0) Game.borrow(100); }")
            .ok,
        "初始化失败：{:?}",
        s.fault
    );
    assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert_eq!(s.world.debt_milli, 100_100, "100_000 + 首笔利息 100");
    assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert_eq!(s.world.debt_milli, 100_200, "复利：100_100 + 100");
}

/// 增量回放失败不丢当 tick 可见性：borrow / buy 的资金线与新对象经
/// mirror.fetch 整体重建后立即可读（skip_delta_replay 注入；a0_boundary
/// 的 take 先例扩展到管理操作）。
#[test]
fn funds_and_buy_visible_after_replay_failure() {
    let cfg = SessionConfig::new(common::host_bin()).with_fault("skip_delta_replay");
    let mut s = Session::new(cfg, demo_world());
    assert!(
        s.load_code(
            r#"
function loop() {
  Game.borrow(50.5);
  Game.log("f:" + Math.floor(Game.gold * 1000 + 0.5) + "," + Math.floor(Game.debt * 1000 + 0.5));
  Game.buy("shelf", 5, 5);
  Game.log("s:" + Game.shelves().length + "," + Game.shelves()[1].id);
}
"#
        )
        .ok,
        "初始化失败：{:?}",
        s.fault
    );
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert!(
        s.logs.iter().any(|(_, l)| l == "f:250500,50500"),
        "{:?}",
        s.logs
    );
    assert!(
        s.logs.iter().any(|(_, l)| l.starts_with("s:2,")),
        "{:?}",
        s.logs
    );
    assert!(
        s.stats.last_exec.mirror_rebuilds >= 1,
        "应经 mirror.fetch 重建：{:?}",
        s.stats.last_exec
    );
    assert_eq!(s.world.shelves.len(), 2);
    assert_eq!(s.world.gold_milli, 75_500);
    assert_eq!(s.world.debt_milli, 50_550, "含首 tick 利息 50");
}
