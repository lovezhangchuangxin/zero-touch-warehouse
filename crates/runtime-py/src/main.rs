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
//! 故障注入（ZTW_FAULT）：框架、旋钮清单与 ipc 主体在 ztw_api::host_ipc
//!（两宿主单源，旋钮清单见其模块文档）；本侧特有的只有 skip_delta_replay
//! 的置位机制（经绑定层导出的 setter，见 inject_loop_faults）。行为
//! 等价性对账：crates/runtime-py/tests/a1_fault_parity.rs。

mod narrow;
mod py_alloc;

use std::ffi::CString;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use pyo3::prelude::*;
use pyo3::types::{PyCode, PyCodeInput, PyDict, PyDictMethods, PyModule, PyTraceback};
use ztw_api::BOOTSTRAP_PY;
use ztw_api::host_ipc::{
    HOST_READ_LIMIT, begin_exec, configure, inject_oversize_or_hang, now_ms, parse_faults,
    with_host,
};
use ztw_api::protocol::{HostFrame, MainFrame, read_frame, resolve_program, write_frame};

use py_alloc::SIGINT;

/// 玩家源码的编译名：堆栈定位用，不落盘。
const PLAYER_FILENAME: &str = "<player>";
/// 绑定层的编译名（堆栈帧过滤用）。
const BOOTSTRAP_FILENAME: &str = "<bootstrap>";
/// Python 程序的默认入口文件名（协议 entry 字段可覆盖）。
const DEFAULT_ENTRY: &str = "main.py";

// ---------------------------------------------------------------------------
// 原生桥 __ztw（__ipc / __now_us）
// ---------------------------------------------------------------------------

/// Python 侧同步 IPC 入口：发请求、阻塞等回复。协议破坏一律退出（exit 3）。
/// 主体在 [`ztw_api::host_ipc::ipc_roundtrip`]（两宿主单源）；包装层只做
/// pyfunction 签名适配。
#[pyfunction]
fn __ipc(op: String, payload: String) -> String {
    ztw_api::host_ipc::ipc_roundtrip(op, payload)
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

/// 玩家模块注册表（收窄期创建的 _ZtwPlayerRegistry 实例）：每次 init
/// 调 update 更新文件集（模块名 → 源码）与绑定注入，并驱逐旧玩家模块。
static PLAYER_REGISTRY: OnceLock<Py<PyAny>> = OnceLock::new();

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
    /// 故障注入钩子：置位 _DROP_DELTAS（经绑定层 setter，跨命名空间）。
    drop_deltas_fn: Option<Py<PyAny>>,
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
    configure(
        parse_faults(std::env::var("ZTW_FAULT").ok()),
        frame_limit,
        "ztw-host-py",
    );

    // 解释器就绪 + 引导（信号处理器 / 原生桥 / 能力收窄）。
    Python::attach(|py| {
        // pyo3 的 auto-initialize 走 Py_InitializeEx(0)：不安装信号处理器。
        // PyErr_SetInterruptEx 只在信号存在 Python 级处理器（非 SIG_DFL /
        // SIG_IGN）时才注入（Modules/signalmodule.c）——导入 _signal 注册
        // SIGINT 的 default_int_handler，看门狗注入才有效。
        py.import("_signal")
            .expect("导入 _signal 注册 SIGINT 处理器");
        register_bridge(py).expect("注册 __ztw 桥");
        let (new_builtins, player_registry) = narrow::narrow(py).expect("能力收窄");
        NEW_BUILTINS.set(new_builtins).expect("收窄只执行一次");
        PLAYER_REGISTRY
            .set(player_registry)
            .expect("收窄只执行一次");
    });

    // 运行时内中断：deadline + 注入标志（与 ztw-host-js 相同的语义）。
    let deadline = Arc::new(AtomicU64::new(u64::MAX));
    let interrupt_fired = Arc::new(AtomicBool::new(false));
    spawn_interrupt_watchdog(deadline.clone(), interrupt_fired.clone());

    let mut ctx = ExecCtx {
        ns: None,
        set_mirror: None,
        stats_fn: None,
        drop_deltas_fn: None,
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
            files,
            entry,
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
        begin_exec(is_init, host_epoch, execution_id);
        // 旧命名空间的释放只走 run_exec 内、GIL 持有下的 drop + GC——
        // 主循环这里不得预先置 None（GIL 外 drop 走 pyo3 延迟 decref 队列，
        // 释放时机依赖 attach 入口冲刷的实现细节，不可依赖）。
        // 本执行预算与互证标志复位（分配器计数不回退——命名空间可能仍
        // 持有上一执行的分配）。
        deadline.store(now_ms().saturating_add(budget_ms), Ordering::Relaxed);
        interrupt_fired.store(false, Ordering::SeqCst);
        py_alloc::STATE.reset_for_exec();

        let mut program: Option<ztw_api::protocol::ResolvedProgram> = None;
        let mut program_invalid: Option<FaultOut> = None;
        if is_init {
            match resolve_program(files, entry, DEFAULT_ENTRY) {
                Ok(Some(p)) => program = Some(p),
                Ok(None) => {
                    // init 帧缺文件集：与 JS 宿主同口径，按脚本级
                    // PROGRAM_INVALID 上报（harness 不会发出这种帧，
                    // 统一分类只为未来第三方主进程接入时行为不分叉）。
                    program_invalid = Some(FaultOut {
                        class: "script",
                        code: "PROGRAM_INVALID".into(),
                        message: "init 帧缺少文件集".into(),
                        stack: String::new(),
                    });
                }
                Err(message) => {
                    program_invalid = Some(FaultOut {
                        class: "script",
                        code: "PROGRAM_INVALID".into(),
                        message,
                        stack: String::new(),
                    });
                }
            }
        }

        let outcome = if let Some(fault) = program_invalid {
            Err(fault)
        } else {
            Python::attach(|py| {
                let fired = interrupt_fired.clone();
                run_exec(
                    py,
                    &mut ctx,
                    is_init,
                    program.as_ref(),
                    mirror.as_ref().map(|m| m.get()),
                    memory_gen,
                    &fired,
                )
            })
        };
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
            with_host(|h| h.stats.mirror_apply_us = delta_us);
        }

        let last_request_id = with_host(|h| h.request_id);
        let stats = with_host(|h| h.stats.clone());
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
                // stderr 标记是 a1_fault_parity 探针的注入生效证据——
                // 连发被静默丢失时主进程无副作用可观测。
                let (dup, tag) = with_host(|h| (h.faults.dup_complete, h.host_tag));
                if dup && sent {
                    eprintln!("{tag}: 故障注入 dup_complete");
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
    program: Option<&ztw_api::protocol::ResolvedProgram>,
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
        ctx.drop_deltas_fn = None;
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
        ctx.drop_deltas_fn = Some(
            boot_ns
                .get_item("_ztw_set_drop_deltas")
                .ok()
                .flatten()
                .ok_or_else(|| boot_missing("_ztw_set_drop_deltas"))?
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
    with_host(|h| h.stats.mirror_parse_us = mirror_us);

    if is_init {
        // 玩家模块注册表更新：文件名校验折算模块名（lib.py → lib）、
        // 驱逐旧玩家模块（解释器常驻，sys.modules 不得跨热重载存活）、
        // 注入本代绑定（Game 等）供玩家模块使用。
        let (files, entry) = program.expect("init 已解析出程序");
        let module_names = py_module_names(files)?;
        let registry = PLAYER_REGISTRY.get().ok_or_else(|| FaultOut {
            class: "environment",
            code: "BOOTSTRAP_FAILED".into(),
            message: "玩家模块注册表未初始化（收窄未完成）".into(),
            stack: String::new(),
        })?;
        let files_py = pyo3::types::PyDict::new(py);
        for (k, v) in &module_names {
            files_py
                .set_item(k, v)
                .map_err(|e| registry_fault(py, e, interrupt_fired))?;
        }
        let bindings_py = pyo3::types::PyDict::new(py);
        for key in ["Game", "Position", "GameError"] {
            if let Some(v) = ctx
                .ns
                .as_ref()
                .expect("刚建立")
                .bind(py)
                .get_item(key)
                .ok()
                .flatten()
            {
                bindings_py
                    .set_item(key, v)
                    .map_err(|e| registry_fault(py, e, interrupt_fired))?;
            }
        }
        registry
            .bind(py)
            .call_method1("update", (files_py, bindings_py))
            .map_err(|e| registry_fault(py, e, interrupt_fired))?;

        // 玩家入口整段执行（编译名 <player>/<entry>）。
        let source = files.get(entry).expect("resolve_program 已校验入口存在");
        let ns = ctx.ns.as_ref().expect("刚建立").bind(py);
        let c_src = match CString::new(source.as_str()) {
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
        let c_name = CString::new(format!("{PLAYER_FILENAME}/{entry}")).expect("入口名无 NUL");
        let code = match PyCode::compile(py, &c_src, &c_name, PyCodeInput::File) {
            Ok(c) => c,
            Err(e) => return Err(classify(py, &e, interrupt_fired)),
        };
        if let Err(e) = eval_in(py, &code, ns) {
            return Err(classify(py, &e, interrupt_fired));
        }
        let entry_fn = ns.get_item("loop").ok().flatten();
        let has_loop = entry_fn.as_ref().is_some_and(|v| v.is_callable());
        if has_loop {
            let f = entry_fn.expect("is_some_and 刚确认");
            if entry_is_async(py, &f)? {
                return Err(async_entry_fault());
            }
        }
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
        if entry_is_async(py, &entry)? {
            return Err(async_entry_fault());
        }
        match entry.call0() {
            Ok(_) => Ok(true),
            Err(e) => Err(classify(py, &e, interrupt_fired)),
        }
    }
}

/// Python 玩家文件名校验并折算模块名：仅根目录下的 `.py` 文件
/// （`lib.py` → 模块名 `lib`），模块名不得与标准库白名单/保留名冲突
///（sys.modules 预载的白名单名玩家侧不可达，未预载的会被 finder 遮蔽
/// 造成语义混淆，一律在 init 拦下给可读错误）。
fn py_module_names(
    files: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, String>, FaultOut> {
    let invalid = |message: String| FaultOut {
        class: "script",
        code: "PROGRAM_INVALID".into(),
        message,
        stack: String::new(),
    };
    let mut out = std::collections::BTreeMap::new();
    for (name, src) in files {
        let stem = name
            .strip_suffix(".py")
            .filter(|s| {
                !s.is_empty()
                    && !s.contains('/')
                    && !s.contains('\\')
                    && !s.contains('.')
                    && !s.contains('\0')
            })
            .ok_or_else(|| {
                invalid(format!(
                    "玩家文件 {name:?} 不合法：Python 侧仅支持根目录下的 .py 文件（如 lib.py → import lib）"
                ))
            })?;
        if narrow::WHITELIST.contains(&stem)
            || narrow::TRANSITIVE.contains(&stem)
            || narrow::KEEP_PRIVATE.contains(&stem)
            || matches!(
                stem,
                "builtins" | "__ztw" | "__main__" | "sys" | "_signal" | "importlib" | "inspect"
            )
        {
            return Err(invalid(format!(
                "玩家文件 {name:?} 的模块名 {stem:?} 与标准库/保留名冲突，请改名"
            )));
        }
        // 模块名还必须是合法 Python 标识符且非关键字：非标识符名
        //（如 my-lib.py / a b.py）注册后 `import` 语句写不出来，只能经
        // __import__ 触达，成死重量；错误在 init 给出并提示改名。
        if !is_python_identifier(stem) || PY_KEYWORDS.contains(&stem) {
            return Err(invalid(format!(
                "玩家文件 {name:?} 的模块名 {stem:?} 不是合法的 Python 标识符（或为关键字），无法 import，请改名"
            )));
        }
        out.insert(stem.to_string(), src.clone());
    }
    Ok(out)
}

/// Python 标识符近似校验（首字符字母/下划线，其余加数字；不引入完整
/// XID 表——覆盖常见形态即可，非 ASCII 字母按字母类放行）。
fn is_python_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// Python 关键字（与钉版 CPython grammar 对齐；soft keywords 的
/// match/case/_ 不是保留字，可作模块名，不列）。
const PY_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

/// 注册表操作属宿主引导机械：失败按环境级（与 bootstrap 失败同类）。
/// 两个例外对齐 classify 的玩家可诱发口径：预算内看门狗注入的
/// KeyboardInterrupt 归脚本级 INTERRUPTED；配额耗尽的 MemoryError 归
/// 脚本级 MEMORY_LIMIT——否则玩家可把脚本错误伪装成环境故障触发
/// 无谓的环境重建。
fn registry_fault(py: Python<'_>, e: pyo3::PyErr, interrupt_fired: &AtomicBool) -> FaultOut {
    if e.is_instance_of::<pyo3::exceptions::PyKeyboardInterrupt>(py)
        && interrupt_fired.load(Ordering::SeqCst)
    {
        return FaultOut {
            class: "script",
            code: "INTERRUPTED".into(),
            message: "执行超预算，运行时内中断生效（落在玩家模块注册表更新处）".into(),
            stack: String::new(),
        };
    }
    if e.is_instance_of::<pyo3::exceptions::PyMemoryError>(py)
        && py_alloc::STATE.limit_hit.load(Ordering::SeqCst)
    {
        return FaultOut {
            class: "script",
            code: "MEMORY_LIMIT".into(),
            message: "Python 内存超限（落在玩家模块注册表更新处）".into(),
            stack: String::new(),
        };
    }
    FaultOut {
        class: "environment",
        code: "BOOTSTRAP_FAILED".into(),
        message: format!("玩家模块注册表操作失败：{e}"),
        stack: String::new(),
    }
}

fn async_entry_fault() -> FaultOut {
    FaultOut {
        class: "script",
        code: "ASYNC_ENTRY".into(),
        message: "入口 loop 不能是 async 函数（沙箱为同步执行模型，协程不会被驱动）".into(),
        stack: String::new(),
    }
}

/// 入口 async 检测：经收窄期捕获的 inspect.iscoroutinefunction
///（注册表方法）。async def loop 会静默不执行函数体，必须在入口处
/// 拦下给可读错误（docs/game-design/05 异步边界）。
fn entry_is_async(py: Python<'_>, entry: &pyo3::Bound<'_, pyo3::PyAny>) -> Result<bool, FaultOut> {
    let registry = PLAYER_REGISTRY.get().ok_or_else(|| FaultOut {
        class: "environment",
        code: "BOOTSTRAP_FAILED".into(),
        message: "玩家模块注册表未初始化（收窄未完成）".into(),
        stack: String::new(),
    })?;
    // 检查失败按"非 async"放行：敌意可调用对象（覆写 __code__ /
    // partial.func 抛异常）随后在 entry 调用处必炸，真实异常交 classify
    // 按脚本级上报——不把玩家可诱发的错误伪装成环境故障。
    Ok(registry
        .bind(py)
        .call_method1("entry_is_async", (entry,))
        .and_then(|v| v.extract::<bool>())
        .unwrap_or(false))
}

/// 首次 loop 执行前的故障注入：skip_delta_replay 的置位机制是本侧特有
/// （经绑定层导出的 setter——命名空间隔离后 _DROP_DELTAS 是 boot_ns 的
/// 模块全局，往玩家 ns 写同名变量不会被 _apply_delta 看到，评审发现的
/// 静默失效）；oversize_frame / hang_exec 的传输层注入在 host_ipc 单源。
fn inject_loop_faults(py: Python<'_>, ctx: &ExecCtx) {
    let (oversize, hang, skip_delta) = with_host(|h| {
        (
            h.faults.oversize_frame,
            h.faults.hang_exec,
            h.faults.skip_delta_replay,
        )
    });
    if skip_delta {
        // 注入语义：take 的镜像增量被丢弃（回放失败路径），下一次查询经
        // mirror.fetch 整体重建。
        if let Some(f) = ctx.drop_deltas_fn.as_ref() {
            let _ = f.bind(py).call1((true,));
        }
    }
    inject_oversize_or_hang(oversize, hang);
}
