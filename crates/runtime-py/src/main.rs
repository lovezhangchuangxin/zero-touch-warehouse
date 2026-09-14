//! A1 Python 宿主进程（docs/architecture/03「Python 宿主」）。
//!
//! - 内嵌 CPython（PyO3 auto-initialize + vendored python-build-standalone，
//!   版本随构建锁定）；解释器驻留本进程与调用线程（main），生命周期内
//!   不重建；热重载重建的是玩家命名空间（全新 dict 重新执行程序）。
//! - stdio 长度前缀 JSON 协议与 ztw-host-js 同构（v2 会话头：host_epoch /
//!   execution_id / request_id 随帧回显与校验）。
//! - 资源限制三层（docs 03）：①配额分配器覆盖 RAW/MEM/OBJ 三域，超额
//!   分配返回 NULL、玩家代码收 MemoryError（脚本级，docs 03 故障分级——
//!   与 JS OOM 归环境级不同）；②宿主内看门狗线程在 deadline 到期后
//!   宽限期内反复 `PyErr_SetInterruptEx` 注入 KeyboardInterrupt（字节码
//!   边界生效、**可被捕获**——与 JS 中断不可捕获不同，反复吞掉由第三层
//!   兜底）；③主进程权威看门狗超宽限终止宿主进程。
//! - 绑定层：bootstrap.py（ztw-api 内嵌）执行进玩家命名空间，原生桥
//!   `__ztw`（__ipc / __now_us）由本宿主注册；查询经宿主本地镜像应答，
//!   变更型调用与 memory 读写走同步 IPC。
//! - 能力收窄见 narrow.rs（__builtins__ / meta_path 白名单 + audit hook）。
//!
//! 故障注入（ZTW_FAULT，与 ztw-host-js 同一套）：abort_init_after_mem、
//! abort_after_reply:<op>、abort_before_send:<op>、abort_after_send、
//! dup_request:<n>、dup_request_corrupt:<n>、dup_complete、
//! old_exec_request、stale_epoch、hang_exec、skip_delta_replay、
//! oversize_frame。

mod narrow;
mod py_alloc;

use std::cell::RefCell;
use std::ffi::CString;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use pyo3::prelude::*;
use pyo3::types::{PyCode, PyCodeInput, PyDict, PyDictMethods, PyModule, PyTraceback};
use ztw_api::BOOTSTRAP_PY;
use ztw_api::protocol::{ExecStats, HostFrame, MainFrame, err_result, read_frame, write_frame};

use py_alloc::SIGINT;

/// 宿主读取主进程帧的上限（与 ztw-host-js 一致）。
const HOST_READ_LIMIT: u64 = 64 * 1024 * 1024;

/// 玩家源码的编译名：堆栈定位用，不落盘。
const PLAYER_FILENAME: &str = "<player>";
/// 绑定层的编译名（堆栈帧过滤用）。
const BOOTSTRAP_FILENAME: &str = "<bootstrap>";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// 宿主状态（单线程，thread_local——与 ztw-host-js 同构）
// ---------------------------------------------------------------------------

struct HostState {
    stdout: std::io::Stdout,
    request_id: u64,
    stats: ExecStats,
    faults: Faults,
    in_init: bool,
    frame_limit: usize,
    host_epoch: u64,
    execution_id: u64,
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
    });
}

#[derive(Debug, Default, Clone)]
struct Faults {
    abort_init_after_mem: bool,
    abort_after_reply_op: Option<String>,
    abort_before_send_op: Option<String>,
    abort_after_send: bool,
    dup_request_id: Option<u64>,
    dup_request_corrupt_id: Option<u64>,
    dup_complete: bool,
    old_exec_request: bool,
    stale_epoch: bool,
    oversize_frame: bool,
    hang_exec: bool,
    skip_delta_replay: bool,
}

fn parse_faults(raw: Option<String>) -> Faults {
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
// 原生桥 __ztw（__ipc / __now_us）
// ---------------------------------------------------------------------------

/// Python 侧同步 IPC 入口：发请求、阻塞等回复。协议破坏一律退出（exit 3）。
#[pyfunction]
fn __ipc(op: String, payload: String) -> String {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let t0 = std::time::Instant::now();
        // 故障注入：提交前断连（主进程未见过该请求）。
        if let Some(target) = &h.faults.abort_before_send_op
            && op == *target
        {
            eprintln!("ztw-host-py: 故障注入 abort_before_send:{target}");
            std::process::abort();
        }
        h.request_id += 1;
        let rid = h.request_id;
        let (epoch, exec_id) = (h.host_epoch, h.execution_id);
        // 注入改写：旧执行号 / 旧代次（主进程侧拒绝路径的触发器）。
        let frame_epoch = if h.faults.stale_epoch && epoch > 0 {
            epoch - 1
        } else {
            epoch
        };
        let frame_exec = if h.faults.old_exec_request && exec_id > 0 {
            exec_id - 1
        } else {
            exec_id
        };
        let frame = HostFrame::Request {
            v: ztw_api::protocol::PROTOCOL_VERSION,
            host_epoch: frame_epoch,
            execution_id: frame_exec,
            request_id: rid,
            op: op.clone(),
            payload,
        };
        // 解码前的限长检查（拒绝时不消耗请求号，帧未发出主进程不会见到）。
        let body = serde_json::to_vec(&frame).unwrap_or_default();
        if body.len() + 4 > h.frame_limit {
            h.request_id -= 1;
            return err_result(
                "FRAME_LIMIT",
                &format!("请求帧 {}B 超上限 {}B", body.len() + 4, h.frame_limit),
            );
        }
        if write_frame(&mut h.stdout, &frame).is_err() {
            eprintln!("ztw-host-py: 写 Game 请求失败");
            std::process::exit(3);
        }
        // 故障注入：提交后、回复前断连（仅第一代次，重启后跑“不重放”流程）。
        if h.faults.abort_after_send && rid == 1 && epoch == 1 {
            eprintln!("ztw-host-py: 故障注入 abort_after_send");
            std::process::abort();
        }
        let reply: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("ztw-host-py: 读回复失败：{e}");
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
            eprintln!("ztw-host-py: 期望 Reply 帧");
            std::process::exit(3);
        };
        // 对称校验：回复必须属于当前代次与执行，且回复号等于请求号。
        if request_id != rid || reply_epoch != epoch || reply_exec != exec_id {
            eprintln!(
                "ztw-host-py: 回复错位 rid {request_id}!={rid} / epoch {reply_epoch}!={epoch} / exec {reply_exec}!={exec_id}"
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
            eprintln!("ztw-host-py: 故障注入 abort_init_after_mem");
            std::process::abort();
        }
        if let Some(target) = &h.faults.abort_after_reply_op
            && op == *target
        {
            eprintln!("ztw-host-py: 故障注入 abort_after_reply:{target}");
            std::process::abort();
        }
        // 故障注入：同号同负载重发一次（触发主进程去重缓存路径）。
        if h.faults.dup_request_id == Some(rid) {
            eprintln!("ztw-host-py: 故障注入 dup_request:{rid}");
            if write_frame(&mut h.stdout, &frame).is_err() {
                eprintln!("ztw-host-py: 重发 Game 请求失败");
                std::process::exit(3);
            }
            let second: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("ztw-host-py: 读重发回复失败：{e}");
                    std::process::exit(3);
                }
            };
            let MainFrame::Reply {
                request_id: rid2,
                result: result2,
                ..
            } = second
            else {
                eprintln!("ztw-host-py: 期望重发的 Reply 帧");
                std::process::exit(3);
            };
            if rid2 != rid || result2 != result {
                eprintln!("ztw-host-py: 重发回复错位或不一致");
                std::process::exit(3);
            }
        }
        // 故障注入：同号异负载重发（预期主进程终止宿主；读到 EOF 走退出路径）。
        if h.faults.dup_request_corrupt_id == Some(rid) {
            eprintln!("ztw-host-py: 故障注入 dup_request_corrupt:{rid}");
            let mut dup = frame.clone();
            if let HostFrame::Request { payload, .. } = &mut dup {
                payload.insert(0, ' '); // 指纹不同，负载仍可解析
            }
            if write_frame(&mut h.stdout, &dup).is_err() {
                std::process::exit(3);
            }
            match read_frame::<_, MainFrame>(&mut std::io::stdin(), HOST_READ_LIMIT) {
                Ok(_) => {
                    // 主进程未拒绝 = 协议语义破坏；退出让测试失败得显式。
                    eprintln!("ztw-host-py: 同号异负载未被主进程拒绝");
                    std::process::exit(3);
                }
                Err(_) => std::process::exit(3),
            }
        }
        result
    })
}

/// 微秒时钟：仅供绑定层量测计时，不构成游戏语义。
#[pyfunction]
fn __now_us() -> f64 {
    use std::sync::LazyLock;
    use std::time::Instant;
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_micros() as f64
}

/// 注册原生桥模块 __ztw（bootstrap.py 经 `import __ztw` 使用；sys.modules
/// 命中绕过 meta_path，不受白名单约束）。
fn register_bridge(py: Python<'_>) -> PyResult<()> {
    let m = PyModule::new(py, "__ztw")?;
    m.add_function(wrap_pyfunction!(__ipc, &m)?)?;
    m.add_function(wrap_pyfunction!(__now_us, &m)?)?;
    let sys = PyModule::import(py, "sys")?;
    let modules = sys.getattr("modules")?;
    modules.cast::<PyDict>()?.set_item("__ztw", &m)?;
    Ok(())
}

/// 玩家命名空间的 __builtins__ 构造函数（收窄时捕获，见 main 引导）。
static NEW_BUILTINS: OnceLock<Py<PyAny>> = OnceLock::new();

// ---------------------------------------------------------------------------
// 故障分类
// ---------------------------------------------------------------------------

struct FaultOut {
    /// "script" | "environment" | "protocol"。
    class: &'static str,
    code: String,
    message: String,
    stack: String,
}

/// 手动走栈帧（不经 traceback/linecache——后者会触发 audit hook 的 open
/// 拒绝）。行号 + 函数名足够定位；绑定层帧过滤掉。
fn format_stack(tb: Option<Bound<'_, PyTraceback>>) -> String {
    let mut out = String::new();
    let mut cur = tb;
    while let Some(t) = cur {
        cur = (|| -> Option<Bound<'_, PyTraceback>> {
            let frame = t.getattr("tb_frame").ok()?;
            let code = frame.getattr("f_code").ok()?;
            let filename: String = code.getattr("co_filename").ok()?.extract().ok()?;
            if filename != BOOTSTRAP_FILENAME {
                let name: String = code
                    .getattr("co_name")
                    .ok()?
                    .extract()
                    .unwrap_or_else(|_| "<unknown>".into());
                let lineno: i64 = t.getattr("tb_lineno").ok()?.extract().ok()?;
                out.push_str(&format!(
                    "  File \"{filename}\", line {lineno}, in {name}\n"
                ));
            }
            t.getattr("tb_next").ok()?.extract().ok()
        })();
    }
    out
}

/// 异常分类：脚本级错误，code = 异常类型名；中断与内存超限按互证标志
/// 归类（玩家手动 raise 无标志，按普通脚本错误——JS 侧同款裁决）。
fn classify(py: Python<'_>, e: &PyErr, interrupt_fired: &AtomicBool) -> FaultOut {
    let value = e.value(py);
    let type_name = value
        .get_type()
        .name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let stack = format_stack(e.traceback(py));
    if type_name == "KeyboardInterrupt" && interrupt_fired.load(Ordering::SeqCst) {
        return FaultOut {
            class: "script",
            code: "INTERRUPTED".into(),
            message:
                "执行超预算，运行时内中断生效（KeyboardInterrupt 可被捕获，反复吞掉将由主进程终止兜底）"
                    .into(),
            stack,
        };
    }
    if type_name == "MemoryError" && py_alloc::STATE.limit_hit.load(Ordering::SeqCst) {
        let total = py_alloc::STATE.total.load(Ordering::SeqCst);
        let peak = py_alloc::STATE.peak.load(Ordering::SeqCst);
        let limit = py_alloc::STATE.limit_bytes();
        return FaultOut {
            class: "script",
            code: "MEMORY_LIMIT".into(),
            message: format!(
                "Python 内存超限（配额分配器拒绝；记账 {total}/上限 {limit}，历史峰值 {peak}）——\
                 脚本级错误：热重载重建命名空间后恢复"
            ),
            stack,
        };
    }
    let code = if type_name.is_empty() {
        "SCRIPT_ERROR".to_string()
    } else {
        type_name
    };
    FaultOut {
        class: "script",
        code,
        message: e.to_string(),
        stack,
    }
}

// ---------------------------------------------------------------------------
// 主循环
// ---------------------------------------------------------------------------

/// 跨执行保留的执行环境：玩家命名空间 + 绑定层句柄。
struct ExecCtx {
    ns: Option<Py<PyDict>>,
    set_mirror: Option<Py<PyAny>>,
    stats_fn: Option<Py<PyAny>>,
    /// 首次 loop 前的注入只做一次（hang/oversize/skip_delta 语义）。
    first_loop_injected: bool,
}

fn main() {
    let mut heap_limit = 512 * 1024 * 1024usize;
    let mut stack_limit = 1024 * 1024usize;
    let mut frame_limit = 1024 * 1024usize;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let val = args.next().unwrap_or_default();
        match flag.as_str() {
            "--heap-limit" => heap_limit = val.parse().unwrap_or(heap_limit),
            "--stack-limit" => stack_limit = val.parse().unwrap_or(stack_limit),
            "--frame-limit" => frame_limit = val.parse().unwrap_or(frame_limit),
            _ => {}
        }
    }
    // CPython 递归深度由解释器自管，无独立栈限额；接受参数保持与
    // ztw-host-js 的进程接口对齐。
    let _ = stack_limit;
    if heap_limit == 0 {
        eprintln!("ztw-host-py: --heap-limit 0 无效（省略该参数使用默认 512MiB）");
        std::process::exit(2);
    }

    // 标准库定位：发行物前缀在构建期烧入。PYTHONHOME 必须在解释器初始化
    // 前设置。此时进程单线程，set_var 的并发约束不构成风险。
    if let Some(home) = option_env!("ZTW_PY_HOME")
        && !home.is_empty()
    {
        unsafe { std::env::set_var("PYTHONHOME", home) };
    }

    // 配额分配器必须在解释器初始化前安装（首次 attach 触发初始化）。
    py_alloc::install(heap_limit as u64);
    let faults = parse_faults(std::env::var("ZTW_FAULT").ok());
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        h.faults = faults;
        h.frame_limit = frame_limit;
    });

    // 解释器就绪 + 引导（信号处理器 / 原生桥 / 能力收窄）。
    Python::attach(|py| {
        // pyo3 的 auto-initialize 走 Py_InitializeEx(0)：不安装信号处理器。
        // PyErr_SetInterruptEx 只在信号存在 Python 级处理器（非 SIG_DFL /
        // SIG_IGN）时才注入（Modules/signalmodule.c）——导入 _signal 注册
        // SIGINT 的 default_int_handler，看门狗注入才有效。
        py.import("_signal")
            .expect("导入 _signal 注册 SIGINT 处理器");
        register_bridge(py).expect("注册 __ztw 桥");
        let new_builtins = narrow::narrow(py).expect("能力收窄");
        NEW_BUILTINS.set(new_builtins).expect("收窄只执行一次");
    });

    // 运行时内中断：deadline + 注入标志（与 ztw-host-js 相同的语义）。
    let deadline = Arc::new(AtomicU64::new(u64::MAX));
    let interrupt_fired = Arc::new(AtomicBool::new(false));
    spawn_interrupt_watchdog(deadline.clone(), interrupt_fired.clone());

    let mut ctx = ExecCtx {
        ns: None,
        set_mirror: None,
        stats_fn: None,
        first_loop_injected: false,
    };

    loop {
        let frame: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("ztw-host-py: 读执行请求失败：{e}");
                break;
            }
        };
        let MainFrame::Exec {
            v,
            host_epoch,
            execution_id,
            kind,
            budget_ms,
            source,
            mirror,
            memory_gen,
            ..
        } = frame
        else {
            eprintln!("ztw-host-py: 顶层只接受 Exec 帧");
            std::process::exit(3);
        };
        if v != ztw_api::protocol::PROTOCOL_VERSION {
            eprintln!(
                "ztw-host-py: 协议版本 {v} != {}",
                ztw_api::protocol::PROTOCOL_VERSION
            );
            std::process::exit(3);
        }
        if kind != "init" && kind != "loop" {
            eprintln!("ztw-host-py: 未知执行类型 {kind}");
            std::process::exit(3);
        }
        let is_init = kind == "init";
        HOST.with(|h| {
            let mut h = h.borrow_mut();
            h.in_init = is_init;
            h.stats = ExecStats::default();
            h.host_epoch = host_epoch;
            h.execution_id = execution_id;
        });
        // 旧命名空间的释放只走 run_exec 内、GIL 持有下的 drop + GC——
        // 主循环这里不得预先置 None（GIL 外 drop 走 pyo3 延迟 decref 队列，
        // 释放时机依赖 attach 入口冲刷的实现细节，不可依赖）。
        // 本执行预算与互证标志复位（分配器计数不回退——命名空间可能仍
        // 持有上一执行的分配）。
        deadline.store(now_ms().saturating_add(budget_ms), Ordering::Relaxed);
        interrupt_fired.store(false, Ordering::SeqCst);
        py_alloc::STATE.reset_for_exec();

        let outcome = Python::attach(|py| {
            let fired = interrupt_fired.clone();
            run_exec(
                py,
                &mut ctx,
                is_init,
                source.as_deref(),
                mirror.as_deref(),
                memory_gen,
                &fired,
            )
        });
        // 恢复“无限”deadline，避免执行间隙误注入。
        deadline.store(u64::MAX, Ordering::Relaxed);

        // 收割绑定层的增量回放耗时（量测）。
        if let Some(stats_fn) = ctx.stats_fn.as_ref() {
            let delta_us: u64 = Python::attach(|py| {
                stats_fn
                    .bind(py)
                    .call0()
                    .ok()
                    .and_then(|d| d.get_item("delta_us").ok())
                    .and_then(|v| v.extract().ok())
                    .unwrap_or(0)
            });
            HOST.with(|h| h.borrow_mut().stats.mirror_apply_us = delta_us);
        }

        let last_request_id = HOST.with(|h| h.borrow().request_id);
        let stats = HOST.with(|h| h.borrow().stats.clone());
        match outcome {
            Ok(has_loop) => {
                let frame = HostFrame::Complete {
                    v: ztw_api::protocol::PROTOCOL_VERSION,
                    host_epoch,
                    execution_id,
                    last_request_id,
                    has_loop,
                    stats,
                };
                let sent = write_frame(&mut std::io::stdout(), &frame).is_ok();
                // 故障注入：完成帧连发（第二次应被主进程按旧执行丢弃）。
                let dup = HOST.with(|h| h.borrow().faults.dup_complete);
                if dup && sent {
                    let _ = write_frame(&mut std::io::stdout(), &frame);
                }
                if !sent {
                    break;
                }
            }
            Err(out) => {
                let protocol_broken = out.class == "protocol";
                let frame = HostFrame::Fault {
                    v: ztw_api::protocol::PROTOCOL_VERSION,
                    host_epoch,
                    execution_id,
                    class: out.class.to_string(),
                    code: out.code,
                    message: out.message,
                    stack: out.stack,
                    last_request_id,
                    stats,
                };
                let sent = write_frame(&mut std::io::stdout(), &frame).is_ok();
                if protocol_broken || !sent {
                    std::process::exit(3);
                }
            }
        }
    }
}

/// 宿主内看门狗（第二层）：deadline 到期后在宽限期内每 50ms 注入一次
/// KeyboardInterrupt；第三层（主进程）超宽限期终止宿主，二者互为兜底。
fn spawn_interrupt_watchdog(deadline: Arc<AtomicU64>, fired: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("ztw-py-watchdog".into())
        .spawn(move || {
            loop {
                let d = deadline.load(Ordering::Relaxed);
                if d == u64::MAX {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                let now = now_ms();
                if now >= d {
                    // 解释器已初始化（main 在 spawn 前完成引导），可安全注入。
                    fired.store(true, Ordering::SeqCst);
                    unsafe { pyo3::ffi::PyErr_SetInterruptEx(SIGINT) };
                    std::thread::sleep(Duration::from_millis(50));
                } else {
                    std::thread::sleep(Duration::from_millis((d - now).clamp(1, 20)));
                }
            }
        })
        .expect("spawn interrupt watchdog");
}

/// 直接以给定命名空间执行编译产物：pyo3 的 `PyCode::run` 无条件
/// `PyImport_AddModule("__main__")` 兜底（pyo3 code.rs 实现），会把收窄时
/// 从 sys.modules 洗除的 `__main__` 重建回来（玩家 `import __main__` 即
/// 可达真 builtins 字典）。这里绕过该实现细节；`__builtins__` 由调用方
/// 显式注入命名空间，不依赖兜底。
fn eval_in<'py>(
    py: Python<'py>,
    code: &pyo3::Bound<'py, PyCode>,
    ns: &pyo3::Bound<'py, PyDict>,
) -> pyo3::PyResult<pyo3::Bound<'py, PyAny>> {
    let r = unsafe { pyo3::ffi::PyEval_EvalCode(code.as_ptr(), ns.as_ptr(), ns.as_ptr()) };
    // 所有权契约：PyEval_EvalCode 返回新引用（异常时 NULL + 置错误）。
    unsafe { pyo3::Bound::from_owned_ptr_or_err(py, r) }
}

/// 单次执行：init = 全新命名空间（白名单 builtins → 绑定层 → 玩家源码）；
/// loop = 调用入口函数。每次执行前注入全量镜像。
fn run_exec(
    py: Python<'_>,
    ctx: &mut ExecCtx,
    is_init: bool,
    source: Option<&str>,
    mirror: Option<&str>,
    memory_gen: u64,
    interrupt_fired: &AtomicBool,
) -> Result<bool, FaultOut> {
    if is_init {
        // 先释放旧命名空间（含玩家数据）再建新环境：内存超限后的热重载
        // 需要配额空间，drop 必须发生在 GIL 内（GIL 外 drop 走延迟回收，
        // 时机不可控）；显式 GC 收引用环。
        ctx.ns = None;
        ctx.set_mirror = None;
        ctx.stats_fn = None;
        unsafe { pyo3::ffi::PyGC_Collect() };
        let Some(new_builtins) = NEW_BUILTINS.get() else {
            return Err(FaultOut {
                class: "environment",
                code: "BOOTSTRAP_FAILED".into(),
                message: "收窄未完成".into(),
                stack: String::new(),
            });
        };
        let builtins = new_builtins
            .call(py, (), None)
            .map_err(|e| classify(py, &e, interrupt_fired))?;
        // 绑定层执行进独立引导命名空间：内部符号（M/GEN/_ipc/_apply_delta
        // 等）不进入玩家命名空间——JS 侧 IIFE 封装的同款语义；玩家侧只
        // 发布公开表面 Game / Position / GameError（docs 04）。
        let boot_ns = PyDict::new(py);
        boot_ns
            .set_item("__builtins__", builtins.clone_ref(py))
            .map_err(|e| classify(py, &e, interrupt_fired))?;
        boot_ns
            .set_item("__name__", "__ztw_bootstrap__")
            .map_err(|e| classify(py, &e, interrupt_fired))?;
        // 绑定层执行（属宿主引导，任何失败都是环境级）。
        let bootstrap = CString::new(BOOTSTRAP_PY).expect("bootstrap 无 NUL");
        let bootstrap_name = CString::new(BOOTSTRAP_FILENAME).unwrap();
        let bootstrap_run = PyCode::compile(py, &bootstrap, &bootstrap_name, PyCodeInput::File)
            .and_then(|c| eval_in(py, &c, &boot_ns).map(|_| ()));
        if let Err(e) = bootstrap_run {
            return Err(FaultOut {
                class: "environment",
                code: "BOOTSTRAP_FAILED".into(),
                message: format!("绑定层初始化失败：{e}"),
                stack: String::new(),
            });
        }
        let boot_missing = |what: &str| FaultOut {
            class: "environment",
            code: "BOOTSTRAP_FAILED".into(),
            message: format!("绑定层缺少导出 {what}"),
            stack: String::new(),
        };
        ctx.set_mirror = Some(
            boot_ns
                .get_item("_ztw_set_mirror")
                .ok()
                .flatten()
                .ok_or_else(|| boot_missing("_ztw_set_mirror"))?
                .unbind(),
        );
        ctx.stats_fn = Some(
            boot_ns
                .get_item("_ztw_stats")
                .ok()
                .flatten()
                .ok_or_else(|| boot_missing("_ztw_stats"))?
                .unbind(),
        );
        let publish = |key: &str| {
            boot_ns
                .get_item(key)
                .ok()
                .flatten()
                .ok_or_else(|| boot_missing(key))
        };
        let game = publish("Game")?;
        let position = publish("Position")?;
        let game_error = publish("GameError")?;
        let ns = PyDict::new(py);
        ns.set_item("__builtins__", builtins)
            .map_err(|e| classify(py, &e, interrupt_fired))?;
        ns.set_item("__name__", "__ztw_player__")
            .map_err(|e| classify(py, &e, interrupt_fired))?;
        for (k, v) in [
            ("Game", game),
            ("Position", position),
            ("GameError", game_error),
        ] {
            ns.set_item(k, v)
                .map_err(|e| classify(py, &e, interrupt_fired))?;
        }
        ctx.ns = Some(ns.unbind());
    } else if ctx.ns.is_none() {
        return Err(FaultOut {
            class: "protocol",
            code: "NO_NAMESPACE".into(),
            message: "无玩家命名空间，需要 init 执行重建".into(),
            stack: String::new(),
        });
    }

    // 注入全量镜像与 memory 代次（init 与每 loop 均携带）。
    let set_mirror = ctx.set_mirror.as_ref().expect("引导已完成");
    let t0 = std::time::Instant::now();
    set_mirror
        .bind(py)
        .call1((mirror.unwrap_or("{}"), memory_gen))
        .map_err(|e| classify(py, &e, interrupt_fired))?;
    let mirror_us = t0.elapsed().as_micros() as u64;
    HOST.with(|h| h.borrow_mut().stats.mirror_parse_us = mirror_us);

    if is_init {
        // 玩家源码整段执行（编译名 <player>）。
        let source = source.unwrap_or_default();
        let ns = ctx.ns.as_ref().expect("刚建立").bind(py);
        let c_src = match CString::new(source) {
            Ok(c) => c,
            Err(_) => {
                return Err(FaultOut {
                    class: "script",
                    code: "SCRIPT_ERROR".into(),
                    message: "源码包含 NUL 字节".into(),
                    stack: String::new(),
                });
            }
        };
        let c_name = CString::new(PLAYER_FILENAME).expect("固定文件名");
        let code = match PyCode::compile(py, &c_src, &c_name, PyCodeInput::File) {
            Ok(c) => c,
            Err(e) => return Err(classify(py, &e, interrupt_fired)),
        };
        if let Err(e) = eval_in(py, &code, ns) {
            return Err(classify(py, &e, interrupt_fired));
        }
        let has_loop = ns
            .get_item("loop")
            .ok()
            .flatten()
            .is_some_and(|v| v.is_callable());
        Ok(has_loop)
    } else {
        if !ctx.first_loop_injected {
            ctx.first_loop_injected = true;
            inject_loop_faults(py, ctx);
        }
        let ns = ctx.ns.as_ref().expect("引导已完成").bind(py);
        let entry = ns.get_item("loop").ok().flatten();
        let Some(entry) = entry.filter(|v| v.is_callable()) else {
            return Err(FaultOut {
                class: "script",
                code: "ENTRY_MISSING".into(),
                message: "入口 loop 不存在或不可调用（被玩家代码覆写？）".into(),
                stack: String::new(),
            });
        };
        match entry.call0() {
            Ok(_) => Ok(true),
            Err(e) => Err(classify(py, &e, interrupt_fired)),
        }
    }
}

/// 首次 loop 执行前的故障注入（oversize_frame / hang_exec /
/// skip_delta_replay——时机与 JS 宿主一致）。
fn inject_loop_faults(py: Python<'_>, ctx: &ExecCtx) {
    use std::io::Write;
    let (oversize, hang, skip_delta) = HOST.with(|h| {
        let h = h.borrow();
        (
            h.faults.oversize_frame,
            h.faults.hang_exec,
            h.faults.skip_delta_replay,
        )
    });
    if skip_delta {
        // 注入语义：take 的镜像增量被丢弃（回放失败路径），下一次查询经
        // mirror.fetch 整体重建。
        if let Some(ns) = ctx.ns.as_ref() {
            let _ = ns.bind(py).set_item("_DROP_DELTAS", true);
        }
    }
    if oversize {
        eprintln!("ztw-host-py: 故障注入 oversize_frame");
        let mut out = std::io::stdout();
        let _ = out.write_all(&0x7FFF_F000u32.to_be_bytes());
        let _ = out.write_all(&[b'x'; 4096]);
        let _ = out.flush();
        std::thread::sleep(Duration::from_secs(3600));
    }
    if hang {
        eprintln!("ztw-host-py: 故障注入 hang_exec");
        std::thread::sleep(Duration::from_secs(3600));
    }
}
