//! A0 JS 宿主进程（docs/architecture/03「JS 宿主」）。
//!
//! - 内嵌 rquickjs（quickjs-ng）；Runtime 启动即设置堆上限、栈上限与
//!   中断处理器（deadline 到期返回 true，引擎抛出不可捕获异常）。
//! - 玩家代码整脚本求值（初始化执行一次），此后每 tick 同步调用 loop()；
//!   loop() 返回后清空微任务队列，计入同一预算与中断覆盖。
//! - 变更型 Game 调用与 memory 读写经同步 IPC（stdin/stdout，长度前缀
//!   JSON）到主进程；查询经本地镜像应答。
//! - 动态 import 不注册加载器，运行时报错；不提供定时器 / 网络。
//! - 故障分类：脚本级（语法 / 未捕获 / 中断）保持宿主存活；环境级
//!   （JS 内存超限）销毁执行环境待主进程重新初始化；协议错误退出进程。
//!
//! 故障注入（ZTW_FAULT，逗号分隔，测试专用）：
//!   abort_init_after_mem      初始化第一条 memory 回复后 abort()
//!   abort_after_reply:<op>    指定 op 的回复送达后 abort()
//!   oversize_frame            首次 loop 执行前发送超大帧
//!   hang_exec                 首次 loop 执行前挂起（模拟宿主无响应）
//!   skip_delta_replay         丢弃镜像增量（触发整体重建路径）

use std::cell::RefCell;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rquickjs::{Context, Ctx, Function, Runtime, Value};

use ztw_api::BOOTSTRAP_JS;
use ztw_api::protocol::{ExecStats, HostFrame, MainFrame, err_result, read_frame, write_frame};

/// 宿主读取主进程帧的上限（镜像随世界规模增长，取宽上限）。
const HOST_READ_LIMIT: u64 = 64 * 1024 * 1024;

/// 宿主进程启动时刻：__nowUs 的计时零点。
static START: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// 故障注入
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Faults {
    abort_init_after_mem: bool,
    abort_after_reply_op: Option<String>,
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

struct HostState {
    stdout: std::io::Stdout,
    request_id: u64,
    stats: ExecStats,
    faults: Faults,
    in_init: bool,
    frame_limit: usize,
}

thread_local! {
    static HOST: RefCell<HostState> = RefCell::new(HostState {
        stdout: std::io::stdout(),
        request_id: 0,
        stats: ExecStats::default(),
        faults: Faults::default(),
        in_init: false,
        frame_limit: 1024 * 1024,
    });
}

/// JS 侧同步 IPC 入口：发请求、阻塞等回复。协议破坏一律退出（exit 3）。
fn ipc_js(_cx: Ctx, op: String, payload: String) -> rquickjs::Result<String> {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let t0 = Instant::now();
        h.request_id += 1;
        let rid = h.request_id;
        let frame = HostFrame::Request {
            v: ztw_api::protocol::PROTOCOL_VERSION,
            request_id: rid,
            op: op.clone(),
            payload,
        };
        // 解码前的限长检查：先实测完整帧的序列化长度（JSON 字符串转义会
        // 膨胀），超限以可读错误拒绝，避免必然被杀的协议路径。
        // 拒绝时不消耗请求号（帧未发出，主进程不会见到该 id）。
        let body = serde_json::to_vec(&frame).unwrap_or_default();
        if body.len() + 4 > h.frame_limit {
            h.request_id -= 1;
            return Ok(err_result(
                "FRAME_LIMIT",
                &format!("请求帧 {}B 超上限 {}B", body.len() + 4, h.frame_limit),
            ));
        }
        if write_frame(&mut h.stdout, &frame).is_err() {
            eprintln!("ztw-host: 写 Game 请求失败");
            std::process::exit(3);
        }
        let reply: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("ztw-host: 读回复失败：{e}");
                std::process::exit(3);
            }
        };
        let MainFrame::Reply {
            v: _,
            request_id,
            result,
        } = reply
        else {
            eprintln!("ztw-host: 期望 Reply 帧");
            std::process::exit(3);
        };
        if request_id != rid {
            eprintln!("ztw-host: 回复请求号错位 {request_id} != {rid}");
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
            eprintln!("ztw-host: 故障注入 abort_init_after_mem");
            std::process::abort();
        }
        if let Some(target) = &h.faults.abort_after_reply_op
            && op == *target
        {
            eprintln!("ztw-host: 故障注入 abort_after_reply:{target}");
            std::process::abort();
        }
        Ok(result)
    })
}

// ---------------------------------------------------------------------------
// 执行环境
// ---------------------------------------------------------------------------

struct Env {
    rt: Runtime,
    ctx: Context,
}

/// 单次执行结局：完成（has_loop 仅 init 有意义）或故障。
enum Run {
    Ok(bool),
    Fault(FaultOut),
}

fn build_env(
    heap_limit: usize,
    stack_limit: usize,
    deadline: Arc<AtomicU64>,
    interrupt_fired: Arc<AtomicBool>,
) -> Env {
    let rt = Runtime::new().expect("创建 Runtime");
    rt.set_memory_limit(heap_limit);
    rt.set_max_stack_size(stack_limit);
    {
        let deadline = deadline.clone();
        let fired = interrupt_fired.clone();
        rt.set_interrupt_handler(Some(Box::new(move || {
            // 真实中断由引擎抛出（不可捕获）；此标记供故障分类交叉验证，
            // 防止玩家伪造 InternalError("interrupted") 冒充中断。
            if now_ms() >= deadline.load(Ordering::Relaxed) {
                fired.store(true, Ordering::SeqCst);
                true
            } else {
                false
            }
        })));
    }
    let ctx = Context::full(&rt).expect("创建 Context");
    ctx.with(|cx| {
        cx.globals()
            .set(
                "__ipc",
                Function::new(cx.clone(), ipc_js).expect("注册 __ipc"),
            )
            .expect("注册 __ipc");
        // 微秒时钟：仅供绑定层量测计时，不构成游戏语义。
        cx.globals()
            .set(
                "__nowUs",
                Function::new(cx.clone(), || {
                    Instant::now()
                        .checked_duration_since(*START)
                        .map(|d| d.as_micros() as f64)
                        .unwrap_or(0.0)
                })
                .expect("注册 __nowUs"),
            )
            .expect("注册 __nowUs");
        cx.eval::<Value, _>(BOOTSTRAP_JS).expect("bootstrap 求值");
    });
    Env { rt, ctx }
}

struct FaultOut {
    class: &'static str, // "script" | "environment"
    code: String,
    message: String,
    stack: String,
}

fn classify_exec_fault(env: &Env, interrupt_fired: &AtomicBool, heap_limit: usize) -> FaultOut {
    // 堆耗尽时连读取异常对象属性都可能失败（getter / 字符串分配），
    // classify_fault 的 .ok().unwrap_or_default() 会把 InternalError 读成
    // 全空形态，误判 SCRIPT_ERROR "(无消息)"（mac CI 两次抖动实录：无限
    // 分类的故障记录 name/message 双空）。读异常前临时解除堆上限：
    // pending exception 本身存活于堆上、证据不损；读毕恢复原限，脚本级
    // 故障路径的后续语义不变。
    env.rt.set_memory_limit(usize::MAX);
    let out = env.ctx.with(|cx| classify_fault(&cx, interrupt_fired));
    env.rt.set_memory_limit(heap_limit);
    out
}

fn classify_fault(cx: &Ctx, interrupt_fired: &AtomicBool) -> FaultOut {
    let v = cx.catch();
    let mut name = String::new();
    let mut message = String::new();
    let mut stack = String::new();
    if let Some(o) = v.as_object() {
        name = o.get::<_, String>("name").ok().unwrap_or_default();
        message = o.get::<_, String>("message").ok().unwrap_or_default();
        stack = o.get::<_, String>("stack").ok().unwrap_or_default();
    } else if let Some(s) = v.as_string() {
        message = s.to_string().unwrap_or_default();
    }
    // 中断判定不信任消息文本：只有中断处理器真的触发过才归类 INTERRUPTED
    //（玩家可伪造 InternalError 对象，但触发不了处理器）。
    if name == "InternalError"
        && message.contains("interrupted")
        && interrupt_fired.load(Ordering::SeqCst)
    {
        return FaultOut {
            class: "script",
            code: "INTERRUPTED".into(),
            message: "执行超预算，运行时内中断生效".into(),
            stack,
        };
    }
    // OOM 判定信任引擎抛出的 InternalError：异常展开后 GC 往往已释放堆，
    // 事后读占用无法确证（实测），交叉验证反而漏报真实 OOM。玩家可用
    // InternalError 构造器伪造 OOM 强制环境重建——后果限于自己的全局
    // 变量丢失与重新初始化，属误用自伤，不是安全边界（docs/architecture/03
    // 首段定位）；中断伪造因会掩盖真实错误，仍用标记交叉验证。
    // 消息为空的 InternalError 同样按 OOM 处理：堆极限恰好落在引擎构造
    // OOM 错误消息字符串的分配上时会抛出退化形态 InternalError("")，
    // 严格按消息关键字匹配会把它漏成脚本错误（mac CI 两次抖出此形态，
    // 本地无法复现）；引擎生成的其他 InternalError 均带消息，空消息的
    // 引擎来源只有这一种，玩家伪造则落入已接受的自伤路径。
    if name == "InternalError"
        && (message.is_empty() || message.contains("memory") || message.contains("allocation"))
    {
        return FaultOut {
            class: "environment",
            code: "MEMORY_LIMIT".into(),
            message: if message.is_empty() {
                "JS 内存超限（引擎退化形态：空消息 InternalError）".into()
            } else {
                format!("JS 内存超限：{message}")
            },
            stack,
        };
    }
    let code = match name.as_str() {
        "" => "SCRIPT_ERROR".to_string(),
        other => other.to_string(),
    };
    FaultOut {
        class: "script",
        code,
        message: if message.is_empty() {
            "(无消息)".into()
        } else {
            message
        },
        stack,
    }
}

fn main() {
    let mut heap_limit = 256 * 1024 * 1024usize;
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
    let faults = parse_faults(std::env::var("ZTW_FAULT").ok());
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        h.faults = faults;
        h.frame_limit = frame_limit;
    });
    let deadline = Arc::new(AtomicU64::new(u64::MAX));
    let interrupt_fired = Arc::new(AtomicBool::new(false));
    let mut env: Option<Env> = None;
    let mut source: Option<String> = None;
    let mut first_loop_injected = false;

    loop {
        let frame: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("ztw-host: 读执行请求失败：{e}");
                break;
            }
        };
        let MainFrame::Exec {
            v,
            kind,
            budget_ms,
            source: src,
            mirror,
            memory_gen,
            ..
        } = frame
        else {
            eprintln!("ztw-host: 顶层只接受 Exec 帧");
            break;
        };
        if v != ztw_api::protocol::PROTOCOL_VERSION {
            eprintln!(
                "ztw-host: 协议版本 {v} != {}",
                ztw_api::protocol::PROTOCOL_VERSION
            );
            std::process::exit(3);
        }
        let is_init = kind == "init";
        HOST.with(|h| {
            let mut h = h.borrow_mut();
            h.in_init = is_init;
            h.stats = ExecStats::default();
        });
        if is_init {
            // 初始化即重建执行环境（首次加载 / 热重载 / 环境故障恢复同路径）。
            env = Some(build_env(
                heap_limit,
                stack_limit,
                deadline.clone(),
                interrupt_fired.clone(),
            ));
            source = src;
        }
        // 运行时内中断 deadline：本次执行的预算；中断标记同步复位。
        deadline.store(now_ms().saturating_add(budget_ms), Ordering::Relaxed);
        interrupt_fired.store(false, Ordering::SeqCst);

        // 注入镜像；执行玩家代码。env 借用限制在本块内。
        let run: Run = {
            let Some(env) = env.as_ref() else {
                // 环境故障销毁后、重新初始化前收到 loop：协议错误，干净退出。
                eprintln!("ztw-host: 执行环境已销毁，需要 init 执行重建");
                std::process::exit(3);
            };
            let (mirror_json, session_gen) = (mirror.unwrap_or_else(|| "{}".into()), memory_gen);
            let t0 = Instant::now();
            let set_ok = env.ctx.with(|cx| {
                let f: Function = cx
                    .globals()
                    .get::<_, Function>("__setMirror")
                    .expect("bootstrap 提供 __setMirror");
                f.call::<_, ()>((mirror_json, session_gen)).is_ok()
            });
            let mirror_us = t0.elapsed().as_micros() as u64;
            HOST.with(|h| h.borrow_mut().stats.mirror_parse_us = mirror_us);
            if !set_ok {
                Run::Fault(classify_exec_fault(env, &interrupt_fired, heap_limit))
            } else {
                run_player(
                    env,
                    is_init,
                    source.as_deref(),
                    &mut first_loop_injected,
                    &interrupt_fired,
                    heap_limit,
                )
            }
        };

        // 恢复 deadline 为“无限”，避免间隙期误触发。
        deadline.store(u64::MAX, Ordering::Relaxed);

        let last_request_id = HOST.with(|h| h.borrow().request_id);
        // 收割绑定层的增量回放 / 重建耗时（量测；环境已销毁时跳过）。
        if let Some(env) = env.as_ref() {
            let delta_us = env
                .ctx
                .with(|cx| {
                    cx.eval::<f64, _>("(globalThis.__mirrorStats && __mirrorStats().delta_us) || 0")
                })
                .unwrap_or(0.0);
            HOST.with(|h| h.borrow_mut().stats.mirror_apply_us = delta_us as u64);
        }
        match run {
            Run::Ok(has_loop) => {
                let stats = HOST.with(|h| h.borrow_mut().stats.clone());
                let frame = HostFrame::Complete {
                    v: ztw_api::protocol::PROTOCOL_VERSION,
                    last_request_id,
                    has_loop,
                    stats,
                };
                if write_frame(&mut std::io::stdout(), &frame).is_err() {
                    break;
                }
            }
            Run::Fault(out) => {
                let stats = HOST.with(|h| h.borrow_mut().stats.clone());
                let frame = HostFrame::Fault {
                    v: ztw_api::protocol::PROTOCOL_VERSION,
                    class: out.class.to_string(),
                    code: out.code,
                    message: out.message,
                    stack: out.stack,
                    last_request_id,
                    stats,
                };
                let sent = write_frame(&mut std::io::stdout(), &frame).is_ok();
                if out.class == "environment" {
                    // 内存超限后的 Runtime 状态不可信：立即销毁，
                    // 不从故障运行时提取持久数据（docs/architecture/03）。
                    env = None;
                }
                if !sent {
                    break;
                }
            }
        }
    }
}

/// 玩家代码执行（init 求值 / loop 调用 + 微任务清空）。
fn run_player(
    env: &Env,
    is_init: bool,
    source: Option<&str>,
    first_loop_injected: &mut bool,
    interrupt_fired: &AtomicBool,
    heap_limit: usize,
) -> Run {
    if is_init {
        let code = source.unwrap_or_default();
        use rquickjs::context::EvalOptions;
        let mut opts = EvalOptions::default();
        opts.global = true;
        opts.filename = Some("<player>".into());
        match env
            .ctx
            .with(|cx| cx.eval_with_options::<(), _>(code.as_bytes(), opts))
        {
            Ok(_) => {
                let has_loop = env.ctx.with(|cx| {
                    cx.globals()
                        .get::<_, Value>("loop")
                        .map(|v| v.is_function())
                        .unwrap_or(false)
                });
                Run::Ok(has_loop)
            }
            Err(_) => Run::Fault(classify_exec_fault(env, interrupt_fired, heap_limit)),
        }
    } else {
        if !*first_loop_injected {
            *first_loop_injected = true;
            inject_loop_faults(env);
        }
        // 入口可能被玩家代码覆写（globalThis.loop = 5）：脚本级故障，
        // 不炸宿主进程（docs/architecture/03 故障分级）。
        let entry_ok = env.ctx.with(|cx| {
            cx.globals()
                .get::<_, Value>("loop")
                .map(|v| v.is_function())
                .unwrap_or(false)
        });
        if !entry_ok {
            return Run::Fault(FaultOut {
                class: "script",
                code: "ENTRY_MISSING".into(),
                message: "入口 loop 不再是函数（被玩家代码覆写？）".into(),
                stack: String::new(),
            });
        }
        match env.ctx.with(|cx| {
            let f: Function = cx
                .globals()
                .get::<_, Function>("loop")
                .expect("已校验 loop 入口仍为函数");
            f.call::<_, ()>(())
        }) {
            Ok(()) => match drain_microtasks(env, interrupt_fired, heap_limit) {
                Ok(()) => Run::Ok(true),
                Err(out) => Run::Fault(out),
            },
            Err(_) => Run::Fault(classify_exec_fault(env, interrupt_fired, heap_limit)),
        }
    }
}

/// 首次 loop 执行前的故障注入（ZTW_FAULT）。
fn inject_loop_faults(env: &Env) {
    let (oversize, hang, skip_delta) = HOST.with(|h| {
        let h = h.borrow();
        (
            h.faults.oversize_frame,
            h.faults.hang_exec,
            h.faults.skip_delta_replay,
        )
    });
    if skip_delta {
        // 注入语义：take 的镜像增量被丢弃（回放失败路径），
        // 下一次查询经 mirror.fetch 整体重建。
        let _ = env
            .ctx
            .with(|cx| cx.eval::<(), _>("globalThis.__DROP_DELTAS = true;"));
    }
    if oversize {
        eprintln!("ztw-host: 故障注入 oversize_frame");
        let mut out = std::io::stdout();
        let _ = out.write_all(&0x7FFF_F000u32.to_be_bytes());
        let _ = out.write_all(&[b'x'; 4096]);
        let _ = out.flush();
        std::thread::sleep(Duration::from_secs(3600));
    }
    if hang {
        eprintln!("ztw-host: 故障注入 hang_exec");
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn drain_microtasks(
    env: &Env,
    interrupt_fired: &AtomicBool,
    heap_limit: usize,
) -> Result<(), FaultOut> {
    loop {
        match env.rt.execute_pending_job() {
            Ok(true) => continue,
            Ok(false) => return Ok(()),
            Err(_) => {
                // 微任务队列中的异常：按脚本级错误上报。
                return Err(classify_exec_fault(env, interrupt_fired, heap_limit));
            }
        }
    }
}
