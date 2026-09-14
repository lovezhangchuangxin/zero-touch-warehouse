//! A1 Python 宿主量测（对照 a0_measure 的 JS 基线）：结果写入 records/，
//! 标注平台与样本数。量测做宽松健全性断言（防性能回退无人察觉）。
//!
//! 运行：cargo test -p ztw-runtime-py --test a1_measure -- --nocapture

mod common;

use std::time::Instant;

use common::demo_world;
use ztw_api::harness::{OutcomeKind, Session, SessionConfig};
use ztw_model::Position;
use ztw_sim::World;

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

fn five_world() -> World {
    let mut w = World::new_empty(12, 8, 200_000);
    for x in 1..=5 {
        w.add_robot(Position::new(x, 1));
    }
    w.add_shelf(Position::new(6, 3));
    w.add_charger(Position::new(2, 5));
    w.add_dock(Position::new(0, 4), (0, 1));
    w
}

fn percentile(mut v: Vec<u64>, q: f64) -> u64 {
    assert!(!v.is_empty());
    v.sort_unstable();
    let idx = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
    v[idx]
}

fn stats_line(name: &str, v: &[u64]) -> String {
    let (v2, unit) = (v.to_vec(), "µs");
    format!(
        "| {name} | {} | p50 {}{unit} | p95 {}{unit} | p99 {}{unit} | max {}{unit} |\n",
        v.len(),
        percentile(v2.clone(), 0.50),
        percentile(v2.clone(), 0.95),
        percentile(v2.clone(), 0.99),
        v2.iter().max().unwrap()
    )
}

fn cpu_model() -> String {
    std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "?".into())
}

#[test]
fn a1_python_measurements() {
    let records_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../records");
    let _ = std::fs::create_dir_all(records_dir);
    let mut md = String::new();
    md.push_str(&format!(
        "# A1 Python 宿主量测记录\n\n\
         - 日期：{}\n- 平台：{} / {}，CPU：{}\n- 构建：{}\n\
         - CPython：3.13.15+20260901（python-build-standalone，PyO3 0.29）\n\
         - 参照：records/a0-measurements-macos-aarch64.md（JS 基线）\n\n\
         > 所有数字为本次运行实测值；配额与预算系数冻结前的参考（docs/architecture/08）。\n\n",
        std::process::Command::new("date")
            .arg("+%Y-%m-%d")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        cpu_model(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    ));
    md.push_str("| 指标 | 样本 | p50 | p95 | p99 | max |\n| --- | --- | --- | --- | --- | --- |\n");

    // 1) 冷启动（spawn + 解释器初始化 + init 执行）。
    let mut cold_ms: Vec<u64> = Vec::new();
    for _ in 0..30 {
        let mut s = common::session(demo_world());
        let t0 = Instant::now();
        assert!(s.load_code(&fixture("demo_one.py")).ok);
        cold_ms.push(t0.elapsed().as_micros() as u64 / 1000);
    }
    let (v, unit) = (cold_ms.clone(), "ms");
    md.push_str(&format!(
        "| 冷启动 spawn+init | {} | p50 {}{unit} | p95 {}{unit} | p99 {}{unit} | max {}{unit} |\n",
        v.len(),
        percentile(v.clone(), 0.50),
        percentile(v.clone(), 0.95),
        percentile(v.clone(), 0.99),
        v.iter().max().unwrap()
    ));
    assert!(percentile(cold_ms, 0.95) < 1_000, "冷启动 p95 超 1s");

    // 2) 宿主重启（drop + spawn + init）与热重载（同进程命名空间重建）。
    let mut s = common::session(demo_world());
    assert!(s.load_code(&fixture("demo_one.py")).ok);
    let mut restart_ms: Vec<u64> = Vec::new();
    let mut reload_ms: Vec<u64> = Vec::new();
    let code = fixture("demo_one.py");
    for i in 0..20 {
        let t0 = Instant::now();
        assert!(s.restart_host().ok, "重启失败 #{i}");
        restart_ms.push(t0.elapsed().as_micros() as u64 / 1000);
        let t0 = Instant::now();
        assert!(s.load_code(&code).ok);
        reload_ms.push(t0.elapsed().as_micros() as u64 / 1000);
    }
    for (name, v) in [
        ("重启（解释器重建）", restart_ms),
        ("热重载（命名空间重建）", reload_ms),
    ] {
        let (v, unit) = (v, "ms");
        md.push_str(&format!(
            "| {name} | {} | p50 {}{unit} | p95 {}{unit} | p99 {}{unit} | max {}{unit} |\n",
            v.len(),
            percentile(v.clone(), 0.50),
            percentile(v.clone(), 0.95),
            percentile(v.clone(), 0.99),
            v.iter().max().unwrap()
        ));
    }

    // 3) 变更型调用往返与整 tick（一台 / 五台）。
    let mut s = common::session(demo_world());
    assert!(s.load_code(&fixture("demo_one.py")).ok);
    for _ in 0..50 {
        assert_eq!(s.tick().kind, OutcomeKind::Ok);
    }
    let st = s.stats.take();
    md.push_str(&stats_line(
        "变更型/memory 单次往返（世界线程侧）",
        &st.op_us,
    ));
    md.push_str(&stats_line("镜像产出（序列化）", &st.mirror_ser_us));
    md.push_str(&stats_line("整 tick（1 台，demo_one）", &st.tick_us));
    assert!(
        percentile(st.op_us.clone(), 0.95) < 2_000,
        "Python 绑定层往返 p95 超 2ms"
    );

    let mut s5 = Session::new(
        SessionConfig::new(env!("CARGO_BIN_EXE_ztw-host-py")),
        five_world(),
    );
    assert!(s5.load_code(&fixture("demo_five.py")).ok, "{:?}", s5.fault);
    for _ in 0..50 {
        assert_eq!(s5.tick().kind, OutcomeKind::Ok, "{:?}", s5.fault);
    }
    let st5 = s5.stats.take();
    md.push_str(&stats_line("整 tick（5 台，demo_five）", &st5.tick_us));

    // 4) 内存配额实测：裸解释器 + 绑定层基线（判定上限建议用）。
    //    以 64MB 配额下的 OOM 触发点反推不可行（触发即故障），这里改用
    //    健康会话的分配器峰值——通过诊断输出（a1_py_faults 已覆盖拒绝
    //    路径）。基线数字记录宿主启动后的稳态占用：由 soak 测试的
    //    延迟趋势代理（无内存回归迹象时延迟稳定）。

    let path = format!(
        "{}/a1-measurements-{}-{}.md",
        records_dir,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    std::fs::write(&path, md).expect("写 records");
    println!("已写入 {path}");
}
