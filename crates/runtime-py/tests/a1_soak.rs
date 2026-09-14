//! A1 热重载浸泡（docs/architecture/08「热重载浸泡」行）：两语言各 1000
//! 次热重载后检查延迟分布与终态闭环——Python 覆盖命名空间重建与模块
//! 缓存复用、受控句柄释放；JS 覆盖 Runtime 重建。
//! 内存趋势的细粒度记录由 a1_measure 写入 records/（本地生成物）。

mod common;

use std::time::{Duration, Instant};

use common::demo_world;
use ztw_api::harness::{OutcomeKind, Session, SessionConfig};

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

fn percentile(mut v: Vec<u64>, q: f64) -> u64 {
    v.sort_unstable();
    let idx = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
    v[idx]
}

fn soak(bin: &std::path::Path, fixture_name: &str, reloads: usize) {
    let mut s = Session::new(SessionConfig::new(bin), demo_world());
    let code = fixture(fixture_name);
    let t_all = Instant::now();
    assert!(s.load_code(&code).ok, "首次初始化失败：{:?}", s.fault);
    let mut durations_ms: Vec<u64> = Vec::with_capacity(reloads);
    for i in 0..reloads {
        let t0 = Instant::now();
        let init = s.load_code(&code);
        assert!(init.ok, "第 {} 次热重载失败：{:?}", i + 1, init.fault);
        durations_ms.push(t0.elapsed().as_millis() as u64);
    }
    let total = t_all.elapsed();
    // 终态闭环：连续 tick 正常、宿主存活。
    for _ in 0..3 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    }
    assert!(s.host_alive(), "浸泡后宿主必须存活");

    let p50 = percentile(durations_ms.clone(), 0.50);
    let p95 = percentile(durations_ms.clone(), 0.95);
    let max = durations_ms.iter().copied().max().unwrap();
    // 宽上界（CI 慢机器余量）：单次重建 p95 < 250ms、max < 2s、全程 < 180s。
    // 本地参考：JS ~ms 级、Python ~10-50ms；超预算时先查回归再放宽并在
    // records/ 记理由（时序纪律与 a0 相同）。
    assert!(p95 < 250, "热重载 p95 {p95}ms 超宽上界");
    assert!(max < 2_000, "热重载 max {max}ms 超宽上界");
    assert!(total < Duration::from_secs(180), "全程 {total:?} 超时");
    println!(
        "soak({fixture_name}) n={reloads} p50={p50}ms p95={p95}ms max={max}ms total={total:?}"
    );
}

#[test]
fn soak_1000_hot_reloads_js() {
    soak(&common::js_bin(), "soak_player.js", 1000);
}

#[test]
fn soak_1000_hot_reloads_py() {
    soak(
        std::path::Path::new(env!("CARGO_BIN_EXE_ztw-host-py")),
        "soak_player.py",
        1000,
    );
}
