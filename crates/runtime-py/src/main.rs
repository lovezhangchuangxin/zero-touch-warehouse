//! A1 Python 宿主进程（docs/architecture/03「Python 宿主」）。
//!
//! - 内嵌 CPython（PyO3 auto-initialize + vendored python-build-standalone，
//!   版本随构建锁定）；解释器驻留本进程与调用线程（main），生命周期内
//!   不重建；热重载重建的是玩家命名空间（全新 dict 重新执行程序）。
//! - stdio 长度前缀 JSON 协议与 ztw-host-js 同构（v2 会话头：host_epoch /
//!   execution_id / request_id 随帧回显与校验）。
//! - 本阶段（A1 提交 2）为空壳：init 在全新命名空间执行源码、loop 调用
//!   入口函数；Game 绑定、资源限制与能力收窄在后续提交补全。
//!
//! 故障分类（本阶段最小集）：语法错误 / 未捕获异常 → 脚本级（宿主存活）；
//! 协议错误 → 故障帧 + 退出。中断（KeyboardInterrupt）与 MemoryError 的
//! 互证分类随资源限制提交落地。

use std::ffi::CString;

use pyo3::prelude::*;
use pyo3::types::{PyCode, PyCodeInput, PyCodeMethods, PyDict, PyDictMethods};
use ztw_api::protocol::{ExecStats, HostFrame, MainFrame, read_frame, write_frame};

/// 宿主读取主进程帧的上限（与 ztw-host-js 一致）。
const HOST_READ_LIMIT: u64 = 64 * 1024 * 1024;

/// 玩家源码的编译名：堆栈与语法错误定位用，不落盘。
const PLAYER_FILENAME: &str = "<player>";

struct FaultOut {
    /// "script"（本阶段全部）；"environment" / "protocol" 随后续提交引入。
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
            // 参数面与 ztw-host-js 对齐（harness 统一下发）；本阶段仅
            // 存储，执行手段随资源限制提交落地。
            "--heap-limit" => heap_limit = val.parse().unwrap_or(heap_limit),
            "--stack-limit" => stack_limit = val.parse().unwrap_or(stack_limit),
            "--frame-limit" => frame_limit = val.parse().unwrap_or(frame_limit),
            _ => {}
        }
    }
    let _ = (heap_limit, stack_limit, frame_limit);

    // 标准库定位：发行物前缀在构建期烧入。PYTHONHOME 必须在解释器初始化
    // 前（auto-initialize 在首次 attach 时初始化）设置。此时进程单线程，
    // set_var 的并发约束不构成风险。
    if let Some(home) = option_env!("ZTW_PY_HOME")
        && !home.is_empty()
    {
        unsafe { std::env::set_var("PYTHONHOME", home) };
    }

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
        // 本阶段未实现运行时内中断；budget 由主进程看门狗兜底（超宽限
        // 终止宿主）。deadline 语义随资源限制提交接入。
        let _ = budget_ms;
        if kind != "init" && kind != "loop" {
            eprintln!("ztw-host-py: 未知执行类型 {kind}");
            std::process::exit(3);
        }
        let is_init = kind == "init";
        if is_init {
            namespace = None; // 初始化即重建命名空间（首次加载 / 热重载同路径）
        }

        // 单次执行在 attach 闭包内完成；init 成功时带回新命名空间。
        let (new_ns, outcome): (Option<Py<PyDict>>, Result<bool, FaultOut>) =
            Python::attach(|py| {
                if is_init {
                    match init_exec(py, source.as_deref().unwrap_or_default()) {
                        Ok((ns, has_loop)) => (Some(ns), Ok(has_loop)),
                        Err(f) => (None, Err(f)),
                    }
                } else {
                    match &namespace {
                        Some(ns) => (None, loop_exec(py, ns)),
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

/// 初始化执行：全新命名空间中整体执行玩家源码。
fn init_exec(py: Python<'_>, source: &str) -> Result<(Py<PyDict>, bool), FaultOut> {
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
        Err(e) => return Err(classify(py, &e)),
    };
    if let Err(e) = code.run(Some(&ns), Some(&ns)) {
        return Err(classify(py, &e));
    }
    let has_loop = ns
        .get_item("loop")
        .ok()
        .flatten()
        .is_some_and(|v| v.is_callable());
    Ok((ns.unbind(), has_loop))
}

/// loop 执行：调用入口函数。命名空间与全局变量复用（脚本级错误后保留）。
fn loop_exec(py: Python<'_>, ns: &Py<PyDict>) -> Result<bool, FaultOut> {
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
        Err(e) => Err(classify(py, &e)),
    }
}

/// 异常分类（最小集）：脚本级错误，code = 异常类型名。
/// KeyboardInterrupt / MemoryError 的互证分类随资源限制提交补全。
fn classify(py: Python<'_>, e: &PyErr) -> FaultOut {
    let value = e.value(py);
    let type_name = value
        .get_type()
        .name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
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
