//! A0 量测（docs/architecture/08）：结果写入 records/，标注平台、构建
//! 版本与样本数。量测本身也做宽松的健全性断言（防性能回退无人察觉）。
//!
//! 运行：cargo test -p ztw-runtime --test a0_measure -- --nocapture

mod common;

use std::time::Instant;

use common::{demo_world, fixture, host_bin};
use ztw_api::harness::{OutcomeKind, Session, SessionConfig};
use ztw_model::Position;
use ztw_sim::World;

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

/// 更大压力场景（docs/architecture/08“及更大压力场景”）：12 台 + 6 货架。
fn twelve_world() -> World {
    let mut w = World::new_empty(16, 12, 200_000);
    for x in 1..=8 {
        w.add_robot(Position::new(x, 1));
        w.add_robot(Position::new(x, 6));
    }
    for (x, y) in [(2, 3), (4, 3), (6, 3), (3, 8), (5, 8), (7, 8)] {
        w.add_shelf(Position::new(x, y));
    }
    w.add_charger(Position::new(0, 0));
    w.add_dock(Position::new(0, 4), (0, 1));
    w.add_listing(ztw_model::OrderSide::Sell, "battery", 2, 5_000);
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

fn mean(v: &[u64]) -> f64 {
    v.iter().sum::<u64>() as f64 / v.len().max(1) as f64
}

fn rquickjs_version() -> String {
    let lock = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let text = std::fs::read_to_string(lock).unwrap_or_default();
    let mut seen = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("name = ") {
            seen = line.contains("rquickjs");
        } else if seen && line.starts_with("version = ") {
            return line
                .trim_start_matches("version = ")
                .trim_matches('"')
                .to_string();
        }
    }
    "?".into()
}

fn cpu_model() -> String {
    if cfg!(target_os = "macos") {
        std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
    } else {
        std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("?").trim().to_string())
        })
    }
    .unwrap_or_else(|| "?".into())
}

fn record_header() -> String {
    format!(
        "# A0 量测记录\n\n\
         - 日期：{}\n\
         - 平台：{} / {}，CPU：{}\n\
         - 构建：{}（ztw-runtime {}，rustc {}）\n\
         - rquickjs：{}\n\
         - 执行预算：tick 基础 {}ms（+每实体 {}µs，封顶 {}ms）、初始化 {}ms、看门狗宽限 {}ms\n\
         - 帧上限 {}B；memory 限额 nodes={} bytes={} depth={} str={}；日志 entry={}B ring={}\n\n\
         > 所有数字为本次运行的实测值；预算公式系数与配额据此冻结前的参考（docs/architecture/08）。\n",
        chrono_like_now(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        cpu_model(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        env!("CARGO_PKG_VERSION"),
        rustc_version(),
        rquickjs_version(),
        200,
        50,
        500,
        5_000,
        1_500,
        1024 * 1024,
        10_000,
        256 * 1024,
        16,
        8 * 1024,
        2 * 1024,
        256,
    )
}

fn chrono_like_now() -> String {
    // 无 chrono 依赖：用 SystemTime 的秒数近似日期。
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days = secs / 86_400;
    // 1970-01-01 起的天数 → 年月日（民用算法，够用）。
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "?".into())
}

#[test]
fn a0_measurements() {
    let mut report = record_header();
    report.push_str("\n## 镜像产出（每 tick，500 样本）\n\n| 指标 | 样本 | p50 | p95 | p99 | max |\n|---|---|---|---|---|---|\n");
    // —— 镜像产出：纯查询脚本跑 500 tick。
    {
        let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
        let code = "export function loop() { Game.robots(); Game.tick; }";
        assert!(s.load_code(code).ok);
        for _ in 0..500 {
            let out = s.tick();
            assert_eq!(out.kind, OutcomeKind::Ok);
        }
        let st = s.stats.take();
        let us = st.mirror_ser_us.clone();
        report.push_str(&stats_line("镜像序列化", &us));
        let bytes = st.mirror_bytes.clone();
        report.push_str(&format!(
            "\n| 镜像字节 | {} | 均值 {:.0}B | max {}B | | |\n",
            bytes.len(),
            mean(&bytes.iter().map(|b| *b as u64).collect::<Vec<_>>()),
            bytes.iter().max().unwrap()
        ));
    }

    // —— 管理操作增量（take）：20 个全新会话各一次。
    report.push_str(
        "\n## take 镜像增量（20 样本）\n\n| 指标 | 样本 | 均值 | max |\n|---|---|---|---|\n",
    );
    {
        let mut delta_bytes = Vec::new();
        let mut apply_stats = (0u64, 0u64, 0u64);
        for _ in 0..20 {
            let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
            assert!(s.load_code(&fixture("take_and_query.js")).ok);
            let out = s.tick();
            assert_eq!(out.kind, OutcomeKind::Ok);
            let st = s.stats.take();
            assert_eq!(st.delta_bytes.len(), 1);
            delta_bytes.push(st.delta_bytes[0] as u64);
            apply_stats = (
                st.last_exec.mirror_rebuilds,
                st.last_exec.mirror_apply_us,
                0,
            );
        }
        report.push_str(&format!(
            "| 增量字节 | {} | {:.0}B | {}B |\n",
            delta_bytes.len(),
            mean(&delta_bytes),
            delta_bytes.iter().max().unwrap()
        ));
        // 增量回放耗时（宿主侧 __nowUs 累计，含一次重建路径参考）。
        report.push_str(&format!(
            "\n宿主侧增量回放累计：{} 次 / {}µs（含 mirror.fetch 整体重建 {}µs）\n",
            apply_stats.0, apply_stats.1, apply_stats.2
        ));
    }

    // —— 变更型调用（robot.move）与 memory 写的单次往返（世界线程侧）。
    report.push_str("\n## 单次往返延迟（世界线程侧处理耗时，300 样本/类）\n\n| 类别 | 样本 | p50 | p95 | p99 | max |\n|---|---|---|---|---|---|\n");
    {
        let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
        assert!(s.load_code(&fixture("move_only.js")).ok);
        for _ in 0..300 {
            let out = s.tick();
            assert_eq!(out.kind, OutcomeKind::Ok);
        }
        let st = s.stats.take();
        assert_eq!(st.op_us.len(), 300);
        report.push_str(&stats_line("robot.move", &st.op_us));
        report.push_str(&format!(
            "\n（宿主侧整 exec 平均往返：{:.0}µs / 次，最大 {}µs）\n",
            st.last_exec.ipc_total_us as f64 / st.last_exec.ipc_count.max(1) as f64,
            st.last_exec.ipc_max_us
        ));
    }
    {
        let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
        assert!(s.load_code(&fixture("mem_write_only.js")).ok);
        for _ in 0..300 {
            let out = s.tick();
            assert_eq!(out.kind, OutcomeKind::Ok);
        }
        let st = s.stats.take();
        // 每 tick：根读 + 根写 + robot_memory + r.memory 写 = 4 次往返，
        // 加初始化 1 次写。
        assert_eq!(st.op_us.len(), 300 * 4 + 1, "每 tick 应恰 4 次 memory 往返");
        report.push_str(&stats_line("memory 读写（混合）", &st.op_us));
        report.push_str(&format!(
            "\n（宿主侧整 exec 平均往返：{:.0}µs / 次，最大 {}µs）\n",
            st.last_exec.ipc_total_us as f64 / st.last_exec.ipc_count.max(1) as f64,
            st.last_exec.ipc_max_us
        ));
    }

    // —— 代表性脚本整 tick p50/p95/p99（1 台 / 5 台）。
    report.push_str("\n## 代表性脚本整 tick（400 样本/场景）\n\n| 场景 | 样本 | p50 | p95 | p99 | max |\n|---|---|---|---|---|---|\n");
    for (name, world, fx) in [
        ("1 台机器人", demo_world(), "demo_one.js"),
        ("5 台机器人", five_world(), "demo_five.js"),
        ("12 台机器人（压力）", twelve_world(), "demo_five.js"),
    ] {
        let mut s = Session::new(SessionConfig::new(host_bin()), world);
        assert!(s.load_code(&fixture(fx)).ok);
        for _ in 0..400 {
            let out = s.tick();
            assert_eq!(out.kind, OutcomeKind::Ok, "{name} tick 失败");
        }
        let st = s.stats.take();
        report.push_str(&stats_line(name, &st.tick_us));
        report.push_str(&format!(
            "\n（剩余 IPC 调用数 {} 次/tick，镜像 {}B）\n",
            st.last_exec.ipc_count,
            st.mirror_bytes.last().unwrap()
        ));
        // memory 写占比（docs/architecture/06 重议判据的原始数据）。
        let mem_us: u64 = st.op_us.iter().sum();
        let tick_us: u64 = st.tick_us.iter().sum();
        report.push_str(&format!(
            "- {name}：世界线程侧全部请求处理 {mem_us}µs / 整 tick {tick_us}µs（{:.1}%）\n",
            mem_us as f64 / tick_us.max(1) as f64 * 100.0
        ));
        // 健全性断言：5 台 p99 应在数十毫秒内（debug 构建）。
        let p99 = percentile(st.tick_us.clone(), 0.99);
        assert!(p99 < 50_000, "{name} 整 tick p99 = {p99}µs，超 50ms");
    }

    // —— 单独初始化延迟（20 个全新会话：spawn + init 执行 + 提交）。
    report.push_str(
        "\n## 初始化延迟（20 样本）\n\n| 指标 | 样本 | p50 | p95 | max |\n|---|---|---|---|---|\n",
    );
    {
        let code = "Game.memory[\"a\"] = 1;\nexport function loop() {}";
        let mut times: Vec<u64> = Vec::new();
        for _ in 0..20 {
            let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
            let t0 = Instant::now();
            assert!(s.load_code(code).ok);
            times.push(t0.elapsed().as_micros() as u64);
        }
        report.push_str(&stats_line("spawn+初始化", &times));
    }

    // —— 宿主重启延迟（20 样本）。
    report.push_str("\n## 宿主重启（杀进程→重启→初始化成功，20 样本）\n\n| 指标 | 样本 | p50 | p95 | max |\n|---|---|---|---|---|\n");
    {
        let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
        assert!(s.load_code(&fixture("demo_patrol.js")).ok);
        let mut times: Vec<u64> = Vec::new();
        for _ in 0..20 {
            s.kill_host_now();
            let t0 = Instant::now();
            let init = s.restart_host();
            let dt = t0.elapsed();
            assert!(init.ok);
            times.push(dt.as_micros() as u64);
        }
        report.push_str(&stats_line("重启+初始化", &times));
    }

    // —— 超限到停止的延迟。
    report.push_str("\n## 超限到停止（10 + 5 样本）\n\n| 场景 | 样本 | p50 | p95 | max |\n|---|---|---|---|---|\n");

    {
        let mut times = Vec::new();
        for _ in 0..10 {
            let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
            assert!(s.load_code(&fixture("infinite_loop.js")).ok);
            let t0 = Instant::now();
            let out = s.tick();
            times.push(t0.elapsed().as_micros() as u64);
            assert_eq!(
                out.kind,
                OutcomeKind::Fault(ztw_api::harness::FaultClass::Script)
            );
        }
        report.push_str(&stats_line("无限循环→运行时内中断", &times));
    }
    {
        let cfg = SessionConfig::new(host_bin()).with_fault("hang_exec");
        let mut times = Vec::new();
        for _ in 0..5 {
            let mut s = Session::new(cfg.clone(), demo_world());
            assert!(s.load_code(&fixture("demo_patrol.js")).ok);
            let t0 = Instant::now();
            let out = s.tick();
            times.push(t0.elapsed().as_micros() as u64);
            assert_eq!(
                out.kind,
                OutcomeKind::Fault(ztw_api::harness::FaultClass::HostTerminated("watchdog"))
            );
        }
        report.push_str(&stats_line("挂起→看门狗终止", &times));
        let p95 = percentile(times.clone(), 0.95);
        assert!(p95 < 4_000_000, "看门狗 p95 = {p95}µs 超 4s");
    }

    // 写入记录文件。
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../records");
    std::fs::create_dir_all(dir).unwrap();
    let path = format!(
        "{}/a0-measurements-{}-{}.md",
        dir,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    std::fs::write(&path, report).unwrap();
    println!("量测记录已写入 {path}");
}
