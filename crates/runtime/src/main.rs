//! JS 宿主进程（docs/architecture/03「JS 宿主」）。
//!
//! - 内嵌 rquickjs（quickjs-ng）；Runtime 启动即设置堆上限、栈上限与
//!   中断处理器（deadline 到期返回 true，引擎抛出不可捕获异常）。
//! - 玩家程序按 ESM 模块图求值：入口模块（默认 main.js）经内存
//!   loader（player_modules，仅相对说明符、仅文件集内）链接依赖，
//!   初始化执行一次；此后每 tick 从入口模块 namespace 取 `loop` 导出
//!   （活绑定）同步调用。模块体运行错误让 eval 返回的 Promise 变
//!   Rejected——宿主收割后走统一故障分类；顶层 await 未完成
//!   （Pending）分类为脚本级故障。
//! - loop() 返回后清空微任务队列（动态 import() 与 await 续体在此
//!   解析），计入同一预算与中断覆盖；未处理的 Promise 拒绝经宿主端
//!   rejection tracker 收割，按脚本级故障上报。
//! - 变更型 Game 调用与 memory 读写经同步 IPC（stdin/stdout，长度前缀
//!   JSON）到主进程；查询经本地镜像应答。不提供定时器 / 网络。
//! - 故障分类：脚本级（语法 / 未捕获 / 中断 / TLA 未完成 / 未处理
//!   拒绝 / 程序不合法）保持宿主存活；环境级（JS 内存超限）销毁执行
//!   环境待主进程重新初始化；协议错误退出进程。
//!
//! 故障注入（ZTW_FAULT，逗号分隔，测试专用）：
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
//!   skip_delta_replay           丢弃镜像增量（触发整体重建路径）

use std::cell::RefCell;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rquickjs::promise::PromiseState;
use rquickjs::{Context, Ctx, Function, Module, Object, Persistent, Runtime, Value};

use ztw_api::BOOTSTRAP_JS;

mod limit_alloc;
mod player_modules;
use limit_alloc::LimitAllocator;
use player_modules::{PlayerLoader, PlayerResolver, RejectLedger, validate_files};
use ztw_api::protocol::{
    ExecStats, HostFrame, MainFrame, err_result, read_frame, resolve_program, write_frame,
};

/// 宿主读取主进程帧的上限（镜像随世界规模增长，取宽上限）。
const HOST_READ_LIMIT: u64 = 64 * 1024 * 1024;

/// JS 程序的默认入口文件名（协议 entry 字段可覆盖）。
const DEFAULT_ENTRY: &str = "main.js";

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
    // A1 协议注入（docs/architecture/08：旧消息与重复消息 / 三时点断连）。
    abort_before_send_op: Option<String>,
    abort_after_send: bool,
    dup_request_id: Option<u64>,
    /// 同号异负载重发（预期主进程 DUP_REQUEST_MISMATCH 终止宿主）。
    dup_request_corrupt_id: Option<u64>,
    /// 完成帧连发两次（预期第二次被主进程按旧执行丢弃，不重复结算）。
    dup_complete: bool,
    old_exec_request: bool,
    /// 仅本执行的首个请求改写为旧执行号：EXEC_CLOSED 被拒后宿主继续
    /// 正常发号，验证主进程把拒绝计入对账（不误判 GAP / MISMATCH）。
    old_exec_request_first: bool,
    stale_epoch: bool,
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

struct HostState {
    stdout: std::io::Stdout,
    request_id: u64,
    stats: ExecStats,
    faults: Faults,
    in_init: bool,
    frame_limit: usize,
    /// 当前宿主代次与执行编号：来自主进程 Exec 帧，随帧回显，
    /// 供主进程做旧代次 / 旧执行拒绝（docs/architecture/03 v2 会话头）。
    host_epoch: u64,
    execution_id: u64,
    /// 本执行是否已发出首个请求（old_exec_request_first 的一次性闸门，
    /// 收到新 Exec 帧时复位）。
    exec_first_request_sent: bool,
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
    });
}

/// JS 侧同步 IPC 入口：发请求、阻塞等回复。协议破坏一律退出（exit 3）。
fn ipc_js(_cx: Ctx, op: String, payload: String) -> rquickjs::Result<String> {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let t0 = Instant::now();
        // 故障注入：提交前断连（主进程未见过该请求）。
        if let Some(target) = &h.faults.abort_before_send_op
            && op == *target
        {
            eprintln!("ztw-host: 故障注入 abort_before_send:{target}");
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
            v: ztw_api::protocol::PROTOCOL_VERSION,
            host_epoch: frame_epoch,
            execution_id: frame_exec,
            request_id: rid,
            op: op.clone(),
            payload: ztw_api::protocol::raw_or_quoted(payload.clone()),
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
            )
            .get()
            .to_string());
        }
        if write_frame(&mut h.stdout, &frame).is_err() {
            eprintln!("ztw-host: 写 Game 请求失败");
            std::process::exit(3);
        }
        // 故障注入：提交后、回复前断连（主进程可能已提交，回复未送达）。
        // 仅第一代次宿主触发：测试要在崩溃后重启同款宿主跑“不重放”流程。
        if h.faults.abort_after_send && rid == 1 && epoch == 1 {
            eprintln!("ztw-host: 故障注入 abort_after_send");
            std::process::abort();
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
            host_epoch: reply_epoch,
            execution_id: reply_exec,
            request_id,
            result,
        } = reply
        else {
            eprintln!("ztw-host: 期望 Reply 帧");
            std::process::exit(3);
        };
        // 对称校验：回复必须属于当前代次与执行，且回复号等于请求号。
        if request_id != rid || reply_epoch != epoch || reply_exec != exec_id {
            eprintln!(
                "ztw-host: 回复错位 rid {request_id}!={rid} / epoch {reply_epoch}!={epoch} / exec {reply_exec}!={exec_id}"
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
            eprintln!("ztw-host: 故障注入 abort_init_after_mem");
            std::process::abort();
        }
        if let Some(target) = &h.faults.abort_after_reply_op
            && op == *target
        {
            eprintln!("ztw-host: 故障注入 abort_after_reply:{target}");
            std::process::abort();
        }
        // 故障注入：同号同负载重发一次（触发主进程去重缓存路径）。
        if h.faults.dup_request_id == Some(rid) {
            eprintln!("ztw-host: 故障注入 dup_request:{rid}");
            if write_frame(&mut h.stdout, &frame).is_err() {
                eprintln!("ztw-host: 重发 Game 请求失败");
                std::process::exit(3);
            }
            let second: MainFrame = match read_frame(&mut std::io::stdin(), HOST_READ_LIMIT) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("ztw-host: 读重发回复失败：{e}");
                    std::process::exit(3);
                }
            };
            let MainFrame::Reply {
                request_id: rid2,
                result: result2,
                ..
            } = second
            else {
                eprintln!("ztw-host: 期望重发的 Reply 帧");
                std::process::exit(3);
            };
            if rid2 != rid {
                eprintln!("ztw-host: 重发回复请求号错位 {rid2} != {rid}");
                std::process::exit(3);
            }
            if result2.get() != result.get() {
                eprintln!("ztw-host: 重发回复与原回复不一致");
                std::process::exit(3);
            }
        }
        // 故障注入：同号异负载重发（预期主进程终止宿主；读到 EOF 走退出路径）。
        if h.faults.dup_request_corrupt_id == Some(rid) {
            eprintln!("ztw-host: 故障注入 dup_request_corrupt:{rid}");
            let mut dup = frame.clone();
            if let HostFrame::Request { payload, .. } = &mut dup {
                // 指纹不同，负载仍可解析。v4 注意：RawValue 的捕获区间
                // 会跳过前导空白，只能在值内部（`{` 之后）插空格才能
                // 跨线存活。
                let mut text = payload.get().to_string();
                if !text.is_empty() {
                    text.insert(1, ' ');
                }
                *payload = ztw_api::protocol::raw_or_quoted(text);
            }
            if write_frame(&mut h.stdout, &dup).is_err() {
                std::process::exit(3);
            }
            match read_frame::<_, MainFrame>(&mut std::io::stdin(), HOST_READ_LIMIT) {
                Ok(MainFrame::Reply { .. }) => {
                    // 主进程未拒绝 = 协议语义破坏；退出让测试失败得显式。
                    eprintln!("ztw-host: 同号异负载未被主进程拒绝");
                    std::process::exit(3);
                }
                Ok(_) => {
                    eprintln!("ztw-host: 期望重发的 Reply 帧");
                    std::process::exit(3);
                }
                Err(_) => std::process::exit(3),
            }
        }
        Ok(result.get().to_string())
    })
}

// ---------------------------------------------------------------------------
// 执行环境
// ---------------------------------------------------------------------------

struct Env {
    /// 入口模块的 namespace（活绑定容器）与未处理拒绝账本。字段声明顺序
    /// 即 drop 顺序：两者都可能持有 Persistent / JS 值引用，不得活过
    /// Runtime——账本还经 tracker 闭包（存于 Runtime opaque）共享一份，
    /// 全部声明在 `rt` 之前，保证 Runtime 存活期内释放。
    loop_ns: Option<Persistent<Object<'static>>>,
    rejects: std::rc::Rc<std::cell::RefCell<RejectLedger>>,
    rt: Runtime,
    ctx: Context,
    /// 分配器状态镜像：限额拒绝标志（故障分类）与当前总额（诊断）。
    alloc_state: limit_alloc::AllocState,
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
    files: &std::collections::BTreeMap<String, String>,
) -> Env {
    // 堆限额由分配器执行（拒绝时置标志供故障分类，见 limit_alloc 模块
    // 注释）；quickjs 自身限额放到无穷，避免它的无标志拒绝抢先发生。
    let (alloc, alloc_state) = LimitAllocator::new(heap_limit);
    let rt = Runtime::new_with_alloc(alloc).expect("创建 Runtime");
    rt.set_memory_limit(usize::MAX);
    rt.set_max_stack_size(stack_limit);
    // 玩家模块装载：内存文件集 + 仅相对说明符（player_modules 注释）。
    // 未注册 loader 时 quickjs-ng 无文件系统回退，这里注册即封闭世界。
    rt.set_loader(PlayerResolver, PlayerLoader::new(files.clone()));
    // 未处理拒绝观测：async 内 throw / 无 catch 的拒绝原本随被丢弃的
    // promise 静默消失，tracker 把它们记入账本供执行尾部收割。
    // tracker 由引擎在 promise 机械内部同步调用——回调里不得执行任何
    // 玩家 JS（reason 的属性读取会跑 getter / Proxy trap，异常会以
    // pending 形态横跨帧边界污染后续执行），只做 C 级的 Persistent
    // 引用接管与计数；reason 的文本提取推迟到 harvest 的受控点。
    let rejects = std::rc::Rc::new(std::cell::RefCell::new(RejectLedger::default()));
    {
        let rejects = rejects.clone();
        rt.set_host_promise_rejection_tracker(Some(Box::new(
            move |cx, _promise, reason, handled| {
                let mut ledger = rejects.borrow_mut();
                if handled {
                    ledger.note_handled();
                } else {
                    ledger.note_unhandled(Persistent::save(&cx, reason));
                }
            },
        )));
    }
    let ctx = Context::full(&rt).expect("创建 Context");
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
    Env {
        loop_ns: None,
        rt,
        ctx,
        alloc_state,
        rejects,
    }
}

/// 拒绝原因的受控文本化（仅 harvest 调用）：属性读取可能执行玩家
/// getter / Proxy trap，每次读取后无条件清 pending，防异常跨帧污染。
fn describe_reason_guarded(cx: &Ctx<'_>, v: &Value<'_>) -> String {
    if let Some(o) = v.as_object() {
        let name: String = o.get::<_, String>("name").ok().unwrap_or_default();
        let _ = cx.catch();
        let message: String = o.get::<_, String>("message").ok().unwrap_or_default();
        let _ = cx.catch();
        if name.is_empty() && message.is_empty() {
            format!("{:?}", v.type_of())
        } else {
            format!("{name}: {message}")
        }
    } else if let Some(s) = v.as_string() {
        s.to_string().unwrap_or_default()
    } else {
        format!("{:?}", v.type_of())
    }
}

struct FaultOut {
    class: &'static str, // "script" | "environment"
    code: String,
    message: String,
    stack: String,
    /// 全空形态标记：pending exception 未能物化（name/message/stack 全
    /// 空）。分类互证据此判定，不匹配展示文案。
    bare: bool,
}

fn classify_exec_fault(env: &Env, interrupt_fired: &AtomicBool, heap_limit: usize) -> FaultOut {
    let mut out = env.ctx.with(|cx| classify_fault(&cx, interrupt_fired));
    // 全空形态（异常未能物化：quickjs 的 JS_ThrowOutOfMemory 重入保护在
    // 堆极限压住错误对象自身构造时返回裸 JS_EXCEPTION，mac CI 实录）与
    // 分配器的限额拒绝标志互证：标志由分配器在拒绝那一刻置位，不随
    // 异常展开释放堆而失效——确定性判据，不依赖消息文本与事后读数。
    //
    // limit_hit 是粘性标志（随执行复位），且 quickjs 的 OOM InternalError
    // 可被玩家 try/catch 吞掉——捕获后 throw undefined 也会落进本分支判
    // 环境级。这在既定教义内（限额确实被命中过，环境重建无损，属 OOM
    // 自伤可接受一类）；文案因此只陈述"执行中发生过拒绝"，不与分类时刻
    // 的总额数字相互矛盾（展开后堆已释放）。
    if out.bare {
        let hit = env.alloc_state.limit_hit.load(Ordering::SeqCst);
        let total = env.alloc_state.total.load(Ordering::SeqCst);
        if hit {
            return FaultOut {
                class: "environment",
                code: "MEMORY_LIMIT".into(),
                message: format!(
                    "JS 内存超限（本次执行中发生过限额拒绝；当前总额 {total}/上限 {heap_limit}，异常展开后可能已释放）"
                ),
                stack: out.stack,
                bare: false,
            };
        }
        out.message = format!("(无消息；分配器总额 {total}/{heap_limit}，无限额拒绝记录)");
    }
    out
}

/// rquickjs 调用失败按变体分流。`Allocation` 变体在 rquickjs 0.13 中
/// 仅由 Runtime/Context 构造路径产生（本宿主对构造失败直接 expect），
/// 运行期 eval/call 失败恒为 `Error::Exception`——本分支实际不可达，
/// 作为对未来 rquickjs 版本把分配失败透传到调用面的防御保留。
fn classify_call_err(
    env: &Env,
    interrupt_fired: &AtomicBool,
    heap_limit: usize,
    e: rquickjs::Error,
) -> FaultOut {
    if matches!(e, rquickjs::Error::Allocation) {
        return FaultOut {
            class: "environment",
            code: "MEMORY_LIMIT".into(),
            message: "JS 内存超限（宿主绑定层分配失败）".into(),
            stack: String::new(),
            bare: false,
        };
    }
    classify_exec_fault(env, interrupt_fired, heap_limit)
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
            bare: false,
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
            bare: false,
        };
    }
    let code = match name.as_str() {
        "" => "SCRIPT_ERROR".to_string(),
        other => other.to_string(),
    };
    // pending exception 为 undefined / 空对象时三字段全空：异常未能物化
    // 的形态标记，供 classify_exec_fault 与限额标志互证。
    let bare = name.is_empty() && message.is_empty() && stack.is_empty();
    FaultOut {
        class: "script",
        code,
        message: if message.is_empty() {
            "(无消息)".into()
        } else {
            message
        },
        stack,
        bare,
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
    // 旧 quickjs 语义 limit=0 表示不限；分配器语义 0 = 拒绝一切分配，
    // 会让 Runtime 构造直接 panic——显式传 0 时以可读错误退出。
    if heap_limit == 0 {
        eprintln!(
            "ztw-host: --heap-limit 0 无效（分配器语义下将拒绝一切分配；省略该参数使用默认 256MiB）"
        );
        std::process::exit(2);
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
    let mut program: Option<ztw_api::protocol::ResolvedProgram> = None;
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
            h.host_epoch = host_epoch;
            h.execution_id = execution_id;
            h.exec_first_request_sent = false;
        });
        let mut program_fault: Option<FaultOut> = None;
        if is_init {
            // 初始化即重建执行环境（首次加载 / 热重载 / 环境故障恢复同路径）。
            let resolved = resolve_program(files, entry, DEFAULT_ENTRY).and_then(|p| match p {
                Some((files, entry)) => validate_files(&files).map(|()| (files, entry)),
                None => Err("init 帧缺少文件集".to_string()),
            });
            match resolved {
                Ok(p) => {
                    env = Some(build_env(
                        heap_limit,
                        stack_limit,
                        deadline.clone(),
                        interrupt_fired.clone(),
                        &p.0,
                    ));
                    program = Some(p);
                }
                Err(message) => {
                    // 程序不合法（空文件集 / 入口缺失）：脚本级故障，
                    // 旧环境随之失效（本 init 未重建成功）。
                    env = None;
                    program_fault = Some(FaultOut {
                        class: "script",
                        code: "PROGRAM_INVALID".into(),
                        message,
                        stack: String::new(),
                        bare: false,
                    });
                }
            }
        }
        // 运行时内中断 deadline：本次执行的预算；中断标记同步复位。
        deadline.store(now_ms().saturating_add(budget_ms), Ordering::Relaxed);
        interrupt_fired.store(false, Ordering::SeqCst);
        if let Some(env) = env.as_ref() {
            env.alloc_state.limit_hit.store(false, Ordering::SeqCst);
        }

        // 注入镜像；执行玩家代码。env 借用限制在本块内。
        let run: Run = if let Some(fault) = program_fault {
            Run::Fault(fault)
        } else {
            let Some(env) = env.as_mut() else {
                // 环境故障销毁后、重新初始化前收到 loop：协议错误，干净退出。
                eprintln!("ztw-host: 执行环境已销毁，需要 init 执行重建");
                std::process::exit(3);
            };
            let mirror_str = mirror.as_ref().map(|m| m.get()).unwrap_or("{}");
            let session_gen = memory_gen;
            let t0 = Instant::now();
            let set_res = env.ctx.with(|cx| {
                let f: Function = cx
                    .globals()
                    .get::<_, Function>("__setMirror")
                    .expect("bootstrap 提供 __setMirror");
                f.call::<_, ()>((mirror_str, session_gen))
            });
            let mirror_us = t0.elapsed().as_micros() as u64;
            HOST.with(|h| h.borrow_mut().stats.mirror_parse_us = mirror_us);
            match set_res {
                Err(e) => Run::Fault(classify_call_err(env, &interrupt_fired, heap_limit, e)),
                Ok(()) => run_player(
                    env,
                    is_init,
                    program.as_ref(),
                    &mut first_loop_injected,
                    &interrupt_fired,
                    heap_limit,
                ),
            }
        };

        // 恢复 deadline 为“无限”，避免间隙期误触发。
        deadline.store(u64::MAX, Ordering::Relaxed);

        let (last_request_id, host_epoch, execution_id) = HOST.with(|h| {
            let h = h.borrow();
            (h.request_id, h.host_epoch, h.execution_id)
        });
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
                    host_epoch,
                    execution_id,
                    last_request_id,
                    has_loop,
                    stats,
                };
                if write_frame(&mut std::io::stdout(), &frame).is_err() {
                    break;
                }
                // 故障注入：完成帧连发（第二次应被主进程按旧执行丢弃）。
                let dup_complete = HOST.with(|h| h.borrow().faults.dup_complete);
                if dup_complete && write_frame(&mut std::io::stdout(), &frame).is_err() {
                    break;
                }
            }
            Run::Fault(out) => {
                let stats = HOST.with(|h| h.borrow_mut().stats.clone());
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

/// 玩家代码执行。init = ESM 入口模块声明 + 求值 + 终态收割；
/// loop = 入口模块 namespace 取 `loop` 导出（活绑定）调用。
/// 两分支结尾都在 with 块外清空微任务队列并收割未处理拒绝。
fn run_player(
    env: &mut Env,
    is_init: bool,
    program: Option<&ztw_api::protocol::ResolvedProgram>,
    first_loop_injected: &mut bool,
    interrupt_fired: &AtomicBool,
    heap_limit: usize,
) -> Run {
    env.rejects.borrow_mut().reset();
    if is_init {
        let (files, entry) = program.expect("init 已解析出程序");
        let entry_src = files
            .get(entry)
            .expect("resolve_program 已校验入口存在")
            .clone();
        // 声明（链接期校验）→ 求值 → namespace/promise 持久化，全部在同一
        // with 块内完成：Module 与 Promise 受 ctx 生命周期约束不可逃逸，
        // 只有 Persistent 句柄能出块。Err 覆盖说明符非法 / 模块缺失等
        // 链接期失败（异常留在 pending，走统一分类）。
        type Saved = (
            Persistent<Object<'static>>,
            Persistent<rquickjs::Promise<'static>>,
        );
        let saved = env.ctx.with(|cx| -> rquickjs::Result<Saved> {
            let module = Module::declare(cx.clone(), entry.clone(), entry_src)?;
            let (module, promise) = module.eval()?;
            let ns = module.namespace()?;
            Ok((Persistent::save(&cx, ns), Persistent::save(&cx, promise)))
        });
        let (ns_persist, promise_persist) = match saved {
            Ok(pair) => pair,
            Err(e) => return Run::Fault(classify_call_err(env, interrupt_fired, heap_limit, e)),
        };
        // 动态 import() 与顶层 await 续体在微任务队列里：with 块外 drain。
        if let Err(out) = drain_microtasks(env, interrupt_fired, heap_limit) {
            return Run::Fault(out);
        }
        // 模块求值终态：Resolved（完成）/ Rejected（模块体故障）/
        // Pending（顶层 await 未完成——await 永不解析的值挂起整个初始化）。
        enum Final {
            Done(bool),
            Rejected,
            Pending,
        }
        let final_state = env.ctx.with(|cx| -> Final {
            let Ok(promise) = promise_persist.clone().restore(&cx) else {
                // restore 失败无 pending 可读：按脚本错误处理。
                return Final::Rejected;
            };
            match promise.state() {
                PromiseState::Resolved => {
                    let has_loop = ns_persist
                        .clone()
                        .restore(&cx)
                        .ok()
                        .and_then(|ns| ns.get::<_, Value>("loop").ok())
                        .is_some_and(|v| v.is_function());
                    Final::Done(has_loop)
                }
                PromiseState::Rejected => {
                    // 把拒绝值转成 pending 异常，块外走统一分类。
                    let _ = promise.result::<()>();
                    Final::Rejected
                }
                PromiseState::Pending => Final::Pending,
            }
        });
        match final_state {
            Final::Done(has_loop) => {
                env.loop_ns = Some(ns_persist);
                harvest_rejections(env).map_or(Run::Ok(has_loop), Run::Fault)
            }
            Final::Rejected => {
                Run::Fault(classify_exec_fault(env, interrupt_fired, heap_limit))
            }
            Final::Pending => Run::Fault(FaultOut {
                class: "script",
                code: "ASYNC_INIT_PENDING".into(),
                message: "模块初始化未完成：顶层 await 等待的值在本执行内没有解析（沙箱无定时器/网络；跨执行挂起初始化不受支持）".into(),
                stack: String::new(),
                bare: false,
            }),
        }
    } else {
        if !*first_loop_injected {
            *first_loop_injected = true;
            inject_loop_faults(env);
        }
        // 入口契约：模块 namespace 的 loop 导出（活绑定，玩家重绑定可见）。
        let Some(ns_persist) = env.loop_ns.clone() else {
            return Run::Fault(FaultOut {
                class: "script",
                code: "ENTRY_MISSING".into(),
                message: "入口模块 namespace 不存在（初始化未完成？）".into(),
                stack: String::new(),
                bare: false,
            });
        };
        let entry_ok = env.ctx.with(|cx| {
            ns_persist
                .clone()
                .restore(&cx)
                .ok()
                .and_then(|ns| ns.get::<_, Value>("loop").ok())
                .is_some_and(|v| v.is_function())
        });
        if !entry_ok {
            return Run::Fault(FaultOut {
                class: "script",
                code: "ENTRY_MISSING".into(),
                message: "入口 loop 导出不再是函数（被玩家代码重绑定？）".into(),
                stack: String::new(),
                bare: false,
            });
        }
        let called = env.ctx.with(|cx| {
            let ns = ns_persist.restore(&cx).expect("刚校验可恢复");
            let f: Function = ns.get("loop").expect("已校验 loop 仍为函数");
            f.call::<_, ()>(())
        });
        match called {
            Ok(()) => match drain_microtasks(env, interrupt_fired, heap_limit) {
                Ok(()) => harvest_rejections(env).map_or(Run::Ok(true), Run::Fault),
                Err(out) => Run::Fault(out),
            },
            Err(e) => Run::Fault(classify_call_err(env, interrupt_fired, heap_limit, e)),
        }
    }
}

/// 执行尾部收割：存在未处理拒绝 → 脚本级可读故障（教学面：async 内
/// throw / 无 catch 的拒绝不再静默消失）。reason 的文本化在宿主受控
/// 点进行（见 describe_reason_guarded）。
fn harvest_rejections(env: &Env) -> Option<FaultOut> {
    let (unhandled, first) = {
        let mut ledger = env.rejects.borrow_mut();
        (ledger.unhandled_count(), ledger.take_first_reason())
    };
    if unhandled == 0 {
        return None;
    }
    let reason = env.ctx.with(|cx| {
        let desc = first
            .and_then(|p| p.restore(&cx).ok())
            .map(|v| describe_reason_guarded(&cx, &v))
            .unwrap_or_else(|| "(无消息)".to_string());
        // 兜底：提取路径的最后一步若留下 pending（防御性，正常已被
        // 逐次清除），在此统一清空。
        let _ = cx.catch();
        desc
    });
    Some(FaultOut {
        class: "script",
        code: "UNHANDLED_REJECTION".into(),
        message: format!(
            "存在 {unhandled} 个未处理的 Promise 拒绝（async 内抛错或 .then 链无 catch）；首个原因：{reason}"
        ),
        stack: String::new(),
        bare: false,
    })
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
