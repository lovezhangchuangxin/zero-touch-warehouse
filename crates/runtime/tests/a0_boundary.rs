//! 主进程边界验收（docs/architecture/08 验证表「主进程边界」的 A0 子集）：
//! 超大消息、日志大小受限、终止路径不排队在 Game 请求之后；
//! 以及镜像增量回放失败 → 整体重建（docs/architecture/03）。

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{demo_world, fixture, host_bin, session};
use ztw_api::harness::{FaultClass, OutcomeKind, Session, SessionConfig};

// ---------------------------------------------------------------------------
// 超大消息：主进程在解码前拒绝
// ---------------------------------------------------------------------------

#[test]
fn oversized_frame_rejected_before_decode() {
    // 宿主故障注入：首次 loop 执行前发送声明长度 0x7FFFF000 的帧。
    let cfg = SessionConfig::new(host_bin()).with_fault("oversize_frame");
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("demo_patrol.js")).ok);
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("protocol")),
        "超大帧必须按协议故障终止宿主"
    );
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "BAD_FRAME");
    assert!(
        rec.message.contains("超上限") || rec.message.contains("exceeds"),
        "{rec:?}"
    );
    assert!(dt < Duration::from_secs(3), "拒绝必须即时：{dt:?}");
    // 会话（主进程替身）保持可用：重启宿主可完成初始化。
    // 注入按进程生效，此处不再驱动 loop（新进程首个 loop 会再次注入）。
    let init = s.restart_host();
    assert!(init.ok, "会话应能重启宿主：{:?}", init.fault);
}

#[test]
fn host_rejects_oversized_payload_with_readable_error() {
    // 宿主侧解码前限长：frame_limit 压小后，超大 memory 写得到可读错误
    // 而非协议死亡（docs/architecture/06「IPC 解码前另有限长检查」）。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.frame_limit = 8 * 1024;
    let code = r#"
function loop() {
  Game.memory["seed"] = 1;
  try {
    Game.memory["big"] = "z".repeat(100 * 1024);
    Game.log("unexpected-success");
  } catch (e) {
    Game.log("err", e.code);
  }
}
"#;
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s.logs.back().unwrap().1.clone();
    assert!(
        line.contains("FRAME_LIMIT"),
        "宿主应预检并给出可读错误：{line}"
    );
    assert!(s.host_alive(), "宿主不应因预检拒绝而死");
    // 原树不变：预检拒绝等于写未发生（真实快照比对，非恒真式）。
    assert_eq!(
        s.memory_snapshot(),
        ztw_model::MemValue::Map(vec![("seed".to_string(), ztw_model::MemValue::Num(1.0))])
    );
}

// ---------------------------------------------------------------------------
// 日志大小受限
// ---------------------------------------------------------------------------

#[test]
fn log_entry_limit_and_ring_cap() {
    let mut cfg = SessionConfig::new(host_bin());
    cfg.log_entry_limit = 2048;
    cfg.log_ring_cap = 64;
    let code = r#"
function loop() {
  try { Game.log("x".repeat(100000)); Game.log("unexpected-success"); }
  catch (e) { Game.log("log-err", e.code); }
  for (let i = 0; i < 300; i++) Game.log("fill", i);   // 环形缓冲覆盖最旧
}
"#;
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    // 超大条目被拒（有可读错误），未进入环形缓冲。
    assert!(
        s.logs.iter().all(|(_, l)| !l.starts_with("xxxx")),
        "超大日志不得入队"
    );
    // 环形缓冲有界：300 + 1 条 → 只保留最近 64 条。
    assert_eq!(s.logs.len(), 64, "日志环形缓冲必须有界");
    assert!(
        s.logs.front().unwrap().1.starts_with("fill"),
        "覆盖最旧记录"
    );
    // 每条都带 tick。
    assert!(s.logs.iter().all(|(t, _)| *t == 0));
}

// ---------------------------------------------------------------------------
// 终止路径不排队在 Game 请求之后
// ---------------------------------------------------------------------------

#[test]
fn watchdog_kill_latency_with_hung_host() {
    // 宿主挂起（无响应）：主进程看门狗在预算 + 宽限期后终止，
    // 世界线程消息循环不被 Game 队列阻塞。
    let cfg = SessionConfig::new(host_bin()).with_fault("hang_exec");
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("demo_patrol.js")).ok);
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("watchdog"))
    );
    // budget(200ms) + grace(1500ms) = 1.7s，允许调度余量。
    assert!(
        dt >= Duration::from_millis(1600) && dt < Duration::from_secs(5),
        "看门狗终止延迟 {dt:?}"
    );
    assert!(!s.host_alive());
}

#[test]
fn kill_button_path_not_queued_behind_game_requests() {
    // 日志洪泛（Game 请求流持续不断）期间从另一线程直接杀宿主：
    // 终止不等待请求队列，tick 在杀后立即返回。预算放大到 5s，
    // 保证杀点（300ms）落在请求流进行中而非预算中断之后。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.tick_budget_base_ms = 5_000;
    cfg.grace_ms = 5_000;
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("log_flood.js")).ok);
    let handle: Option<Arc<std::sync::Mutex<std::process::Child>>> = s.child_handle();
    let h = handle.unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        if let Ok(mut c) = h.lock() {
            let _ = c.kill();
        }
    });
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    // 终止不排队在 Game 队列之后：杀后消息循环即刻见到 EOF 并返回。
    // （并行测试的 CPU 争抢可能推迟杀线程，上限取看门狗宽限之上。）
    assert!(
        dt < Duration::from_millis(2500),
        "杀进程后 tick 应立即返回：{dt:?}"
    );
    assert!(
        matches!(out.kind, OutcomeKind::Fault(FaultClass::HostTerminated(_))),
        "杀进程应以宿主终止收场：{:?}",
        out.kind
    );
    // 请求流确实在杀之前已在进行（宽松下限，避免调度竞态）。
    assert!(
        out.requests_served > 0,
        "应有日志请求流：{}",
        out.requests_served
    );
}

#[test]
fn kill_host_now_reports_killed_and_preserves_state() {
    let mut s = session(demo_world());
    let code = r#"
Game.memory["persist"] = 7;
function loop() { Game.memory["persist"] = Game.memory["persist"] + 1; }
"#;
    assert!(s.load_code(code).ok);
    s.tick();
    s.tick();
    assert_eq!(
        ztw_api::memory::ReadResult::Scalar(ztw_model::MemValue::Num(9.0)),
        mem_read(&mut s, "persist")
    );
    s.kill_host_now();
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("killed")),
        "主动终止应有专属结局：{:?}",
        s.fault
    );
    // 已提交 memory 不回退。
    assert_eq!(
        mem_read(&mut s, "persist"),
        ztw_api::memory::ReadResult::Scalar(ztw_model::MemValue::Num(9.0))
    );
    assert!(s.restart_host().ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    // 重启会重新执行初始化：非幂等初始化（无守卫的赋值）把计数重置为 7，
    // 再经 loop +1 = 8。初始化幂等性是玩家程序的责任（docs/architecture/06）。
    assert_eq!(
        mem_read(&mut s, "persist"),
        ztw_api::memory::ReadResult::Scalar(ztw_model::MemValue::Num(8.0))
    );
}

fn mem_read(s: &mut Session, key: &str) -> ztw_api::memory::ReadResult {
    use ztw_model::MemValue;
    let snap = s.memory_snapshot();
    if let MemValue::Map(pairs) = snap
        && let Some((_, v)) = pairs.into_iter().find(|(k, _)| k == key)
    {
        return ztw_api::memory::ReadResult::Scalar(v);
    }
    ztw_api::memory::ReadResult::Missing
}

// ---------------------------------------------------------------------------
// 镜像增量回放失败 → 整体重建
// ---------------------------------------------------------------------------

#[test]
fn delta_replay_failure_rebuilds_mirror() {
    // 宿主注入 skip_delta_replay：take 的增量被丢弃（回放失败路径），
    // 之后同一 tick 内的查询经 mirror.fetch 按当前世界整体重建，
    // 结果与主进程一致（管理操作当 tick 可见不因回放失败丢失）。
    // 夹具 take_first_query：take 之后的查询才踩到重建。
    let cfg = SessionConfig::new(host_bin()).with_fault("skip_delta_replay");
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("take_first_query.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s
        .logs
        .iter()
        .rev()
        .find(|(_, l)| l.starts_with("take "))
        .map(|(_, l)| l.clone())
        .expect("有 take 日志");
    assert!(line.contains("after 1"), "重建后查询应正确：{line}");
    assert!(line.contains("listings 1"), "挂单消失应一致：{line}");
    // 重建确实发生：mirror.fetch 计数 ≥ 1。
    assert!(
        s.stats.last_exec.mirror_rebuilds >= 1,
        "应经 mirror.fetch 重建：{:?}",
        s.stats.last_exec
    );
    // 主进程状态一致。
    assert_eq!(s.world.gold_milli, 190_000);
    assert_eq!(s.world.my_orders.len(), 1);
}
