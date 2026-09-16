//! A1 故障注入两宿主对账（评审 2026-09 裁决的「最小共享 + 机械对账」
//! 路线）：对 [`ztw_api::host_ipc::FAULT_KNOBS`] 的每个旋钮，以同一探针
//! 分别驱动 JS 与 Python 宿主，断言可观测结局（故障类别 / 结果码 / 宿主
//! 存活 / 日志落地 / 世界终态）一致。ipc 骨架已在 host_ipc 单源，帧级行为
//! 漂移在编译期不可能；本测试兜住的是单源之外的宿主本地机制（如
//! skip_delta_replay 的置位路径、各宿主对共享骨架的接线）——2c2fc4b 曾
//! 出现 py 侧注入静默失效而平行测试矩阵未察觉的先例，评审发现而非测试
//! 发现；此后这类分叉在此处机械暴露。
//!
//! 旋钮集互锁：`every_knob_has_a_parity_probe` 保证 FAULT_KNOBS 与本文件
//! 的探针一一对应——新增旋钮漏写探针、或探针漂移出清单，都直接红。
//!
//! 双宿主测试要求 ztw-host-js 已构建：本包不依赖 ztw-runtime（bin 不随
//! 依赖构建），按兄弟路径解析；`cargo test --workspace`（CI / just gate）
//! 恒可用，单独跑本包前先 `cargo build -p ztw-runtime`。

mod common;

use common::{demo_world, js_bin};
use ztw_api::harness::{FaultClass, OutcomeKind, Session, SessionConfig};
use ztw_api::host_ipc::FAULT_KNOBS;

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

fn session_with(bin: impl AsRef<std::path::Path>, fault: &str) -> Session {
    Session::new(
        SessionConfig::new(bin.as_ref()).with_fault(fault),
        demo_world(),
    )
}

/// stderr 标记等待：标记由后台读线程异步收割，宿主打完标记后帧处理
/// 立即返回——tick 归来时缓冲可能尚未收齐（Windows CI 慢机实测竞态）。
/// 轮询至截止再断言（与 desktop 测试 wait_status 同款 CI 余量）。
fn wait_stderr_contains(s: &Session, needle: &str, why: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let text = s.stderr_text();
        if text.contains(needle) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("{why}：{text}");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// 故障类别的跨宿主可比较投影（HostTerminated 携带 reason）。
fn class_tag(c: &FaultClass) -> String {
    match c {
        FaultClass::Script => "script".into(),
        FaultClass::Environment => "environment".into(),
        FaultClass::HostTerminated(r) => format!("host:{r}"),
    }
}

/// 一次 tick 后的可观测结局。不含宿主侧 message 文案（两侧引擎的异常
/// 文本格式允许不同），只含协议与状态层事实。
#[derive(Debug)]
struct Obs {
    kind: Result<(), (String, String)>,
    host_alive: bool,
    logs: Vec<String>,
    gold_milli: i64,
    orders: usize,
    state_hash: u64,
}

impl PartialEq for Obs {
    /// logs 比较走 [`logs_equivalent`]（数字 token 按数值相等，见其
    /// 注释）——严格相等会把 190 vs 190.0 的已记录差异（a1-findings
    /// 备忘）误报为分叉。
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.host_alive == other.host_alive
            && logs_equivalent(&self.logs, &other.logs)
            && self.gold_milli == other.gold_milli
            && self.orders == other.orders
            && self.state_hash == other.state_hash
    }
}

impl Obs {
    /// 已知等价对归一化（records/a1-findings 双语言语义差异备忘）：绑定层
    /// 错误 JS 抛普通 Error、py 抛 GameError（均带 code 属性），宿主故障
    /// 帧 code 相应为 "Error" / "GameError"——语义等价，对账前归一。
    fn normalized(mut self) -> Obs {
        if let Err((class, code)) = &mut self.kind
            && class == "script"
            && code == "Error"
        {
            *code = "GameError".to_string();
        }
        self
    }
}

fn obs_after_tick(s: &mut Session) -> Obs {
    let out = s.tick();
    let kind = match out.kind {
        OutcomeKind::Ok => Ok(()),
        OutcomeKind::Fault(class) => {
            Err((class_tag(&class), s.fault.as_ref().unwrap().code.clone()))
        }
        OutcomeKind::Paused => Err(("paused".into(), String::new())),
    };
    Obs {
        kind,
        host_alive: s.host_alive(),
        logs: s.logs.iter().map(|(_, l)| l.clone()).collect(),
        gold_milli: s.world.gold_milli,
        orders: s.world.my_orders.len(),
        state_hash: s.world.state_hash(),
    }
}

/// loop 阶段旋钮的通用探针：初始化不受注入影响，首个 tick 的结局投影。
/// 前提：fixture 在 init 执行期不发起 IPC 请求——改写类旋钮（旧执行号 /
/// 旧代次）在 init 期同样生效，init 期请求会把注入提前到初始化阶段。
fn probe(bin: impl AsRef<std::path::Path>, fault: &str, code: &str) -> Obs {
    let mut s = session_with(bin, fault);
    let init = s.load_code(code);
    assert!(init.ok, "[{fault}] 初始化不受注入影响：{:?}", init.fault);
    obs_after_tick(&mut s)
}

/// init 阶段旋钮的探针：初始化中途崩溃的结局投影（类别 + 结果码）。
fn probe_init(bin: impl AsRef<std::path::Path>, fault: &str, code: &str) -> (String, String) {
    let mut s = session_with(bin, fault);
    let init = s.load_code(code);
    assert!(!init.ok, "[{fault}] 初始化应被注入打断");
    let rec = init.fault.expect("初始化故障记录");
    let tags = (class_tag(&rec.class), rec.code);
    // 初始化分支不落地（docs 06：初始化成功才提交 memory）。
    assert!(
        s.memory_snapshot() == ztw_model::MemValue::Map(vec![]),
        "[{fault}] 初始化分支不得提交：{:?}",
        s.memory_snapshot()
    );
    assert_eq!(s.world.tick, 0);
    tags
}

/// 两宿主同探针对比：返回 JS 侧投影（已归一化）供调用方做语义断言。
fn parity(fault: &str, code_js: &str, code_py: &str) -> Obs {
    let js = probe(js_bin(), fault, code_js).normalized();
    let py = probe(common::host_bin(), fault, code_py).normalized();
    assert_eq!(js, py, "[{fault}] 两宿主结局投影必须一致");
    js
}

/// 日志序列对账：逐 token 比较，数字 token 按数值相等——JS String(190)
/// →"190" 而 py str(190.0)→"190.0"（records/a1-findings 双语言语义差异
/// 备忘，对账测试首次跑出的实证），文本 token 仍须逐字一致。
fn logs_equivalent(a: &[String], b: &[String]) -> bool {
    let num_eq = |p: &str, q: &str| -> bool {
        match (p.parse::<f64>(), q.parse::<f64>()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
    };
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x == y || {
                let (vx, vy): (Vec<&str>, Vec<&str>) = (
                    x.split_whitespace().collect(),
                    y.split_whitespace().collect(),
                );
                vx.len() == vy.len() && vx.iter().zip(&vy).all(|(p, q)| p == q || num_eq(p, q))
            }
        })
}

// ---------------------------------------------------------------------------
// 旋钮集互锁：FAULT_KNOBS ↔ 探针清单一一对应
// ---------------------------------------------------------------------------

/// 本文件实例化的旋钮（带参形式；与 FAULT_KNOBS 的占位替换规则一致：
/// `<op>`→log、`<n>`→1）。新增旋钮必须在这里登记探针，删除旋钮则须
/// 同步删探针——清单与探针永不静默脱钩。
const PROBED_KNOBS: &[&str] = &[
    "abort_init_after_mem",
    "abort_after_reply:log",
    "abort_before_send:log",
    "abort_after_send",
    "dup_request:1",
    "dup_request_corrupt:1",
    "dup_complete",
    "old_exec_request",
    "old_exec_request_first",
    "stale_epoch",
    "oversize_frame",
    "hang_exec",
    "skip_delta_replay",
];

#[test]
fn every_knob_has_a_parity_probe() {
    let canonical: std::collections::BTreeSet<String> = FAULT_KNOBS
        .iter()
        .map(|k| k.replace("<op>", "log").replace("<n>", "1"))
        .collect();
    let probed: std::collections::BTreeSet<&str> = PROBED_KNOBS.iter().copied().collect();
    assert_eq!(
        canonical,
        probed.iter().map(|s| s.to_string()).collect(),
        "FAULT_KNOBS 与探针清单不一致：新增旋钮须配对账探针"
    );
}

/// 清单条目 ↔ 测试函数的机械绑定：上面的集合比对只保证清单 ↔ 清单
/// 一致，删测试函数而留条目会让互锁照绿——本测试按命名约定
/// （knob_<base>_matches_both_hosts）对本文件源码自检，堵住该方向。
#[test]
fn every_probed_knob_has_a_test_function() {
    let src = include_str!("a1_fault_parity.rs");
    for knob in PROBED_KNOBS {
        let base = knob.split(':').next().unwrap();
        let expected = format!("fn knob_{base}_matches_both_hosts");
        assert!(
            src.contains(&expected),
            "清单条目 {knob} 缺少对应测试函数 {expected}——删除探针须同步删条目"
        );
    }
}

// ---------------------------------------------------------------------------
// init 阶段断连
// ---------------------------------------------------------------------------

#[test]
fn knob_abort_init_after_mem_matches_both_hosts() {
    let js = probe_init(
        js_bin(),
        "abort_init_after_mem",
        &fixture("init_mem_write.js"),
    );
    let py = probe_init(
        common::host_bin(),
        "abort_init_after_mem",
        &fixture("init_mem_write.py"),
    );
    assert_eq!(js, py);
    assert_eq!(js, ("host:crash".to_string(), "HOST_EXITED".to_string()));
}

// ---------------------------------------------------------------------------
// 三时点断连（loop 阶段）
// ---------------------------------------------------------------------------

#[test]
fn knob_abort_before_send_matches_both_hosts() {
    // take 已提交、log 未发出即断连：恰好一单、一次扣款、未送达的请求
    // 不落地。
    let js = parity(
        "abort_before_send:log",
        &fixture("take_then_log.js"),
        &fixture("take_then_log.py"),
    );
    assert_eq!(js.kind, Err(("host:crash".into(), "HOST_EXITED".into())));
    assert!(!js.host_alive);
    assert!(js.logs.is_empty(), "未送达的请求不得落地：{:?}", js.logs);
    assert_eq!(js.orders, 1);
    assert_eq!(js.gold_milli, 200_000 - 10_000);
}

#[test]
fn knob_abort_after_send_matches_both_hosts() {
    // 提交后、回复前断连是主进程侧竞态（EOF 或回复写失败两分支；take
    // 是否已提交两分支）——两宿主各自落在任一分支均可，但必须满足同一
    // 不变量：扣款与订单数一致、宿主死亡、后续重启不重放。
    for bin in [js_bin(), std::path::PathBuf::from(common::host_bin())] {
        let mut s = session_with(&bin, "abort_after_send");
        assert!(
            s.load_code(&fixture_if(&bin, "take_if_missing")).ok,
            "{:?}",
            s.fault
        );
        let out = s.tick();
        assert!(
            matches!(
                out.kind,
                OutcomeKind::Fault(FaultClass::HostTerminated("crash" | "write"))
            ),
            "[{}] 断连症状必须是 EOF 或回复写失败：{:?}",
            bin.display(),
            out.kind
        );
        let orders = s.world.my_orders.len();
        assert!(orders <= 1, "至多一单：{orders}");
        assert_eq!(s.world.gold_milli, 200_000 - 10_000 * orders as i64);
        assert!(!s.host_alive());
        // 重启不重放：程序先查真实订单，最终恰好一单、一次扣款。
        assert!(s.restart_host().ok, "{:?}", s.fault);
        assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
        assert_eq!(s.world.my_orders.len(), 1);
        assert_eq!(s.world.gold_milli, 200_000 - 10_000);
        assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
        assert_eq!(s.world.my_orders.len(), 1, "不得重复接单");
    }
}

#[test]
fn knob_abort_after_reply_matches_both_hosts() {
    // 回复送达后断连：op 已在主进程落地（日志恰好一条），宿主进程崩溃。
    let js = parity(
        "abort_after_reply:log",
        &fixture("old_exec_log.js"),
        &fixture("old_exec_log.py"),
    );
    assert_eq!(js.kind, Err(("host:crash".into(), "HOST_EXITED".into())));
    assert!(!js.host_alive);
    assert_eq!(js.logs, vec!["hello".to_string()]);
}

// ---------------------------------------------------------------------------
// 重复消息与旧消息（帧级旋钮；ipc 主体在 host_ipc 单源，此处验证两宿主
// 接线后端到端等价）
// ---------------------------------------------------------------------------

#[test]
fn knob_dup_request_matches_both_hosts() {
    // 合法重发被主进程去重，结局投影与"不重发"逐项相同（重发不增
    // ipc_count / requests_served）——注入静默丢失只有 stderr 标记可证
    // （host_ipc 的 dup_request 分支先打标记再重发），故本探针不走
    // parity() 而逐宿主持有会话断言。
    let pairs = [
        (js_bin(), fixture("dup_log.js")),
        (
            std::path::PathBuf::from(common::host_bin()),
            fixture("dup_log.py"),
        ),
    ];
    let mut obs = Vec::new();
    for (bin, code) in &pairs {
        let mut s = session_with(bin, "dup_request:1");
        assert!(s.load_code(code).ok, "{:?}", s.fault);
        obs.push(obs_after_tick(&mut s).normalized());
        assert_eq!(s.logs.len(), 1, "重发不重复执行：{:?}", s.logs);
        wait_stderr_contains(
            &s,
            "故障注入 dup_request:1",
            &format!(
                "{}：重发注入必须真实发生（静默丢失正是对账要抓的形态）",
                bin.display()
            ),
        );
    }
    assert_eq!(obs[0], obs[1], "两宿主结局投影必须一致");
    assert_eq!(obs[0].kind, Ok(()));
}

#[test]
fn knob_dup_request_corrupt_matches_both_hosts() {
    let js = parity(
        "dup_request_corrupt:1",
        &fixture("dup_log.js"),
        &fixture("dup_log.py"),
    );
    assert_eq!(
        js.kind,
        Err(("host:protocol".into(), "DUP_REQUEST_MISMATCH".into()))
    );
    assert!(!js.host_alive);
    assert_eq!(js.logs, vec!["first".to_string()], "原请求恰好落地一次");
}

#[test]
fn knob_old_exec_request_matches_both_hosts() {
    let js = parity(
        "old_exec_request",
        &fixture("old_exec_log.js"),
        &fixture("old_exec_log.py"),
    );
    assert_eq!(
        js.kind,
        Err(("script".into(), "GameError".into())),
        "（Error≡GameError 已归一化）"
    );
    assert!(js.host_alive, "可恢复拒绝，宿主存活");
    assert!(js.logs.is_empty(), "被拒的 log 不应落地：{:?}", js.logs);
}

#[test]
fn knob_old_exec_request_first_matches_both_hosts() {
    // 被拒后继续发号 + 完成帧对账：两 tick 各拒首请求、各落两条后续
    // 日志。此前该旋钮仅 JS 侧实现（测试落点选择）；共享 ipc 后两宿主
    // 行为统一，在此取得 py 侧首份覆盖。
    let pairs = [
        (js_bin(), fixture("old_exec_recover.js")),
        (
            std::path::PathBuf::from(common::host_bin()),
            fixture("old_exec_recover.py"),
        ),
    ];
    for (bin, code) in &pairs {
        let mut s = session_with(bin, "old_exec_request_first");
        assert!(s.load_code(code).ok, "{:?}", s.fault);
        let out = s.tick();
        assert_eq!(
            out.kind,
            OutcomeKind::Ok,
            "{}：{:?}",
            bin.display(),
            s.fault
        );
        assert!(!s.logs.iter().any(|(_, l)| l == "first-rejected"));
        assert_eq!(s.logs.len(), 2, "{:?}", s.logs);
        assert!(s.host_alive());
        // 注入按执行重置：第二 tick 首请求再次被拒，仍恰好两条后续日志。
        let out = s.tick();
        assert_eq!(
            out.kind,
            OutcomeKind::Ok,
            "{}：{:?}",
            bin.display(),
            s.fault
        );
        assert_eq!(s.logs.len(), 4, "闸门须随新执行复位：{:?}", s.logs);
        assert!(!s.logs.iter().any(|(_, l)| l == "first-rejected"));
    }
}

#[test]
fn knob_stale_epoch_matches_both_hosts() {
    let js = parity(
        "stale_epoch",
        &fixture("old_exec_log.js"),
        &fixture("old_exec_log.py"),
    );
    assert_eq!(js.kind, Err(("host:protocol".into(), "STALE_EPOCH".into())));
    assert!(!js.host_alive);
    assert!(js.logs.is_empty(), "旧代次消息不得执行");
}

// ---------------------------------------------------------------------------
// 完成帧连发：第二帧按旧执行丢弃，只结算一次
// ---------------------------------------------------------------------------

#[test]
fn knob_dup_complete_matches_both_hosts() {
    let pairs = [
        (js_bin(), fixture("move_only.js")),
        (
            std::path::PathBuf::from(common::host_bin()),
            fixture("move_only.py"),
        ),
    ];
    let mut hashes = Vec::new();
    for (bin, code) in &pairs {
        let mut s = session_with(bin, "dup_complete");
        assert!(s.load_code(code).ok, "{:?}", s.fault);
        assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
        // 上一执行的残留完成帧先到必须被丢弃，本执行真实帧照常处理。
        assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
        assert_eq!(s.world.tick, 2);
        // dup_complete 注入点在各宿主 main.rs 本地（对账测试的防区），
        // 且残留帧被丢弃无任何主进程副作用——静默丢失只有 stderr 标记
        // 可证（注入分支连发前打印）。
        wait_stderr_contains(
            &s,
            "故障注入 dup_complete",
            &format!("{}：连发注入必须真实发生", bin.display()),
        );
        hashes.push(s.world.state_hash());
    }
    assert_eq!(hashes[0], hashes[1], "两 tick 的世界终态必须一致");
}

// ---------------------------------------------------------------------------
// 传输层注入（宿主本地机制，非 ipc 帧改写）
// ---------------------------------------------------------------------------

#[test]
fn knob_oversize_frame_matches_both_hosts() {
    let js = parity(
        "oversize_frame",
        &fixture("old_exec_log.js"),
        &fixture("old_exec_log.py"),
    );
    assert_eq!(js.kind, Err(("host:protocol".into(), "BAD_FRAME".into())));
    assert!(!js.host_alive);
    assert!(js.logs.is_empty());
}

#[test]
fn knob_hang_exec_matches_both_hosts() {
    // 宿主无响应：主进程看门狗超宽限期终止（预算 + 宽限期 ≈ 1.7s/宿主）。
    let js = parity(
        "hang_exec",
        &fixture("old_exec_log.js"),
        &fixture("old_exec_log.py"),
    );
    assert_eq!(
        js.kind,
        Err(("host:watchdog".into(), "WATCHDOG_KILL".into()))
    );
    assert!(!js.host_alive);
    assert!(js.logs.is_empty());
}

// ---------------------------------------------------------------------------
// skip_delta_replay：宿主本地机制（置位路径引擎相关，正是 2c2fc4b 漂移
// 事故的形态——对账重点）
// ---------------------------------------------------------------------------

#[test]
fn knob_skip_delta_replay_matches_both_hosts() {
    let pairs = [
        (js_bin(), fixture("take_first_query.js")),
        (
            std::path::PathBuf::from(common::host_bin()),
            fixture("take_first_query.py"),
        ),
    ];
    let mut all_logs = Vec::new();
    for (bin, code) in &pairs {
        let mut s = session_with(bin, "skip_delta_replay");
        assert!(s.load_code(code).ok, "{:?}", s.fault);
        let out = s.tick();
        assert_eq!(
            out.kind,
            OutcomeKind::Ok,
            "{}：{:?}",
            bin.display(),
            s.fault
        );
        // 注入必须真实生效：镜像增量被丢弃，下一次查询经 mirror.fetch
        // 整体重建（重建计数 ≥ 1）。静默失效正是 2c2fc4b 的事故形态。
        assert!(
            s.stats.last_exec.mirror_rebuilds >= 1,
            "{}：增量丢弃必须触发整体重建：{:?}",
            bin.display(),
            s.stats.last_exec
        );
        all_logs.push(logs_of(&s));
    }
    assert!(
        logs_equivalent(&all_logs[0], &all_logs[1]),
        "两宿主日志序列（数字 token 按数值）必须一致：{:?} vs {:?}",
        all_logs[0],
        all_logs[1]
    );
    for logs in &all_logs {
        let line = logs
            .iter()
            .rev()
            .find(|l| l.starts_with("take "))
            .expect("有 take 日志");
        assert!(line.contains("after 1"), "重建后查询应正确：{line}");
        assert!(line.contains("listings 1"), "挂单消失应一致：{line}");
    }
}

fn logs_of(s: &Session) -> Vec<String> {
    s.logs.iter().map(|(_, l)| l.clone()).collect()
}

/// 按宿主二进制语言挑同名 fixture（js / py 扩展）。
fn fixture_if(bin: &std::path::Path, stem: &str) -> String {
    let ext = if SessionConfig::new(bin).is_python_host() {
        "py"
    } else {
        "js"
    };
    fixture(&format!("{stem}.{ext}"))
}
