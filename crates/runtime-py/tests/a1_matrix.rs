//! A1 双语言一致性矩阵（docs/architecture/08「memory 值模型：两语言同
//! 结果」行）：同一驱动跑 JS 与 Python 宿主，逐步断言 wire 结果（日志
//! 序列）与终态 MemoryTree 完全一致；外加 Python 宿主的协议注入冒烟。
//!
//! 双宿主测试要求 ztw-host-js 已构建：本包不依赖 ztw-runtime（bin 不随
//! 依赖构建），按兄弟路径解析；`cargo test --workspace`（CI / just gate）
//! 恒可用，单独跑本包前先 `cargo build -p ztw-runtime`。

mod common;

use common::demo_world;
use ztw_api::harness::{OutcomeKind, Session, SessionConfig};

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

fn session_with(bin: &std::path::Path) -> Session {
    Session::new(SessionConfig::new(bin), demo_world())
}

fn logs_of(s: &Session) -> Vec<String> {
    s.logs.iter().map(|(_, l)| l.clone()).collect()
}

// ---------------------------------------------------------------------------
// memory 值模型矩阵：两语言同结果
// ---------------------------------------------------------------------------

#[test]
fn memory_value_model_matrix_same_outcome_both_languages() {
    let mut js = session_with(&common::js_bin());
    let mut py = common::session(demo_world());
    assert!(
        js.load_code(&fixture("matrix_memory.js")).ok,
        "JS 初始化失败：{:?}",
        js.fault
    );
    assert!(
        py.load_code(&fixture("matrix_memory.py")).ok,
        "PY 初始化失败：{:?}",
        py.fault
    );
    assert_eq!(js.tick().kind, OutcomeKind::Ok, "{:?}", js.fault);
    assert_eq!(py.tick().kind, OutcomeKind::Ok, "{:?}", py.fault);

    let (js_logs, py_logs) = (logs_of(&js), logs_of(&py));
    assert_eq!(
        js_logs, py_logs,
        "两语言逐步结果（ok/err:<code>/值）必须完全一致"
    );
    // 关键语义锚点抽查（完整对账见上）。
    for anchor in [
        "err:stale:STALE_MEMORY_REFERENCE",
        "err:big_over:INVALID_VALUE",
        "err:nonfinite:INVALID_VALUE",
        "err:cycle:INVALID_VALUE",
        "ok:big_ok",
    ] {
        assert!(
            js_logs.iter().any(|l| l == anchor),
            "缺少锚点 {anchor}：{js_logs:?}"
        );
    }
    assert!(
        js_logs.iter().any(|l| l == "kkeys 10,1,02,2"),
        "字符串数字键插入序保持：{js_logs:?}"
    );

    // 终态树完全一致（MemValue 含结构与顺序）。
    let (js_snap, py_snap) = (js.memory_snapshot(), py.memory_snapshot());
    assert_eq!(js_snap, py_snap, "两语言终态 memory 树必须一致");
}

// ---------------------------------------------------------------------------
// Python 宿主协议注入冒烟：与 JS 侧 a1_protocol 同款语义
// ---------------------------------------------------------------------------

#[test]
fn py_host_duplicate_request_deduped() {
    let mut s = Session::new(
        SessionConfig::new(env!("CARGO_BIN_EXE_ztw-host-py")).with_fault("dup_request:2"),
        demo_world(),
    );
    assert!(s.load_code(&fixture("dup_take.py")).ok, "{:?}", s.fault);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "去重路径透明：{:?}", s.fault);
    assert_eq!(s.world.my_orders.len(), 1, "恰好一单");
    assert_eq!(s.world.gold_milli, 200_000 - 10_000, "扣款恰好一次");
}

#[test]
fn py_host_stale_epoch_rejected() {
    let mut s = Session::new(
        SessionConfig::new(env!("CARGO_BIN_EXE_ztw-host-py")).with_fault("stale_epoch"),
        demo_world(),
    );
    assert!(
        s.load_code("def loop():\n    Game.log('hello')\n").ok,
        "{:?}",
        s.fault
    );
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(ztw_api::harness::FaultClass::HostTerminated("protocol"))
    );
    assert_eq!(s.fault.as_ref().unwrap().code, "STALE_EPOCH");
    assert!(s.logs.is_empty(), "旧代次消息不得执行");
}
