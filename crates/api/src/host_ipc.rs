//! 宿主侧 IPC 骨架（ztw-host-js / ztw-host-py 共享，评审 2026-09 裁决的
//! 「最小共享 + 机械对账」路线）：ZTW_FAULT 故障注入框架、宿主会话状态
//! 与同步请求-回复循环。引擎差异（rquickjs 的 Env 生命周期 / PyO3 的命名
//! 留驻与 GIL）留在各宿主 main.rs；本模块只收两侧逐字等价的骨架——
//! ipc 主体不依赖引擎上下文（两侧包装器的引擎参数均未使用），直接以
//! 普通函数共享，无需 trait 间接层。
//!
//! 两宿主平行演进是明文决策（docs/architecture/03「双宿主平行演进」），
//! 协议演进时须双改的同步点清单亦在该文档；旋钮集与帧收发行为在此
//! 单源，行为等价性由 crates/runtime-py/tests/a1_fault_parity.rs 以
//! 同一探针跑两宿主机械对账。
//!
//! 公共面（`with_host` / `HostState` / `ipc_roundtrip` 等）仅供两宿主
//! 二进制与宿主测试消费：会终止进程（exit / abort）且操作宿主侧会话
//! 状态，主进程一侧的代码不应链接使用。
//!
//! 故障注入（ZTW_FAULT，逗号分隔，测试专用；清单即 [`FAULT_KNOBS`]）：
//!   abort_init_after_mem        初始化第一条 memory 回复后 abort()
//!   abort_after_reply:<op>      指定 op 的回复送达后 abort()
//!   abort_before_send:<op>      指定 op 的请求发出前 abort()（提交前断连）
//!   abort_after_send            首条请求发出后立即 abort()（提交后回复前断连）
//!   dup_request:<n>             第 n 号请求收到回复后逐字节重发（去重缓存）
//!   dup_request_corrupt:<n>     第 n 号请求同号异负载重发（协议故障路径）
//!   dup_complete                完成帧连发两次（旧执行消息丢弃，只结算一次）
//!   old_exec_request            请求帧改带上一执行号（EXEC_CLOSED 拒绝）
//!   old_exec_request_first      仅本执行首个请求改带旧执行号，被拒后照常发号
//!                               （验证拒绝计入对账：不触发 GAP / MISMATCH）
//!   stale_epoch                 请求帧改带上一宿主代次（STALE_EPOCH 拒绝）
//!   oversize_frame              首次 loop 执行前发送超大帧
//!   hang_exec                   首次 loop 执行前挂起（模拟宿主无响应）
//!   skip_delta_replay           丢弃镜像增量（触发整体重建路径；置位机制
//!                               引擎相关，留在各宿主的 inject_loop_faults）

use std::cell::RefCell;
use std::time::Instant;

use crate::protocol::{
    ExecStats, HostFrame, MainFrame, err_result, raw_or_quoted, read_frame, write_frame,
};

/// 宿主读取主进程帧的上限（镜像随世界规模增长，取宽上限）。
pub const HOST_READ_LIMIT: u64 = 64 * 1024 * 1024;

/// 全部注入旋钮（带参形式；`<op>`/`<n>` 为参数占位）。清单、[`Faults`]
/// 字段与 [`parse_faults`] 三者由本模块单测互锁；行为对账由
/// a1_fault_parity 以同款探针跑两宿主——新增旋钮而漏改任何一侧会在
/// 这两处暴露。
pub const FAULT_KNOBS: &[&str] = &[
    "abort_init_after_mem",
    "abort_after_reply:<op>",
    "abort_before_send:<op>",
    "abort_after_send",
    "dup_request:<n>",
    "dup_request_corrupt:<n>",
    "dup_complete",
    "old_exec_request",
    "old_exec_request_first",
    "stale_epoch",
    "oversize_frame",
    "hang_exec",
    "skip_delta_replay",
];

/// 墙钟毫秒（Unix 纪元）：执行预算 deadline 的基准，两宿主共用。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// 故障注入
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Faults {
    pub abort_init_after_mem: bool,
    pub abort_after_reply_op: Option<String>,
    pub abort_before_send_op: Option<String>,
    pub abort_after_send: bool,
    pub dup_request_id: Option<u64>,
    /// 同号异负载重发（预期主进程 DUP_REQUEST_MISMATCH 终止宿主）。
    pub dup_request_corrupt_id: Option<u64>,
    /// 完成帧连发两次（预期第二次被主进程按旧执行丢弃，不重复结算）。
    pub dup_complete: bool,
    pub old_exec_request: bool,
    /// 仅本执行的首个请求改写为旧执行号：EXEC_CLOSED 被拒后宿主继续
    /// 正常发号，验证主进程把拒绝计入对账（不误判 GAP / MISMATCH）。
    pub old_exec_request_first: bool,
    pub stale_epoch: bool,
    pub oversize_frame: bool,
    pub hang_exec: bool,
    pub skip_delta_replay: bool,
}

pub fn parse_faults(raw: Option<String>) -> Faults {
    let mut f = Faults::default();
    let Some(raw) = raw else { return f };
    for part in raw.split(',') {
        let part = part.trim();
        if part == "abort_init_after_mem" {
            f.abort_init_after_mem = true;
        } else if let Some(op) = part.strip_prefix("abort_after_reply:") {
            f.abort_after_reply_op = Some(op.to_string());
        } else if let Some(op) = part.strip_prefix("abort_before_send:") {
            f.abort_before_send_op = Some(op.to_string());
        } else if part == "abort_after_send" {
            f.abort_after_send = true;
        } else if let Some(n) = part.strip_prefix("dup_request:") {
            f.dup_request_id = n.parse().ok();
        } else if let Some(n) = part.strip_prefix("dup_request_corrupt:") {
            f.dup_request_corrupt_id = n.parse().ok();
        } else if part == "dup_complete" {
            f.dup_complete = true;
        } else if part == "old_exec_request" {
            f.old_exec_request = true;
        } else if part == "old_exec_request_first" {
            f.old_exec_request_first = true;
        } else if part == "stale_epoch" {
            f.stale_epoch = true;
        } else if part == "oversize_frame" {
            f.oversize_frame = true;
        } else if part == "hang_exec" {
            f.hang_exec = true;
        } else if part == "skip_delta_replay" {
            f.skip_delta_replay = true;
        }
    }
    f
}

// ---------------------------------------------------------------------------
// 宿主状态（单线程，thread_local）
// ---------------------------------------------------------------------------

pub struct HostState {
    /// 进程 stdout 句柄。字段私有只为把帧写入约束在本模块——Complete /
    /// Fault 终态帧仍由各宿主经全局 stdout 写出（同进程同一把锁，且
    /// write_frame 每帧 flush，无交错风险）。
    stdout: std::io::Stdout,
    /// 单调请求号：只经 ipc_roundtrip 自增 / 限长拒绝回退，宿主侧只读
    /// （完成帧引用最后发送号）——宿主直接改它会静默破坏主进程对账
    /// （REQUEST_ID_GAP / LAST_REQUEST_MISMATCH 误判），编译期不拦截。
    pub request_id: u64,
    pub stats: ExecStats,
    pub faults: Faults,
    pub in_init: bool,
    pub frame_limit: usize,
    /// 当前宿主代次与执行编号：来自主进程 Exec 帧，随帧回显，
    /// 供主进程做旧代次 / 旧执行拒绝（docs/architecture/03 v2 会话头）。
    pub host_epoch: u64,
    pub execution_id: u64,
    /// 本执行是否已发出首个请求（old_exec_request_first 的一次性闸门，
    /// 收到新 Exec 帧时复位）。
    pub exec_first_request_sent: bool,
    /// stderr 前缀（"ztw-host-js" / "ztw-host-py"），启动时经 configure 置入。
    pub host_tag: &'static str,
}

thread_local! {
    static HOST: RefCell<HostState> = RefCell::new(HostState {
        stdout: std::io::stdout(),
        request_id: 0,
        stats: ExecStats::default(),
        faults: Faults::default(),
        in_init: false,
        frame_limit: 1024 * 1024,
        host_epoch: 0,
        execution_id: 0,
        exec_first_request_sent: false,
        host_tag: "ztw-host",
    });
}

/// 宿主状态访问（单线程；借用约束封装在此，调用方不再触 RefCell）。
pub fn with_host<R>(f: impl FnOnce(&mut HostState) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

/// 启动配置：注入旋钮、帧上限与 stderr 前缀。
pub fn configure(faults: Faults, frame_limit: usize, host_tag: &'static str) {
    with_host(|h| {
        h.faults = faults;
        h.frame_limit = frame_limit;
        h.host_tag = host_tag;
    });
}

/// 每执行复位：会话头（代次 / 执行号）、统计与 old_exec_request_first
/// 的一次性闸门。
pub fn begin_exec(is_init: bool, host_epoch: u64, execution_id: u64) {
    with_host(|h| {
        h.in_init = is_init;
        h.stats = ExecStats::default();
        h.host_epoch = host_epoch;
        h.execution_id = execution_id;
        h.exec_first_request_sent = false;
    });
}

// ---------------------------------------------------------------------------
// 同步 IPC：发请求、阻塞等回复。协议破坏一律退出（exit 3）。
// ---------------------------------------------------------------------------

pub fn ipc_roundtrip(op: String, payload: String) -> String {
    with_host(|h| {
        let t0 = Instant::now();
        // 故障注入：提交前断连（主进程未见过该请求）。
        if let Some(target) = &h.faults.abort_before_send_op
            && op == *target
        {
            eprintln!("{}: 故障注入 abort_before_send:{target}", h.host_tag);
            std::process::abort();
        }
        h.request_id += 1;
        let rid = h.request_id;
        let (epoch, exec_id) = (h.host_epoch, h.execution_id);
        // 注入改写：旧执行号 / 旧代次（主进程侧拒绝路径的触发器）。
        // old_exec_request_first 只消费本执行的首个请求，其余照常——
        // 场景是“被拒后继续发号”，须留给宿主后续正常请求。
        let frame_epoch = if h.faults.stale_epoch && epoch > 0 {
            epoch - 1
        } else {
            epoch
        };
        let rewrite_old_exec = exec_id > 0
            && (h.faults.old_exec_request
                || (h.faults.old_exec_request_first && !h.exec_first_request_sent));
        if h.faults.old_exec_request_first {
            h.exec_first_request_sent = true;
        }
        let frame_exec = if rewrite_old_exec {
            exec_id - 1
        } else {
            exec_id
        };
        let frame = HostFrame::Request {
            v: crate::protocol::PROTOCOL_VERSION,
            host_epoch: frame_epoch,
            execution_id: frame_exec,
            request_id: rid,
            op: op.clone(),
            payload: raw_or_quoted(payload.clone()),
        };
        // 解码前的限长检查：先实测完整帧的序列化长度（JSON 字符串转义会
        // 膨胀），超限以可读错误拒绝，避免必然被杀的协议路径。
        // 拒绝时不消耗请求号（帧未发出，主进程不会见到该 id）。
        let body = serde_json::to_vec(&frame).unwrap_or_default();
        if body.len() + 4 > h.frame_limit {
            h.request_id -= 1;
            return err_result(
                "FRAME_LIMIT",
                &format!("请求帧 {}B 超上限 {}B", body.len() + 4, h.frame_limit),
            )
            .get()
            .to_string();
        }
        if write_frame(&mut h.stdout, &frame).is_err() {
            eprintln!("{}: 写 Game 请求失败", h.host_tag);
            std::process::exit(3);
        }
        // 故障注入：提交后、回复前断连（主进程可能已提交，回复未送达）。
        // 仅第一代次宿主触发：测试要在崩溃后重启同款宿主跑“不重放”流程。
        if h.faults.abort_after_send && rid == 1 && epoch == 1 {
            eprintln!("{}: 故障注入 abort_after_send", h.host_tag);
            std::process::abort();
        }
        let reply: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{}: 读回复失败：{e}", h.host_tag);
                std::process::exit(3);
            }
        };
        let MainFrame::Reply {
            host_epoch: reply_epoch,
            execution_id: reply_exec,
            request_id,
            result,
            ..
        } = reply
        else {
            eprintln!("{}: 期望 Reply 帧", h.host_tag);
            std::process::exit(3);
        };
        // 对称校验：回复必须属于当前代次与执行，且回复号等于请求号。
        if request_id != rid || reply_epoch != epoch || reply_exec != exec_id {
            eprintln!(
                "{}: 回复错位 rid {request_id}!={rid} / epoch {reply_epoch}!={epoch} / exec {reply_exec}!={exec_id}",
                h.host_tag
            );
            std::process::exit(3);
        }
        let dt = t0.elapsed().as_micros() as u64;
        h.stats.ipc_count += 1;
        h.stats.ipc_total_us += dt;
        h.stats.ipc_max_us = h.stats.ipc_max_us.max(dt);
        if op == "mirror.fetch" {
            h.stats.mirror_rebuilds += 1;
        }
        // 故障注入（一次性判断由调用方保证场景唯一）。
        if h.in_init && h.faults.abort_init_after_mem && op.starts_with("mem.") {
            eprintln!("{}: 故障注入 abort_init_after_mem", h.host_tag);
            std::process::abort();
        }
        if let Some(target) = &h.faults.abort_after_reply_op
            && op == *target
        {
            eprintln!("{}: 故障注入 abort_after_reply:{target}", h.host_tag);
            std::process::abort();
        }
        // 故障注入：同号同负载重发一次（触发主进程去重缓存路径）。
        if h.faults.dup_request_id == Some(rid) {
            eprintln!("{}: 故障注入 dup_request:{rid}", h.host_tag);
            if write_frame(&mut h.stdout, &frame).is_err() {
                eprintln!("{}: 重发 Game 请求失败", h.host_tag);
                std::process::exit(3);
            }
            let second: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("{}: 读重发回复失败：{e}", h.host_tag);
                    std::process::exit(3);
                }
            };
            let MainFrame::Reply {
                request_id: rid2,
                result: result2,
                ..
            } = second
            else {
                eprintln!("{}: 期望重发的 Reply 帧", h.host_tag);
                std::process::exit(3);
            };
            if rid2 != rid {
                eprintln!("{}: 重发回复请求号错位 {rid2} != {rid}", h.host_tag);
                std::process::exit(3);
            }
            if result2.get() != result.get() {
                eprintln!("{}: 重发回复与原回复不一致", h.host_tag);
                std::process::exit(3);
            }
        }
        // 故障注入：同号异负载重发（预期主进程终止宿主；读到 EOF 走退出路径）。
        if h.faults.dup_request_corrupt_id == Some(rid) {
            eprintln!("{}: 故障注入 dup_request_corrupt:{rid}", h.host_tag);
            let mut dup = frame.clone();
            if let HostFrame::Request { payload, .. } = &mut dup {
                // 指纹不同，负载仍可解析。v4 注意：RawValue 的捕获区间
                // 会跳过值前后空白，只能在值内部（首字符之后）插空格
                // 才能跨线存活；单字符标量无处可插，退化为空数组占位。
                let mut text = payload.get().to_string();
                if text.chars().count() >= 2 {
                    text.insert(1, ' ');
                } else {
                    text = "[]".to_string();
                }
                *payload = raw_or_quoted(text);
            }
            if write_frame(&mut h.stdout, &dup).is_err() {
                std::process::exit(3);
            }
            match read_frame::<_, MainFrame>(&mut std::io::stdin(), HOST_READ_LIMIT) {
                Ok(MainFrame::Reply { .. }) => {
                    // 主进程未拒绝 = 协议语义破坏；退出让测试失败得显式。
                    eprintln!("{}: 同号异负载未被主进程拒绝", h.host_tag);
                    std::process::exit(3);
                }
                Ok(_) => {
                    eprintln!("{}: 期望重发的 Reply 帧", h.host_tag);
                    std::process::exit(3);
                }
                Err(_) => std::process::exit(3),
            }
        }
        result.get().to_string()
    })
}

// ---------------------------------------------------------------------------
// 首次 loop 前的传输层注入（oversize_frame / hang_exec，两侧逐字等价；
// skip_delta_replay 的置位机制引擎相关，留在各宿主）
// ---------------------------------------------------------------------------

pub fn inject_oversize_or_hang(oversize: bool, hang: bool) {
    use std::io::Write;
    if oversize {
        with_host(|h| eprintln!("{}: 故障注入 oversize_frame", h.host_tag));
        let mut out = std::io::stdout();
        let _ = out.write_all(&0x7FFF_F000u32.to_be_bytes());
        let _ = out.write_all(&[b'x'; 4096]);
        let _ = out.flush();
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
    if hang {
        with_host(|h| eprintln!("{}: 故障注入 hang_exec", h.host_tag));
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

// ---------------------------------------------------------------------------
// 单测：旋钮清单 ↔ Faults 字段 ↔ parse_faults 三方互锁
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `<op>` / `<n>` 占位替换为可解析实例。
    fn instantiate(knob: &str) -> String {
        knob.replace("<op>", "log").replace("<n>", "1")
    }

    #[test]
    fn every_knob_parses_into_its_field() {
        let all = FAULT_KNOBS
            .iter()
            .map(|k| instantiate(k))
            .collect::<Vec<_>>();
        let f = parse_faults(Some(all.join(",")));
        assert!(f.abort_init_after_mem);
        assert_eq!(f.abort_after_reply_op.as_deref(), Some("log"));
        assert_eq!(f.abort_before_send_op.as_deref(), Some("log"));
        assert!(f.abort_after_send);
        assert_eq!(f.dup_request_id, Some(1));
        assert_eq!(f.dup_request_corrupt_id, Some(1));
        assert!(f.dup_complete);
        assert!(f.old_exec_request);
        assert!(f.old_exec_request_first);
        assert!(f.stale_epoch);
        assert!(f.oversize_frame);
        assert!(f.hang_exec);
        assert!(f.skip_delta_replay);
    }

    #[test]
    fn none_and_unknown_knobs_are_inert() {
        let f = parse_faults(None);
        assert_eq!(f, Faults::default());
        // 未知旋钮静默忽略（历史行为）：拼错不致宿主启动失败。
        let f = parse_faults(Some("nonsense, dup_request:x".into()));
        assert_eq!(f, Faults::default());
    }

    /// 互锁的逐项方向：清单里每个旋钮单独解析都必须产生非默认状态——
    /// 补上"往 FAULT_KNOBS 加了字符串但 parse_faults 没接分支"的盲区
    /// （此时旋钮 inert，全量 join 测不到）。
    #[test]
    fn each_knob_alone_parses_non_default() {
        for k in FAULT_KNOBS {
            let f = parse_faults(Some(instantiate(k)));
            assert_ne!(
                f,
                Faults::default(),
                "旋钮 {k} 单独解析不得仍为默认值（清单 ↔ parse 漏接）"
            );
        }
    }
}
