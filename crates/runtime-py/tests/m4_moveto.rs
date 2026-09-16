//! M4 寻路双语言一致性矩阵：同一驱动跑 JS 与 Python 宿主，逐步断言
//! 日志序列（E 码表、寻路三分支、move_to 码路径、缓存推进）与终态
//! memory（`_move` 缓存结构）及世界哈希完全一致。

mod common;

use common::demo_world;
use ztw_api::harness::{OutcomeKind, Session, SessionConfig};

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

/// M4 find_path / move_to（docs/game-design/08「寻路」「move_to 与
/// robot.memory」）：两语言宿主跑同一 fixture，日志序列、终态 memory
/// （含 `_move` 缓存）与世界终态哈希三方一致。
#[test]
fn m4_moveto_matrix_same_outcome_both_languages() {
    let mut js = Session::new(SessionConfig::new(common::js_bin().as_path()), demo_world());
    let mut py = common::session(demo_world());
    assert!(
        js.load_code(&fixture("m4_moveto.js")).ok,
        "JS 初始化失败：{:?}",
        js.fault
    );
    assert!(
        py.load_code(&fixture("m4_moveto.py")).ok,
        "PY 初始化失败：{:?}",
        py.fault
    );
    for t in 0..7 {
        assert_eq!(
            js.tick().kind,
            OutcomeKind::Ok,
            "JS tick {t}：{:?}",
            js.fault
        );
        assert_eq!(
            py.tick().kind,
            OutcomeKind::Ok,
            "PY tick {t}：{:?}",
            py.fault
        );
    }

    let js_logs: Vec<&str> = js.logs.iter().map(|(_, l)| l.as_str()).collect();
    let py_logs: Vec<&str> = py.logs.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(
        js_logs, py_logs,
        "两语言寻路 / 复合移动序列（码表 / 三分支 / 结果码 / 位置）必须完全一致"
    );
    // 关键语义锚点抽查（完整对账见上）。
    for anchor in [
        "p1:5,2,1,6,1",
        "p2:len0",
        "p3:null",
        "p5:len2",
        "m1:ARRIVED",
        "m2:OK",
        "m3:ALREADY_ACTED",
        "wm:RESERVED_KEY",
        "w5:ARRIVED@6,1",
        "mc:6,1|0|5",
        "np:NO_PATH",
        "md:OK",
    ] {
        assert!(js_logs.contains(&anchor), "缺少锚点 {anchor}：{js_logs:?}");
    }
    // 终态 memory 一致：`_move` 缓存结构（goal / range / 轨迹）同构。
    let (js_snap, py_snap) = (js.memory_snapshot(), py.memory_snapshot());
    assert_eq!(
        js_snap, py_snap,
        "两语言终态 memory 树（含 _move 缓存）必须一致"
    );
    // 终态世界一致（位置 / 意图为空）。
    assert_eq!(
        js.world.state_hash(),
        py.world.state_hash(),
        "两语言终态世界必须一致"
    );
}
