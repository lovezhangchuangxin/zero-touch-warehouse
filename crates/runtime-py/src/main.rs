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
//! - 本阶段（提交 3）：无 Game 绑定；绑定层与能力收窄随后续提交。
//!
//! 故障分类：语法/未捕获异常 → 脚本级（code=异常类型名）；中断与
//! MemoryError 以看门狗注入标志 / 分配器拒绝标志互证（防玩家伪造），
//! 未带标志的手动 raise 按普通脚本错误归类。

mod py_alloc;

use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use pyo3::prelude::*;
use pyo3::types::{PyCode, PyCodeInput, PyCodeMethods, PyDict, PyDictMethods};
use ztw_api::protocol::{ExecStats, HostFrame, MainFrame, read_frame, write_frame};

use py_alloc::SIGINT;

/// 宿主读取主进程帧的上限（与 ztw-host-js 一致）。
const HOST_READ_LIMIT: u64 = 64 * 1024 * 1024;

/// 玩家源码的编译名：堆栈与语法错误定位用，不落盘。
const PLAYER_FILENAME: &str = "<player>";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct FaultOut {
    /// "script"（除协议外全部）；"protocol" = 协议破坏，进程退出。
    class: &'static str,
    code: String,
    message: String,
    stack: String,
}

fn main() {
    let mut heap_limit = 512 * 1024 * 1024usize;
    let mut stack_limit = 1024 * 1024usize;
    let mut frame_limit = 1024 * 1024usize;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let val = args.next().unwrap_or_default();
        match flag.as_str() {
            // 参数面与 ztw-host-js 对齐（harness 统一下发）。
            "--heap-limit" => heap_limit = val.parse().unwrap_or(heap_limit),
            "--stack-limit" => stack_limit = val.parse().unwrap_or(stack_limit),
            "--frame-limit" => frame_limit = val.parse().unwrap_or(frame_limit),
            _ => {}
        }
    }
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

    // 配额分配器必须在解释器初始化前安装（首次 attach 触发初始化——
    // 此处先显式 warm up，之后的看门狗注入也依赖解释器已就绪）。
    py_alloc::install(heap_limit as u64);
    Python::attach(|py| {
        // pyo3 的 auto-initialize 走 Py_InitializeEx(0)：不安装信号处理器。
        // PyErr_SetInterruptEx 只在信号存在 Python 级处理器（非 SIG_DFL /
        // SIG_IGN）时才注入（Modules/signalmodule.c）——导入 _signal 注册
        // SIGINT 的 default_int_handler，看门狗注入才有效。
        py.import("_signal")
            .expect("导入 _signal 注册 SIGINT 处理器");
    });

    // 运行时内中断：deadline + 注入标志。deadline 语义与 ztw-host-js
    // 相同（主进程经 Exec.budget_ms 下发；执行间隙恢复“无限”）。
    let deadline = Arc::new(AtomicU64::new(u64::MAX));
    let interrupt_fired = Arc::new(AtomicBool::new(false));
    spawn_interrupt_watchdog(deadline.clone(), interrupt_fired.clone());

    // 玩家命名空间跨执行保留；init 重建、loop 复用、脚本级错误后保留。
    let mut namespace: Option<Py<PyDict>> = None;

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
        if is_init {
            namespace = None; // 初始化即重建命名空间（首次加载 / 热重载同路径）
        }
        // 本执行预算与互证标志复位（分配器计数不回退——命名空间可能仍
        // 持有上一执行的分配）。
        deadline.store(now_ms().saturating_add(budget_ms), Ordering::Relaxed);
        interrupt_fired.store(false, Ordering::SeqCst);
        py_alloc::STATE.reset_for_exec();

        // 单次执行在 attach 闭包内完成；init 成功时带回新命名空间。
        let (new_ns, outcome): (Option<Py<PyDict>>, Result<bool, FaultOut>) =
            Python::attach(|py| {
                let fired = interrupt_fired.clone();
                if is_init {
                    match init_exec(py, source.as_deref().unwrap_or_default(), &fired) {
                        Ok((ns, has_loop)) => (Some(ns), Ok(has_loop)),
                        Err(f) => (None, Err(f)),
                    }
                } else {
                    match &namespace {
                        Some(ns) => (None, loop_exec(py, ns, &fired)),
                        None => (
                            None,
                            Err(FaultOut {
                                class: "protocol",
                                code: "NO_NAMESPACE".into(),
                                message: "无玩家命名空间，需要 init 执行重建".into(),
                                stack: String::new(),
                            }),
                        ),
                    }
                }
            });
        // 恢复“无限”deadline，避免执行间隙误注入。
        deadline.store(u64::MAX, Ordering::Relaxed);

        if let Some(ns) = new_ns {
            namespace = Some(ns);
        }
        // 本阶段无 Game 请求；请求号随绑定层提交由桥接层计数。
        let last_request_id = 0u64;
        match outcome {
            Ok(has_loop) => {
                let frame = HostFrame::Complete {
                    v: ztw_api::protocol::PROTOCOL_VERSION,
                    host_epoch,
                    execution_id,
                    last_request_id,
                    has_loop,
                    stats: ExecStats::default(),
                };
                if write_frame(&mut std::io::stdout(), &frame).is_err() {
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
                    stats: ExecStats::default(),
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
                    // 解释器已初始化（main 在 spawn 前 warm up），可安全注入。
                    fired.store(true, Ordering::SeqCst);
                    unsafe { pyo3::ffi::PyErr_SetInterruptEx(SIGINT) };
                    std::thread::sleep(Duration::from_millis(50));
                } else {
                    let wait = (d - now).clamp(1, 20);
                    std::thread::sleep(Duration::from_millis(wait));
                }
            }
        })
        .expect("spawn interrupt watchdog");
}

/// 初始化执行：全新命名空间中整体执行玩家源码。
fn init_exec(
    py: Python<'_>,
    source: &str,
    interrupt_fired: &AtomicBool,
) -> Result<(Py<PyDict>, bool), FaultOut> {
    let ns = PyDict::new(py);
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
    if let Err(e) = code.run(Some(&ns), Some(&ns)) {
        return Err(classify(py, &e, interrupt_fired));
    }
    let has_loop = ns
        .get_item("loop")
        .ok()
        .flatten()
        .is_some_and(|v| v.is_callable());
    Ok((ns.unbind(), has_loop))
}

/// loop 执行：调用入口函数。命名空间与全局变量复用（脚本级错误后保留）。
fn loop_exec(
    py: Python<'_>,
    ns: &Py<PyDict>,
    interrupt_fired: &AtomicBool,
) -> Result<bool, FaultOut> {
    let ns = ns.bind(py);
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

/// 异常分类：脚本级错误，code = 异常类型名；中断与内存超限按互证标志
/// 归类（玩家手动 raise 无标志，按普通脚本错误——JS 侧同款裁决）。
fn classify(py: Python<'_>, e: &PyErr, interrupt_fired: &AtomicBool) -> FaultOut {
    let value = e.value(py);
    let type_name = value
        .get_type()
        .name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if type_name == "KeyboardInterrupt" && interrupt_fired.load(Ordering::SeqCst) {
        return FaultOut {
            class: "script",
            code: "INTERRUPTED".into(),
            message:
                "执行超预算，运行时内中断生效（KeyboardInterrupt 可被捕获，反复吞掉将由主进程终止兜底）"
                    .into(),
            stack: String::new(),
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
            stack: String::new(),
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
        stack: String::new(),
    }
}
