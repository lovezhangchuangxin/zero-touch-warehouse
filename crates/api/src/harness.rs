//! A0 headless 主进程替身。扮演 docs/architecture/01 中 `crates/desktop`
//! 的宿主生命周期管理与世界线程角色，仅供 `cargo test` 与量测使用，
//! 不属于正式发布的主进程。
//!
//! 世界线程按消息循环实现（docs/architecture/03）：发出执行请求后在
//! 本线程直读宿主管道，只处理四类事件——Game 请求应答、执行完成、
//! 执行故障、宿主终止（EOF/坏帧）。
//! 看门狗在独立线程对执行计时，超宽限期直接终止宿主进程，不经过
//! Game 请求队列。

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::value::RawValue;
use ztw_model::{MemValue, OrderSide};
use ztw_sim::World;

use crate::memory::{MemoryLimits, MemoryTree, NodeKind, ReadResult};
use crate::mirror::{MirrorDelta, MirrorView, WorldRevision};
use crate::protocol::{
    ExecStats, HostFrame, MainFrame, err_result, ok_result, raw_or_quoted, read_frame, write_frame,
};

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub host_bin: std::path::PathBuf,
    /// 程序初始化单独预算（docs/architecture/03）。
    pub init_budget_ms: u64,
    /// 每 tick 执行预算：基础 + 每实体余量 × 对象数，封顶。
    pub tick_budget_base_ms: u64,
    pub tick_budget_per_entity_us: u64,
    pub tick_budget_cap_ms: u64,
    /// 主进程看门狗宽限期：预算到期后再等这么久就终止宿主。
    pub grace_ms: u64,
    /// 单帧消息上限（主进程与宿主各自在解码前执行）。
    pub frame_limit: usize,
    pub memory: MemoryLimits,
    pub log_entry_limit: usize,
    pub log_ring_cap: usize,
    pub heap_limit: usize,
    pub stack_limit: usize,
    /// 每执行请求数上限：结果去重缓存的容量边界；达到上限暂停报错
    /// （docs/architecture/03 IPC 提交协议）。
    pub request_limit_per_exec: u64,
    /// 每执行去重缓存的字节上界（指纹 + 原结果累计，默认 64MB）。
    /// 耗尽后跳过缓存新条目——请求照常执行并应答，该号重发因无缓存
    /// 可回放按既有 DUP_REQUEST_MISMATCH 协议故障处理（docs 03：主
    /// 进程结果构造受字节数上界约束，不得在请求风暴下无界堆积）。
    pub dedup_cache_bytes: usize,
    /// 追加环境变量（故障注入经 ZTW_FAULT 传递给宿主）。
    pub env: Vec<(String, String)>,
}

impl SessionConfig {
    pub fn new(host_bin: impl Into<std::path::PathBuf>) -> SessionConfig {
        SessionConfig {
            host_bin: host_bin.into(),
            init_budget_ms: 5_000,
            tick_budget_base_ms: 200,
            tick_budget_per_entity_us: 50,
            tick_budget_cap_ms: 500,
            grace_ms: 1_500,
            frame_limit: 1024 * 1024,
            memory: MemoryLimits::default(),
            log_entry_limit: 2 * 1024,
            log_ring_cap: 256,
            heap_limit: 256 * 1024 * 1024,
            stack_limit: 1024 * 1024,
            request_limit_per_exec: 50_000,
            dedup_cache_bytes: 64 * 1024 * 1024,
            env: Vec::new(),
        }
    }

    /// 故障注入（宿主测试钩子，见 runtime main.rs 的 ZTW_FAULT）。
    pub fn with_fault(mut self, faults: &str) -> SessionConfig {
        self.env.push(("ZTW_FAULT".to_string(), faults.to_string()));
        self
    }

    /// 宿主二进制是否为 Python 宿主（决定 load_code 单文件便捷包装的
    /// 入口名 main.py / main.js）。以二进制文件名含 "py" 判定——本仓
    /// 内仅有 ztw-host-js / ztw-host-py 两种宿主。已知限制：经
    /// ZTW_HOST_BIN 指到自命名二进制可能误判，但两个误判方向都终止于
    /// 宿主侧扩展名校验的可读 PROGRAM_INVALID，不存在按错误语言静默
    /// 执行的路径；桌面生产路径带显式 language，不受此影响。
    pub fn is_python_host(&self) -> bool {
        self.host_bin
            .file_name()
            .map(|s| s.to_string_lossy().contains("py"))
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// 故障与结果
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum FaultClass {
    /// 脚本级错误：语法错误、未捕获异常、运行时内中断。
    Script,
    /// 环境级故障：JS 内存超限；宿主内销毁重建执行环境。
    Environment,
    /// 宿主终止：watchdog=看门狗超宽限期、crash=进程崩溃、
    /// killed=主进程主动终止、protocol=协议错误、write=管道写失败。
    HostTerminated(&'static str),
}

#[derive(Debug, Clone)]
pub struct FaultRecord {
    pub class: FaultClass,
    pub code: String,
    pub message: String,
    pub stack: String,
    pub tick: u64,
    /// 请求关闭点（docs/architecture/08 验收：记录请求关闭点；
    /// 03：附最后已提交 API、tick 和请求编号）。
    pub last_request_id: u64,
    pub last_op: String,
    pub requests_served: u64,
}

#[derive(Debug, Clone)]
pub struct TickOutcome {
    pub kind: OutcomeKind,
    pub tick: u64,
    pub requests_served: u64,
    /// 本 tick 结算结果（机器人 id → 结果码）。
    pub settle: Vec<(ztw_model::Id, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutcomeKind {
    Ok,
    Fault(FaultClass),
    /// 会话处于暂停（此前已有故障未恢复）。
    Paused,
}

#[derive(Debug, Clone)]
pub struct InitOutcome {
    pub ok: bool,
    pub fault: Option<FaultRecord>,
    pub has_loop: bool,
}

// ---------------------------------------------------------------------------
// 诊断采集口（docs/architecture/02 §状态快照与诊断事件）
// ---------------------------------------------------------------------------

/// 诊断采集事件：受理失败、管理操作结果与订单完成只在本层可见
/// （受理码在 handle_op 应答、订单完成发生在 settle 内部），桌面壳的
/// 世界线程每 tick 排空后并入自己的诊断环形缓冲。
#[derive(Debug, Clone, Serialize)]
pub struct DiagTap {
    pub tick: u64,
    pub kind: DiagTapKind,
    /// 来源操作（robot.* / market.* / manage.*；订单完成为 settle.order_done）。
    pub op: String,
    pub code: String,
    /// 机器人 / 订单 / 目标 id。
    pub subject: Option<ztw_model::Id>,
    /// 人读摘要（参数、金额、效果）。
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagTapKind {
    /// 动作 / 管理操作受理失败（含初始化阶段的 INIT_PHASE 拒绝）。
    AcceptFail,
    /// 管理操作成功（接单 / 取消 / 销毁的效果摘要）。
    Manage,
    /// 结算内车辆离场触发的订单完成与收款。
    OrderDone,
}

/// 采集环容量（桌面侧按 tick 排空，256 仅防单 tick 内刷爆）。
const DIAG_TAP_CAP: usize = 256;

// ---------------------------------------------------------------------------
// 终止按钮独立控制路径（docs/architecture/03：不排队在 Game 请求之后）
// ---------------------------------------------------------------------------

/// 宿主进程的外部控制柄：从任意线程直接终止宿主，不经过世界线程的
/// Game 请求队列。置位 killed 后杀进程，世界线程在消息循环出口按
/// `killed_by_us` 将故障分类为 `KILLED_BY_MAIN`。
#[derive(Debug, Clone, Default)]
pub struct HostControl {
    child: Option<Arc<Mutex<Child>>>,
    killed: Arc<AtomicBool>,
}

impl HostControl {
    /// 标记为主进程主动终止并杀死宿主（幂等；宿主不在场则仅置位标记，
    /// 下一次执行的写失败 / EOF 路径据此分类）。
    pub fn kill(&self) {
        // 无宿主在场（尚未加载代码 / 刚被回收）时不置标记：跨代次残留的
        // 标记会把之后真实 crash 误分类为 KILLED_BY_MAIN。
        let Some(child) = &self.child else {
            return;
        };
        self.killed.store(true, Ordering::SeqCst);
        kill_and_reap(child);
    }

    /// 是否已被标记为主进程主动终止。
    pub fn is_marked(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// 量测累计
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct SessionStats {
    /// 世界线程侧每请求处理耗时（µs）。
    pub op_us: Vec<u64>,
    /// 镜像产出耗时（µs）与字节。
    pub mirror_ser_us: Vec<u64>,
    pub mirror_bytes: Vec<usize>,
    /// 管理操作增量字节。
    pub delta_bytes: Vec<usize>,
    /// 整 tick 墙钟（µs）。
    pub tick_us: Vec<u64>,
    /// 最近一次执行的宿主侧统计。
    pub last_exec: ExecStats,
    /// 宿主重启次数。
    pub host_restarts: u64,
}

impl SessionStats {
    pub fn take(&mut self) -> SessionStats {
        std::mem::take(self)
    }
}

// ---------------------------------------------------------------------------
// 宿主进程
// ---------------------------------------------------------------------------

struct HostProc {
    child: Arc<Mutex<Child>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    /// 执行期读到 EOF / 坏帧，或被本进程终止（进程退出）。
    dead: Arc<AtomicBool>,
    #[allow(dead_code)]
    stderr_thread: JoinHandle<()>,
}

impl HostProc {
    fn spawn(
        cfg: &SessionConfig,
    ) -> std::io::Result<(
        HostProc,
        ChildStdin,
        std::io::BufReader<std::process::ChildStdout>,
    )> {
        let mut cmd = Command::new(&cfg.host_bin);
        cmd.args([
            "--heap-limit",
            &cfg.heap_limit.to_string(),
            "--stack-limit",
            &cfg.stack_limit.to_string(),
            "--frame-limit",
            &cfg.frame_limit.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let dead = Arc::new(AtomicBool::new(false));
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let tail = stderr_tail.clone();
        let stderr_thread = std::thread::Builder::new()
            .name("ztw-host-stderr".into())
            .spawn(move || {
                use std::io::Read;
                let mut stderr = stderr;
                let mut buf = [0u8; 1024];
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut t = tail.lock().unwrap();
                            let keep = 16 * 1024;
                            if t.len() + n > keep {
                                let drop = (t.len() + n - keep).min(t.len());
                                t.drain(0..drop);
                            }
                            t.extend_from_slice(&buf[..n]);
                        }
                    }
                }
            })
            .expect("spawn stderr reader");
        let proc = HostProc {
            child: Arc::new(Mutex::new(child)),
            stderr_tail,
            dead,
            stderr_thread,
        };
        // BufReader：宿主单次写整帧后，前缀 + 体的两次 read 命中同一缓冲，
        // 每帧一次 syscall（跨帧残余也留在缓冲里）。
        Ok((proc, stdin, std::io::BufReader::new(stdout)))
    }
}

/// 终止宿主并异步回收：SIGKILL 后由独立线程 wait() 收尸，
/// 避免长时间浸泡运行累积僵尸进程。
fn kill_and_reap(child: &Arc<Mutex<Child>>) {
    if let Ok(mut c) = child.lock() {
        let _ = c.kill();
    }
    let child = child.clone();
    let _ = std::thread::Builder::new()
        .name("ztw-reaper".into())
        .spawn(move || {
            if let Ok(mut c) = child.lock() {
                let _ = c.wait();
            }
        });
}

impl HostProc {
    fn kill(&self) {
        self.dead.store(true, Ordering::SeqCst);
        kill_and_reap(&self.child);
    }

    fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr_tail.lock().unwrap()).into_owned()
    }
}

// ---------------------------------------------------------------------------
// 执行结果（消息循环出口）
// ---------------------------------------------------------------------------

enum ExecResult {
    Complete {
        has_loop: bool,
        #[allow(dead_code)] // 量测快照在消息循环内已并入 self.stats
        stats: ExecStats,
        requests_served: u64,
    },
    Faulted {
        class: FaultClass,
        code: String,
        message: String,
        stack: String,
        #[allow(dead_code)] // 同上
        stats: ExecStats,
        requests_served: u64,
        last_request_id: u64,
    },
}

// ---------------------------------------------------------------------------
// 玩家程序（多文件形态）
// ---------------------------------------------------------------------------

/// 一次 init 提交的完整玩家程序：文件集 + 入口文件名。
/// 语言由宿主二进制决定（ztw-host-js / ztw-host-py）。
#[derive(Debug, Clone)]
pub struct PlayerProgram {
    /// 文件名（相对名，如 `main.js`、`lib/geo.js`）→ 源码。
    pub files: std::collections::BTreeMap<String, String>,
    /// 入口文件名。JS 侧是 ESM 入口模块，Python 侧是整段执行的源码文件。
    pub entry: String,
}

impl PlayerProgram {
    pub fn new(
        files: std::collections::BTreeMap<String, String>,
        entry: impl Into<String>,
    ) -> Self {
        PlayerProgram {
            files,
            entry: entry.into(),
        }
    }

    /// 单文件便捷形态（JS）：整个源码即入口 `main.js`。
    pub fn single_js(code: &str) -> Self {
        PlayerProgram::new(
            [("main.js".to_string(), code.to_string())]
                .into_iter()
                .collect(),
            "main.js",
        )
    }

    /// 单文件便捷形态（Python）：整个源码即入口 `main.py`。
    pub fn single_py(code: &str) -> Self {
        PlayerProgram::new(
            [("main.py".to_string(), code.to_string())]
                .into_iter()
                .collect(),
            "main.py",
        )
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

pub struct Session {
    pub cfg: SessionConfig,
    pub world: World,
    pub memory: MemoryTree,
    pub logs: VecDeque<(u64, String)>,
    host: Option<HostProc>,
    host_stdin: Option<ChildStdin>,
    host_stdout: Option<std::io::BufReader<std::process::ChildStdout>>,
    program: Option<PlayerProgram>,
    pub initialized: bool,
    pub fault: Option<FaultRecord>,
    /// 宿主代次：每次 spawn 递增、不回收；旧代次消息永远拒绝
    /// （docs/architecture/03：重启增加 host_epoch）。
    host_epoch: u64,
    execution_id: u64,
    last_request_id: u64,
    /// 当前执行的去重缓存：request_id →（op+payload 指纹, 原结果）。
    /// 重复请求号同负载返回原结果、异负载协议故障；缓存随执行清空，
    /// 容量受条目数与字节（exec_dedup_bytes ↔ cfg.dedup_cache_bytes）
    /// 双上界约束。值 =（(op,payload) 增量哈希， 原结果）。
    exec_dedup: std::collections::HashMap<u64, (u64, Box<RawValue>)>,
    /// 本执行去重缓存累计字节（指纹 + 结果），随 exec_dedup 在执行
    /// 入口归零；达到 cfg.dedup_cache_bytes 后跳过缓存新条目——该号
    /// 重发走既有 DUP_REQUEST_MISMATCH 协议故障（降级语义见 dedup_put）。
    exec_dedup_bytes: usize,
    /// 本执行已受理的新请求数（去重重发不计数）。
    exec_new_requests: u64,
    /// 本执行是否已触及请求上限（结局改判 REQUEST_LIMIT，见 run_exec）。
    exec_limit_hit: bool,
    /// 本执行已见到的最大请求号（含被 EXEC_CLOSED / REQUEST_LIMIT 拒绝的）。
    /// 完成帧引用宿主的最后发送号，须与“已见”对账而非“已执行”。
    exec_seen_request_id: u64,
    revision: WorldRevision,
    mirror_cache: Option<String>,
    pub stats: SessionStats,
    /// 初始化期间的临时 memory 分支。
    init_branch: Option<MemoryTree>,
    in_init: bool,
    /// 主进程主动终止标记（“终止按钮”路径的结局分类）。
    killed_by_us: Arc<AtomicBool>,
    /// 最近一次已应答的 Game 操作名（诊断用）。
    last_op: String,
    /// 诊断采集环（B2 桌面壳排空；docs/architecture/02 §诊断事件）。
    pub diag: VecDeque<DiagTap>,
}

impl Session {
    pub fn new(cfg: SessionConfig, world: World) -> Session {
        let memory = MemoryTree::new(cfg.memory.clone());
        Session {
            cfg,
            world,
            memory,
            logs: VecDeque::new(),
            host: None,
            host_stdin: None,
            host_stdout: None,
            program: None,
            initialized: false,
            fault: None,
            host_epoch: 0,
            execution_id: 0,
            last_request_id: 0,
            exec_dedup: std::collections::HashMap::new(),
            exec_dedup_bytes: 0,
            exec_new_requests: 0,
            exec_limit_hit: false,
            exec_seen_request_id: 0,
            revision: WorldRevision::default(),
            mirror_cache: None,
            stats: SessionStats::default(),
            init_branch: None,
            in_init: false,
            killed_by_us: Arc::new(AtomicBool::new(false)),
            last_op: String::new(),
            diag: VecDeque::new(),
        }
    }

    // -- 对外：诊断采集与控制柄 ----------------------------------------------

    /// 排空诊断采集环（桌面世界线程每 tick 调用）。
    pub fn take_diag(&mut self) -> Vec<DiagTap> {
        self.diag.drain(..).collect()
    }

    /// 当前世界修订号（渲染快照派生用，docs/architecture/02）。
    pub fn world_revision(&self) -> WorldRevision {
        self.revision.clone()
    }

    /// 宿主进程外部控制柄（终止按钮：从任意线程直杀，不经 Game 队列）。
    pub fn host_control(&self) -> HostControl {
        HostControl {
            child: self.host.as_ref().map(|h| h.child.clone()),
            killed: self.killed_by_us.clone(),
        }
    }

    fn diag_push(
        &mut self,
        kind: DiagTapKind,
        op: &str,
        code: &str,
        subject: Option<ztw_model::Id>,
        detail: String,
    ) {
        self.diag.push_back(DiagTap {
            tick: self.world.tick,
            kind,
            op: op.to_string(),
            code: code.to_string(),
            subject,
            detail,
        });
        while self.diag.len() > DIAG_TAP_CAP {
            self.diag.pop_front();
        }
    }

    // -- 对外：初始化 / tick / 恢复 ------------------------------------------

    /// 加载玩家源码（单文件便捷形态）：按宿主二进制语言包成
    /// `main.js` / `main.py` 单文件程序。多文件用 [`Session::load_program`]。
    pub fn load_code(&mut self, code: &str) -> InitOutcome {
        let program = if self.cfg.is_python_host() {
            PlayerProgram::single_py(code)
        } else {
            PlayerProgram::single_js(code)
        };
        self.load_program(&program)
    }

    /// 加载完整玩家程序（文件集 + 入口）：启动（或复用）宿主并运行
    /// 初始化执行。失败时世界与原 memory 不变（docs/architecture/06）。
    pub fn load_program(&mut self, program: &PlayerProgram) -> InitOutcome {
        // 快速失败：入口缺失 / 空文件集直接在主进程拦截（与宿主侧
        // resolve_program 同源同文案），省一次完整 spawn + init 往返
        //（Python 宿主引导是百毫秒级）。
        if let Err(message) = crate::protocol::resolve_program(
            Some(program.files.clone()),
            Some(program.entry.clone()),
            "",
        ) {
            let rec = FaultRecord {
                class: FaultClass::Script,
                code: "PROGRAM_INVALID".into(),
                message,
                stack: String::new(),
                tick: self.world.tick,
                last_request_id: 0,
                last_op: String::new(),
                requests_served: 0,
            };
            self.fault = Some(rec.clone());
            return InitOutcome {
                ok: false,
                fault: Some(rec),
                has_loop: false,
            };
        }
        if self.host.is_none() {
            match HostProc::spawn(&self.cfg) {
                // BufReader：宿主单次写整帧后，前缀 + 体的两次 read 命中同一缓冲，
                // 每帧一次 syscall（跨帧残余也留在缓冲里）。
                Ok((proc, stdin, stdout)) => {
                    // 新宿主进程 = 新代次（docs/architecture/03：重启递增
                    // host_epoch，旧代次消息永远拒绝）。
                    self.host_epoch += 1;
                    self.host = Some(proc);
                    self.host_stdin = Some(stdin);
                    self.host_stdout = Some(stdout);
                }
                Err(e) => {
                    return InitOutcome {
                        ok: false,
                        fault: Some(FaultRecord {
                            class: FaultClass::HostTerminated("spawn"),
                            code: "SPAWN_FAILED".into(),
                            message: format!(
                                "{}（宿主二进制 {}）——先构建对应宿主（cargo build -p ztw-runtime / -p ztw-runtime-py）或设置 ZTW_HOST_BIN / ZTW_HOST_PY_BIN",
                                e,
                                self.cfg.host_bin.display()
                            ),
                            stack: String::new(),
                            tick: self.world.tick,
                            last_request_id: 0,
                            last_op: String::new(),
                            requests_served: 0,
                        }),
                        has_loop: false,
                    };
                }
            }
        }
        self.program = Some(program.clone());
        self.run_init(program)
    }

    fn run_init(&mut self, program: &PlayerProgram) -> InitOutcome {
        self.init_branch = Some(self.memory.fork());
        self.in_init = true;
        let mirror = self.produce_mirror();
        let exec_id = self.next_exec_id();
        let result = self.run_exec(
            exec_id,
            "init",
            self.cfg.init_budget_ms,
            mirror,
            Some(program),
        );
        self.in_init = false;
        match result {
            ExecResult::Complete { has_loop: true, .. } => {
                let branch = self.init_branch.take().expect("分支存在");
                self.memory.commit_branch(branch);
                self.initialized = true;
                self.fault = None;
                InitOutcome {
                    ok: true,
                    fault: None,
                    has_loop: true,
                }
            }
            ExecResult::Complete {
                has_loop: false,
                requests_served,
                ..
            } => {
                self.init_branch = None; // 丢弃临时分支
                // init 失败即未初始化：清除旧值，防止陈旧的 initialized
                // 让 resume_after_script_error 放行后续 loop 帧打到已
                // 销毁的环境（宿主侧 exit(3)）。
                self.initialized = false;
                let rec = FaultRecord {
                    class: FaultClass::Script,
                    code: "ENTRY_MISSING".into(),
                    message: "初始化成功但入口 loop 不是函数".into(),
                    stack: String::new(),
                    tick: self.world.tick,
                    last_request_id: self.last_request_id,
                    last_op: self.last_op.clone(),
                    requests_served,
                };
                self.fault = Some(rec.clone());
                InitOutcome {
                    ok: false,
                    fault: Some(rec),
                    has_loop: false,
                }
            }
            ExecResult::Faulted {
                class,
                code,
                message,
                stack,
                requests_served,
                last_request_id,
                ..
            } => {
                self.init_branch = None; // 丢弃临时分支：世界与原 memory 不变
                // 同上：init 失败后旧环境可能已被宿主销毁（如
                // PROGRAM_INVALID / 环境级故障），不得再接受 loop 帧。
                self.initialized = false;
                if matches!(class, FaultClass::HostTerminated(_)) {
                    self.drop_host();
                }
                let rec = FaultRecord {
                    class,
                    code,
                    message,
                    stack,
                    tick: self.world.tick,
                    last_request_id,
                    last_op: self.last_op.clone(),
                    requests_served,
                };
                self.fault = Some(rec.clone());
                InitOutcome {
                    ok: false,
                    fault: Some(rec),
                    has_loop: false,
                }
            }
        }
    }

    /// 推进一个完整 tick（docs/architecture/02 五阶段）。
    pub fn tick(&mut self) -> TickOutcome {
        if !self.initialized {
            panic!("尚未初始化玩家代码");
        }
        if self.fault.is_some() {
            return TickOutcome {
                kind: OutcomeKind::Paused,
                tick: self.world.tick,
                requests_served: 0,
                settle: Vec::new(),
            };
        }
        let t0 = Instant::now();
        // 阶段 1：边界事件（车辆到场）。
        self.world.boundary_events();
        // 阶段 2：产出查询视图（last_result 已在上一 tick 结算写入）。
        let mirror = self.produce_mirror();
        // 阶段 3：调用 loop()。玩家出错 / 超限时已受理动作照常进入结算。
        let exec_id = self.next_exec_id();
        let budget = self.tick_budget_ms();
        let exec = self.run_exec(exec_id, "loop", budget, mirror, None);
        let mut fault = None;
        match exec {
            ExecResult::Complete { .. } => {}
            ExecResult::Faulted {
                class,
                code,
                message,
                stack,
                requests_served,
                last_request_id,
                ..
            } => {
                if matches!(class, FaultClass::HostTerminated(_)) {
                    self.drop_host();
                }
                fault = Some(FaultRecord {
                    class,
                    code,
                    message,
                    stack,
                    tick: self.world.tick,
                    last_request_id,
                    last_op: self.last_op.clone(),
                    requests_served,
                });
            }
        }
        // 阶段 4：统一结算（受理 → 只读求解 → 原子提交）。
        // 结算前快照已接订单：settle 期间 my_orders 只会因车辆离场完成而
        // 移除（取消发生在阶段 3 管理操作、有自己的采集），消失者即完成。
        let pre_orders: Vec<(ztw_model::Id, OrderSide, u32, ztw_model::MilliGold)> = self
            .world
            .my_orders
            .values()
            .map(|o| (o.id, o.side, o.qty, o.unit_price_milli))
            .collect();
        let settle_map = self.world.settle();
        for (order_id, side, qty, unit_price_milli) in pre_orders {
            if !self.world.my_orders.contains_key(&order_id) {
                let pay_milli = if side == OrderSide::Buy {
                    qty as ztw_model::MilliGold * unit_price_milli
                } else {
                    0
                };
                self.diag_push(
                    DiagTapKind::OrderDone,
                    "settle.order_done",
                    ztw_model::codes::OK,
                    Some(order_id),
                    format!(
                        "订单完成 {}×{} @{} milli，收款 {} milli，余额 {} milli",
                        side.as_str(),
                        qty,
                        unit_price_milli,
                        pay_milli,
                        self.world.gold_milli
                    ),
                );
            }
        }
        let settle: Vec<(ztw_model::Id, String)> = settle_map
            .iter()
            .map(|(id, r)| (*id, r.code.clone()))
            .collect();
        // 阶段 5：收尾（计息 / 渲染快照不在 A0）。修订号的 tick 分量与
        // 发布后的世界 tick 对齐（B2 快照派生用它区分帧）。
        self.world.end_tick();
        self.revision.tick = self.world.tick;
        let requests_served = self.stats.last_exec.ipc_count;
        let outcome = match fault {
            None => TickOutcome {
                kind: OutcomeKind::Ok,
                tick: self.world.tick,
                requests_served,
                settle,
            },
            Some(rec) => {
                let kind = OutcomeKind::Fault(rec.class.clone());
                let served = rec.requests_served;
                self.fault = Some(rec);
                TickOutcome {
                    kind,
                    tick: self.world.tick,
                    requests_served: served,
                    settle,
                }
            }
        };
        self.stats.tick_us.push(t0.elapsed().as_micros() as u64);
        outcome
    }

    /// 脚本级错误恢复：执行环境与全局变量保留，直接继续。
    /// 仅对已成功初始化后的执行期脚本错误有效（入口缺失 / 初始化失败
    /// 不属于可恢复执行）。
    pub fn resume_after_script_error(&mut self) -> bool {
        match &self.fault {
            Some(rec) if rec.class == FaultClass::Script && self.initialized => {
                self.fault = None;
                true
            }
            _ => false,
        }
    }

    /// 环境级故障恢复：宿主内销毁重建执行环境，重新初始化同一程序。
    pub fn reinit_after_env_fault(&mut self) -> InitOutcome {
        let can = matches!(
            self.fault.as_ref().map(|f| &f.class),
            Some(FaultClass::Environment)
        ) && self.host.is_some();
        if !can {
            return InitOutcome {
                ok: false,
                fault: self.fault.clone(),
                has_loop: false,
            };
        }
        let program = self.program.clone().expect("有已加载程序");
        self.run_init(&program)
    }

    /// 宿主终止恢复：重启宿主进程并重新初始化。
    pub fn restart_host(&mut self) -> InitOutcome {
        self.drop_host();
        self.stats.host_restarts += 1;
        let program = match &self.program {
            Some(p) => p.clone(),
            None => {
                return InitOutcome {
                    ok: false,
                    fault: None,
                    has_loop: false,
                };
            }
        };
        self.load_program(&program)
    }

    /// 语言切换 / 换宿主二进制专用：只杀宿主进程并清空待重载程序。
    /// 与 restart_host 的区别：绝不拿旧代码在新宿主上重跑初始化——
    /// 换语言后旧代码对新宿主是外语，重跑只产出一次注定失败且被吞的
    /// init。切换后由调用方以新代码 load_code（docs 03：语言切换走宿主
    /// 重启，已提交 memory 保留）。
    pub fn drop_host_for_switch(&mut self) {
        self.drop_host();
        self.program = None;
    }

    /// 主进程主动终止（“终止按钮”路径）：直接杀进程，不经过 Game 队列。
    pub fn kill_host_now(&mut self) {
        self.killed_by_us.store(true, Ordering::SeqCst);
        if let Some(h) = &self.host {
            h.kill();
        }
    }

    /// 供测试从独立线程杀宿主（验证控制路径不排队在 Game 请求之后）。
    pub fn child_handle(&self) -> Option<Arc<Mutex<Child>>> {
        self.host.as_ref().map(|h| h.child.clone())
    }

    pub fn host_alive(&self) -> bool {
        self.host.as_ref().map(|h| !h.dead.load(Ordering::SeqCst)) == Some(true)
    }

    pub fn stderr_text(&self) -> String {
        self.host
            .as_ref()
            .map(|h| h.stderr_text())
            .unwrap_or_default()
    }

    fn drop_host(&mut self) {
        if let Some(h) = &self.host {
            h.kill();
        }
        self.host = None;
        self.host_stdin = None;
        self.host_stdout = None;
        // 请求号在宿主代次内单调；重启即新 id 空间。代次本身只增不回收
        // （host_epoch），旧代次帧永远对不上号。
        self.last_request_id = 0;
        self.exec_seen_request_id = 0;
        // 主动终止标记只在本代次内有效，防止误标后续无关故障。
        self.killed_by_us.store(false, Ordering::SeqCst);
    }

    fn next_exec_id(&mut self) -> u64 {
        self.execution_id += 1;
        self.execution_id
    }

    fn tick_budget_ms(&self) -> u64 {
        let entities = (self.world.robots.len()
            + self.world.shelves.len()
            + self.world.chargers.len()
            + self.world.docks.len()
            + self.world.vehicles.len()
            + self.world.ground_boxes.len()
            + self.world.listings.len()
            + self.world.my_orders.len()) as u64;
        let us =
            self.cfg.tick_budget_base_ms * 1000 + entities * self.cfg.tick_budget_per_entity_us;
        (us / 1000).min(self.cfg.tick_budget_cap_ms).max(1)
    }

    fn produce_mirror(&mut self) -> String {
        let t0 = Instant::now();
        let view = MirrorView::from_world(&self.world, self.revision.get());
        let json = view.to_json();
        self.stats
            .mirror_ser_us
            .push(t0.elapsed().as_micros() as u64);
        self.stats.mirror_bytes.push(json.len());
        self.mirror_cache = Some(json.clone());
        json
    }

    // -- 消息循环 ------------------------------------------------------------
    ///
    /// stdin / stdout 移出到局部变量，避免与 handle_op 的可变借用冲突——
    /// 世界线程本体是「直读帧 → 应答」的循环（执行期同步读宿主管道，
    /// 免去独立读线程的每次一跳线程转交），不嵌套阻塞调用宿主。
    fn run_exec(
        &mut self,
        exec_id: u64,
        kind: &str,
        budget_ms: u64,
        mirror: String,
        program: Option<&PlayerProgram>,
    ) -> ExecResult {
        // 本执行的去重缓存与计数清零（docs/architecture/03：请求结果
        // 保存在当前执行内，不跨执行）。已见号不重置：请求号跨执行
        // 单调，完成帧引用的是进程内绝对号。
        self.exec_dedup.clear();
        self.exec_dedup_bytes = 0;
        self.exec_new_requests = 0;
        self.exec_limit_hit = false;
        // 看门狗：预算 + 宽限期后直接终止宿主（独立线程，不经 Game 队列）。
        // 完成信号经 condvar 即时唤醒，避免快进时堆积沉睡线程，
        // 也消除「恰在宽限边界完成仍被杀」的窗口。
        let done = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let fired = Arc::new(AtomicBool::new(false));
        {
            let child = self.host.as_ref().expect("宿主在场").child.clone();
            let done = done.clone();
            let fired = fired.clone();
            let dead = self
                .host
                .as_ref()
                .map(|h| h.dead.clone())
                .expect("宿主在场");
            let grace_total = budget_ms + self.cfg.grace_ms;
            let _ = std::thread::Builder::new()
                .name("ztw-watchdog".into())
                .spawn(move || {
                    let (lock, cv) = &*done;
                    let Ok(mut g) = lock.lock() else { return };
                    let deadline = Duration::from_millis(grace_total);
                    let mut waited = Duration::ZERO;
                    while !*g {
                        if waited >= deadline {
                            break;
                        }
                        let before = std::time::Instant::now();
                        let (guard, timeout) = match cv.wait_timeout(g, deadline - waited) {
                            Ok(x) => x,
                            Err(_) => return,
                        };
                        g = guard;
                        waited += before.elapsed();
                        if timeout.timed_out() {
                            break;
                        }
                    }
                    if !*g {
                        fired.store(true, Ordering::SeqCst);
                        // 直读模式下无人再读管道，主动置 dead——宿主在
                        // 「Complete 已读出、done 锁未取得」窗口被杀时
                        // host_alive 不跨间隙撒谎。
                        dead.store(true, Ordering::SeqCst);
                        kill_and_reap(&child);
                    }
                });
        }
        let mut stdin = self.host_stdin.take().expect("宿主在场");
        let mut stdout = self.host_stdout.take().expect("宿主在场");
        let frame_limit = self.cfg.frame_limit as u64;
        let frame = MainFrame::Exec {
            v: crate::protocol::PROTOCOL_VERSION,
            host_epoch: self.host_epoch,
            execution_id: exec_id,
            kind: kind.to_string(),
            tick: self.world.tick,
            budget_ms,
            files: program.map(|p| p.files.clone()),
            entry: program.map(|p| p.entry.clone()),
            mirror: Some(raw_or_quoted(mirror)),
            memory_gen: self.memory_generation(),
        };
        let write_failed = write_frame(&mut stdin, &frame).is_err();
        let mut requests_served = 0u64;
        // 统一回复发送：写失败可能只是宿主刚被看门狗 / 终止按钮杀掉的
        // 下游症状，因果优先于症状归类（与写失败路径同一裁决）。
        macro_rules! send_reply {
            ($rid:expr, $result:expr) => {
                if write_frame(
                    &mut stdin,
                    &MainFrame::Reply {
                        v: crate::protocol::PROTOCOL_VERSION,
                        host_epoch: self.host_epoch,
                        execution_id: exec_id,
                        request_id: $rid,
                        result: $result,
                    },
                )
                .is_err()
                {
                    let watchdog = fired.load(Ordering::SeqCst);
                    let killed = self.killed_by_us.swap(false, Ordering::SeqCst);
                    let (reason, code, message) = if watchdog {
                        (
                            "watchdog",
                            "WATCHDOG_KILL",
                            "执行超预算与宽限期，主进程终止宿主",
                        )
                    } else if killed {
                        (
                            "killed",
                            "KILLED_BY_MAIN",
                            "主进程主动终止宿主（终止按钮路径）",
                        )
                    } else {
                        ("write", "REPLY_WRITE_FAILED", "回复写入失败")
                    };
                    break ExecResult::Faulted {
                        class: FaultClass::HostTerminated(reason),
                        code: code.into(),
                        message: format!(
                            "{message}（tick {}，最后已提交请求 #{}/{}）",
                            self.world.tick, self.last_request_id, self.last_op
                        ),
                        stack: String::new(),
                        stats: ExecStats::default(),
                        requests_served,
                        last_request_id: self.last_request_id,
                    };
                }
            };
        }
        let result = if write_failed {
            // 写失败只说明管道断了，不代表原因：SIGKILL 后宿主 fd 关闭与本次写
            // 的先后是竞态（快机器上写先成功再走 EOF，慢机器上直接 EPIPE）。
            // 与下方 EOF 路径同优先级归类，主动终止 / 看门狗的因果优先于症状。
            let watchdog = fired.load(Ordering::SeqCst);
            let killed = self.killed_by_us.swap(false, Ordering::SeqCst);
            let (reason, code, message) = if watchdog {
                (
                    "watchdog",
                    "WATCHDOG_KILL",
                    "执行超预算与宽限期，主进程终止宿主",
                )
            } else if killed {
                (
                    "killed",
                    "KILLED_BY_MAIN",
                    "主进程主动终止宿主（终止按钮路径）",
                )
            } else {
                ("write", "HOST_WRITE_FAILED", "写入执行请求失败")
            };
            ExecResult::Faulted {
                class: FaultClass::HostTerminated(reason),
                code: code.into(),
                message: format!(
                    "{message}（tick {}，最后已提交请求 #{}/{}）",
                    self.world.tick, self.last_request_id, self.last_op
                ),
                stack: String::new(),
                stats: ExecStats::default(),
                requests_served,
                last_request_id: self.last_request_id,
            }
        } else {
            loop {
                match read_frame::<_, HostFrame>(&mut stdout, frame_limit) {
                    Ok(HostFrame::Request {
                        v,
                        host_epoch,
                        execution_id,
                        request_id,
                        op,
                        payload,
                    }) => {
                        if v != crate::protocol::PROTOCOL_VERSION {
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "VERSION_MISMATCH".into(),
                                message: format!(
                                    "协议版本 {v} != {}",
                                    crate::protocol::PROTOCOL_VERSION
                                ),
                                stack: String::new(),
                                stats: ExecStats::default(),
                                requests_served,
                                last_request_id: self.last_request_id,
                            };
                        }
                        // 旧代次消息永远拒绝（docs/architecture/03：重启
                        // 递增 host_epoch）。代次对不上只可能是旧进程残留
                        // 或协议破坏，杀宿主。
                        if host_epoch != self.host_epoch {
                            if let Some(h) = &self.host {
                                h.kill();
                            }
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "STALE_EPOCH".into(),
                                message: format!(
                                    "宿主代次不符：期望 {}，得到 {host_epoch}",
                                    self.host_epoch
                                ),
                                stack: String::new(),
                                stats: ExecStats::default(),
                                requests_served,
                                last_request_id: self.last_request_id,
                            };
                        }
                        // 旧执行的消息：执行已关闭，拒绝且不执行；宿主继续
                        // 走它自己的终态（docs 03：执行关闭后拒绝全部旧消息）。
                        if execution_id != exec_id {
                            // 拒绝也构成“已见”：宿主发出即消耗该请求号，
                            // 完成帧的 last_request_id 引用它。不推进则
                            // 捕获异常后继续发号的宿主会被 REQUEST_ID_GAP /
                            // LAST_REQUEST_MISMATCH 误杀——可恢复拒绝被
                            // 升格成协议故障。不入 exec_dedup：旧执行请求
                            // 重发仍先命中本执行号检查，天然幂等。
                            self.exec_seen_request_id = self.exec_seen_request_id.max(request_id);
                            let reply = err_result(
                                "EXEC_CLOSED",
                                &format!("执行已关闭：拒绝旧执行 #{execution_id} 的消息"),
                            );
                            send_reply!(request_id, reply);
                            continue;
                        }
                        // 指纹 =（op, payload）零键 SipHash-1-3 哈希。信任
                        // 假设：宿主生产路径从不重发请求号（FIFO 串行），
                        // 同号异负载只能来自宿主自身 bug 或恶意构造——随机
                        // 碰撞界 ~n²/2⁶⁵（n ≤ 请求上限 + 1024），可忽略；
                        // 精确串比对的成本在此热路径不划算。
                        let mut fp_hash = std::collections::hash_map::DefaultHasher::new();
                        op.hash(&mut fp_hash);
                        payload.get().hash(&mut fp_hash);
                        let fingerprint = fp_hash.finish();
                        if request_id == self.exec_seen_request_id + 1 {
                            // 新请求（无论受理还是拒绝，都已“见到”）。
                            self.exec_seen_request_id = request_id;
                            // 新请求。达到每执行请求数上限即暂停报错
                            // （docs 03），此后应答错误而不执行，直到
                            // 宿主自行走到终态帧。拒绝结果同样入缓存
                            // （条目与字节双上界，见 dedup_put）：预算
                            // 内重发被拒请求得到同一拒绝（幂等）；预算
                            // 耗尽后未缓存的拒绝号按协议故障降级。
                            if self.exec_new_requests >= self.cfg.request_limit_per_exec {
                                self.exec_limit_hit = true;
                                let reply = err_result(
                                    "REQUEST_LIMIT",
                                    &format!(
                                        "本执行请求数已达上限 {}，执行将被暂停",
                                        self.cfg.request_limit_per_exec
                                    ),
                                );
                                self.dedup_put(
                                    request_id,
                                    fingerprint,
                                    op.len() + payload.get().len(),
                                    &reply,
                                );
                                send_reply!(request_id, reply);
                                continue;
                            }
                            self.last_request_id = request_id;
                            self.last_op = op.clone();
                            let t0 = Instant::now();
                            let reply = self.handle_op(&op, &payload);
                            self.stats
                                .op_us
                                .push(t0.elapsed().as_micros().max(1) as u64);
                            requests_served += 1;
                            self.exec_new_requests += 1;
                            self.dedup_put(
                                request_id,
                                fingerprint,
                                op.len() + payload.get().len(),
                                &reply,
                            );
                            send_reply!(request_id, reply);
                        } else if request_id <= self.exec_seen_request_id {
                            // 重复请求号：同负载返回原结果、不重复执行；
                            // 异负载或缓存缺失为协议故障（docs 03）。
                            // 已见号（含被拒请求）之内的重发都走此分支。
                            let cached = self.exec_dedup.get(&request_id);
                            match cached {
                                Some((fp, result)) if *fp == fingerprint => {
                                    let result = result.clone();
                                    send_reply!(request_id, result);
                                }
                                Some(_) => {
                                    if let Some(h) = &self.host {
                                        h.kill();
                                    }
                                    break ExecResult::Faulted {
                                        class: FaultClass::HostTerminated("protocol"),
                                        code: "DUP_REQUEST_MISMATCH".into(),
                                        message: format!(
                                            "重复请求 #{request_id} 同号异负载，op={op}"
                                        ),
                                        stack: String::new(),
                                        stats: ExecStats::default(),
                                        requests_served,
                                        last_request_id: self.last_request_id,
                                    };
                                }
                                None => {
                                    if let Some(h) = &self.host {
                                        h.kill();
                                    }
                                    break ExecResult::Faulted {
                                        class: FaultClass::HostTerminated("protocol"),
                                        code: "DUP_REQUEST_MISMATCH".into(),
                                        message: format!(
                                            "重复请求 #{request_id} 不在本执行结果缓存内，op={op}"
                                        ),
                                        stack: String::new(),
                                        stats: ExecStats::default(),
                                        requests_served,
                                        last_request_id: self.last_request_id,
                                    };
                                }
                            }
                        } else {
                            // 跳变（大于已见+1）：请求号缺口即协议故障。
                            if let Some(h) = &self.host {
                                h.kill();
                            }
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "REQUEST_ID_GAP".into(),
                                message: format!(
                                    "请求号跳变：期望 {}，得到 {request_id}",
                                    self.exec_seen_request_id + 1
                                ),
                                stack: String::new(),
                                stats: ExecStats::default(),
                                requests_served,
                                last_request_id: self.last_request_id,
                            };
                        }
                    }
                    Ok(HostFrame::Complete {
                        v,
                        host_epoch,
                        execution_id,
                        last_request_id,
                        has_loop,
                        stats,
                    }) => {
                        if v != crate::protocol::PROTOCOL_VERSION {
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "VERSION_MISMATCH".into(),
                                message: format!(
                                    "协议版本 {v} != {}",
                                    crate::protocol::PROTOCOL_VERSION
                                ),
                                stack: String::new(),
                                stats,
                                requests_served,
                                last_request_id,
                            };
                        }
                        if host_epoch != self.host_epoch {
                            if let Some(h) = &self.host {
                                h.kill();
                            }
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "STALE_EPOCH".into(),
                                message: format!(
                                    "宿主代次不符：期望 {}，得到 {host_epoch}",
                                    self.host_epoch
                                ),
                                stack: String::new(),
                                stats,
                                requests_served,
                                last_request_id,
                            };
                        }
                        // 旧执行的残留完成消息：丢弃不结算——正常完成与
                        // 强制关闭竞态只结算一次（docs 08 A1 验收）。
                        if execution_id != exec_id {
                            continue;
                        }
                        // 完成帧引用宿主的最后发送号，与本执行“已见”号
                        // 对账——被拒请求见过但未执行，也在计数内。
                        if last_request_id != self.exec_seen_request_id {
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "LAST_REQUEST_MISMATCH".into(),
                                message: format!(
                                    "完成消息引用 {last_request_id}，已见到 {}（其中已执行至 {}）",
                                    self.exec_seen_request_id, self.last_request_id
                                ),
                                stack: String::new(),
                                stats,
                                requests_served,
                                last_request_id,
                            };
                        }
                        self.stats.last_exec = stats.clone();
                        // 请求上限触及后宿主“正常完成”的结局改判为超限
                        // 暂停（docs 03：达到上限暂停报错，脚本级可恢复）。
                        if self.exec_limit_hit {
                            break ExecResult::Faulted {
                                class: FaultClass::Script,
                                code: "REQUEST_LIMIT".into(),
                                message: format!(
                                    "本执行请求数达到上限 {}，暂停（tick {}，最后已提交请求 #{}/{}）",
                                    self.cfg.request_limit_per_exec,
                                    self.world.tick,
                                    self.last_request_id,
                                    self.last_op
                                ),
                                stack: String::new(),
                                stats,
                                requests_served,
                                last_request_id,
                            };
                        }
                        break ExecResult::Complete {
                            has_loop,
                            stats,
                            requests_served,
                        };
                    }
                    Ok(HostFrame::Fault {
                        v,
                        host_epoch,
                        execution_id,
                        class,
                        code,
                        message,
                        stack,
                        last_request_id,
                        stats,
                    }) => {
                        let _ = v; // 故障帧不因版本差异拒收（诊断优先）
                        if host_epoch != self.host_epoch {
                            if let Some(h) = &self.host {
                                h.kill();
                            }
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "STALE_EPOCH".into(),
                                message: format!(
                                    "宿主代次不符：期望 {}，得到 {host_epoch}（原故障 {code}）",
                                    self.host_epoch
                                ),
                                stack: String::new(),
                                stats,
                                requests_served,
                                last_request_id,
                            };
                        }
                        // 旧执行的残留故障消息：丢弃不结算，等当前执行
                        // 自己的终态（或看门狗超时）。
                        if execution_id != exec_id {
                            continue;
                        }
                        self.stats.last_exec = stats.clone();
                        let fc = match class.as_str() {
                            "script" => FaultClass::Script,
                            "environment" => FaultClass::Environment,
                            _ => FaultClass::HostTerminated("protocol"),
                        };
                        // 触限后脚本未捕获绑定层抛错、直接冒泡成 Fault 帧
                        // 的，结局同样改判为超限暂停——与完成帧路径同款语义
                        //（docs 03：触限即 REQUEST_LIMIT，脚本级可恢复）。
                        // 宿主故障帧只报异常名不携带绑定层错误码，改判由
                        // 主进程执行；原始错误保留在 message 供诊断。
                        if self.exec_limit_hit && matches!(fc, FaultClass::Script) {
                            break ExecResult::Faulted {
                                class: FaultClass::Script,
                                code: "REQUEST_LIMIT".into(),
                                message: format!(
                                    "本执行请求数达到上限 {}（原故障 {code}：{message}）",
                                    self.cfg.request_limit_per_exec
                                ),
                                stack,
                                stats,
                                requests_served,
                                last_request_id,
                            };
                        }
                        break ExecResult::Faulted {
                            class: fc,
                            code,
                            message,
                            stack,
                            stats,
                            requests_served,
                            last_request_id,
                        };
                    }
                    Err(crate::protocol::FrameError::Io(_))
                    | Err(crate::protocol::FrameError::Closed) => {
                        if let Some(h) = &self.host {
                            h.dead.store(true, Ordering::SeqCst);
                        }
                        let watchdog = fired.load(Ordering::SeqCst);
                        let killed = self.killed_by_us.swap(false, Ordering::SeqCst);
                        let (reason, code) = if watchdog {
                            ("watchdog", "WATCHDOG_KILL")
                        } else if killed {
                            ("killed", "KILLED_BY_MAIN")
                        } else {
                            ("crash", "HOST_EXITED")
                        };
                        let stderr = self.stderr_text();
                        let close_point = format!(
                            "（tick {}，最后已提交请求 #{}/{}）",
                            self.world.tick, self.last_request_id, self.last_op
                        );
                        break ExecResult::Faulted {
                            class: FaultClass::HostTerminated(reason),
                            code: code.to_string(),
                            message: if watchdog {
                                format!(
                                    "执行超预算与宽限期，主进程终止宿主；代码可能在循环中捕获了所有异常或陷入不可中断运算{close_point}"
                                )
                            } else if killed {
                                format!("主进程主动终止宿主（终止按钮路径）{close_point}")
                            } else {
                                format!("宿主进程退出：{stderr}{close_point}")
                            },
                            stack: String::new(),
                            stats: ExecStats::default(),
                            requests_served,
                            last_request_id: self.last_request_id,
                        };
                    }
                    Err(e) => {
                        if let Some(h) = &self.host {
                            h.dead.store(true, Ordering::SeqCst);
                            h.kill();
                        }
                        let msg = e.to_string();
                        break ExecResult::Faulted {
                            class: FaultClass::HostTerminated("protocol"),
                            code: "BAD_FRAME".into(),
                            message: format!("非法或超大帧：{msg}"),
                            stack: String::new(),
                            stats: ExecStats::default(),
                            requests_served,
                            last_request_id: self.last_request_id,
                        };
                    }
                }
            }
        };
        {
            let (lock, cv) = &*done;
            if let Ok(mut g) = lock.lock() {
                *g = true;
                cv.notify_all();
            }
        }
        // 宿主句柄仍在才放回管道（判据是 host.is_some：死亡路径只置
        // dead 标志，由调用方对 HostTerminated 的 drop_host 统一收尾）。
        if self.host.is_some() {
            self.host_stdin = Some(stdin);
            self.host_stdout = Some(stdout);
        }
        result
    }

    /// 去重缓存写入：条目（request_limit_per_exec + 1024）与字节
    /// （dedup_cache_bytes）双上界。字节预算耗尽后跳过缓存——请求本身
    /// 照常执行并应答，此后该号重发无缓存可回放，按既有
    /// DUP_REQUEST_MISMATCH 协议故障降级（docs 03 已文档化的可接受
    /// 角落），主进程不随请求风暴无界堆积镜像级结果。命中才克隆：
    /// 跳过路径恰是 MB 级镜像回复的风暴场景，不做无谓拷贝。指纹为
    /// （op, payload）增量哈希 u64；字节记账按请求 + 回复口径（超长
    /// 日志场景的耗尽语义不变）。
    fn dedup_put(&mut self, request_id: u64, fingerprint: u64, req_bytes: usize, reply: &RawValue) {
        if self.exec_dedup.len() >= self.cfg.request_limit_per_exec as usize + 1024 {
            return;
        }
        let bytes = self
            .exec_dedup_bytes
            .saturating_add(req_bytes)
            .saturating_add(reply.get().len());
        if bytes > self.cfg.dedup_cache_bytes {
            return;
        }
        self.exec_dedup_bytes = bytes;
        self.exec_dedup
            .insert(request_id, (fingerprint, reply.to_owned()));
    }

    fn memory_generation(&self) -> u64 {
        match (&self.in_init, &self.init_branch) {
            (true, Some(branch)) => branch.generation(),
            _ => self.memory.generation(),
        }
    }

    // 结构不变量（被两侧绑定层的记忆槽位缓存依赖）：主进程对玩家
    // memory 树的结构性改动只有 init 的 fork/commit 路径（代次 +1，宿主
    // 侧随代次重建缓存）；执行期服务端不替换 / 不删除玩家容器节点
    //（robots/<id> 的保留标量 _move 为纯标量写）。若未来引入服务端侧
    // 结构清理（如 destroy 清 robots 记录），必须同步提供宿主可观测的
    // 失效信号（代次或专用 op），否则宿主缓存会跨 tick 返回死句柄。
    fn memory_target_mut(&mut self) -> &mut MemoryTree {
        match (&self.in_init, &mut self.init_branch) {
            (true, Some(branch)) => branch,
            (true, None) => {
                // 内部不变量：初始化执行期间临时分支必须存在。静默落到
                // 主树会破坏“初始化成功才提交”，宁可暴露 bug。
                panic!("初始化执行期间临时 memory 分支缺失");
            }
            _ => &mut self.memory,
        }
    }

    // -- Game 请求处理 -------------------------------------------------------

    /// 初始化阶段禁用动作与管理操作（docs/architecture/03），统一记录诊断。
    fn reject_init_phase(&mut self, op: &str, subject: Option<ztw_model::Id>) -> Box<RawValue> {
        self.diag_push(
            DiagTapKind::AcceptFail,
            op,
            ztw_model::codes::INIT_PHASE,
            subject,
            "初始化阶段禁止动作与管理操作".to_string(),
        );
        ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}))
    }

    /// 动作受理结果记录：受理失败进诊断；成功受理的最终成败由结算事件
    /// （last_results）承担，不重复记录。
    fn diag_accept(&mut self, op: &str, code: &str, robot_id: ztw_model::Id, detail: String) {
        if code != ztw_model::codes::OK {
            self.diag_push(DiagTapKind::AcceptFail, op, code, Some(robot_id), detail);
        }
    }

    fn handle_op(&mut self, op: &str, payload: &RawValue) -> Box<RawValue> {
        macro_rules! parse {
            () => {
                match serde_json::from_str::<serde_json::Value>(payload.get()) {
                    Ok(v) => v,
                    Err(e) => return err_result("BAD_PAYLOAD", &e.to_string()),
                }
            };
        }
        match op {
            "robot.move" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("robot.move", p["robot_id"].as_u64());
                }
                let (Some(robot_id), Some(dx), Some(dy)) =
                    (p["robot_id"].as_u64(), p["dx"].as_i64(), p["dy"].as_i64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.move 参数缺失");
                };
                let code = self.world.accept_move(robot_id, dx as i32, dy as i32);
                self.diag_accept("robot.move", code, robot_id, format!("dx={dx},dy={dy}"));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.charge" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("robot.charge", p["robot_id"].as_u64());
                }
                let Some(robot_id) = p["robot_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "robot.charge 参数缺失");
                };
                let code = self.world.accept_charge(robot_id);
                self.diag_accept("robot.charge", code, robot_id, String::new());
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.take" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("robot.take", p["robot_id"].as_u64());
                }
                let (Some(robot_id), Some(target_id), Some(box_id)) = (
                    p["robot_id"].as_u64(),
                    p["target_id"].as_u64(),
                    p["box_id"].as_u64(),
                ) else {
                    return err_result("BAD_PAYLOAD", "robot.take 参数缺失");
                };
                let code = self.world.accept_take(robot_id, target_id, box_id);
                self.diag_accept("robot.take", code, robot_id, format!("box={box_id}"));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.give" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("robot.give", p["robot_id"].as_u64());
                }
                let (Some(robot_id), Some(target_id)) =
                    (p["robot_id"].as_u64(), p["target_id"].as_u64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.give 参数缺失");
                };
                let box_id = p["box_id"].as_u64(); // 缺省 = 当前携带物
                let code = self.world.accept_give(robot_id, target_id, box_id);
                self.diag_accept("robot.give", code, robot_id, format!("box={box_id:?}"));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.pick" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("robot.pick", p["robot_id"].as_u64());
                }
                let (Some(robot_id), Some(x), Some(y)) =
                    (p["robot_id"].as_u64(), p["x"].as_i64(), p["y"].as_i64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.pick 参数缺失");
                };
                let code = self.world.accept_pick(robot_id, x as i32, y as i32);
                self.diag_accept("robot.pick", code, robot_id, format!("x={x},y={y}"));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.drop" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("robot.drop", p["robot_id"].as_u64());
                }
                let (Some(robot_id), Some(x), Some(y)) =
                    (p["robot_id"].as_u64(), p["x"].as_i64(), p["y"].as_i64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.drop 参数缺失");
                };
                let box_id = p["box_id"].as_u64(); // 缺省 = 当前携带物
                let code = self.world.accept_drop(robot_id, x as i32, y as i32, box_id);
                self.diag_accept("robot.drop", code, robot_id, format!("x={x},y={y}"));
                ok_result(serde_json::json!({ "code": code }))
            }
            "market.take" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("market.take", p["order_id"].as_u64());
                }
                let Some(order_id) = p["order_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "market.take 参数缺失");
                };
                let (code, eff) = self.world.manage_take(order_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        "market.take",
                        code,
                        Some(order_id),
                        format!(
                            "接单 {} {}×{} @{} milli，装卸位 #{}，余额 {} milli",
                            eff.order.side.as_str(),
                            eff.order.goods_type,
                            eff.order.qty,
                            eff.order.unit_price_milli,
                            eff.dock_id,
                            self.world.gold_milli
                        ),
                    );
                    let delta = MirrorDelta::from_take(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        "market.take",
                        code,
                        Some(order_id),
                        String::new(),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "market.cancel" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("market.cancel", p["order_id"].as_u64());
                }
                let Some(order_id) = p["order_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "market.cancel 参数缺失");
                };
                let (code, eff) = self.world.manage_cancel(order_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        "market.cancel",
                        code,
                        Some(order_id),
                        format!(
                            "取消订单，手续费 {} milli，退款 {} milli，移除车辆 {:?}，释放装卸位 {:?}",
                            eff.fee_milli, eff.refund_milli, eff.vehicle_id, eff.dock_id
                        ),
                    );
                    let delta = MirrorDelta::from_cancel(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        "market.cancel",
                        code,
                        Some(order_id),
                        String::new(),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.destroy" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("manage.destroy", p["target_id"].as_u64());
                }
                let Some(target_id) = p["target_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "manage.destroy 参数缺失");
                };
                let (code, eff) = self.world.manage_destroy(target_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        "manage.destroy",
                        code,
                        Some(target_id),
                        format!(
                            "销毁 {:?} #{}，退款 {} milli，释放格 {:?}",
                            eff.kind, eff.target_id, eff.refund_milli, eff.freed_cells
                        ),
                    );
                    let delta = MirrorDelta::from_destroy(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        "manage.destroy",
                        code,
                        Some(target_id),
                        String::new(),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.borrow" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("manage.borrow", None);
                }
                let Some(amount_milli) = p["amount_milli"].as_i64() else {
                    return err_result("BAD_PAYLOAD", "manage.borrow 参数缺失");
                };
                let (code, eff) = self.world.manage_borrow(amount_milli);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        "manage.borrow",
                        code,
                        None,
                        format!(
                            "借款 {} milli，余额 {} milli，欠款 {} milli",
                            eff.amount_milli, self.world.gold_milli, self.world.debt_milli
                        ),
                    );
                    let delta = MirrorDelta::from_borrow(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        "manage.borrow",
                        code,
                        None,
                        format!("amount={amount_milli}"),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.repay" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("manage.repay", None);
                }
                let Some(amount_milli) = p["amount_milli"].as_i64() else {
                    return err_result("BAD_PAYLOAD", "manage.repay 参数缺失");
                };
                let (code, eff) = self.world.manage_repay(amount_milli);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        "manage.repay",
                        code,
                        None,
                        format!(
                            "归还 {} milli，余额 {} milli，欠款 {} milli",
                            eff.amount_milli, self.world.gold_milli, self.world.debt_milli
                        ),
                    );
                    let delta = MirrorDelta::from_repay(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        "manage.repay",
                        code,
                        None,
                        format!("amount={amount_milli}"),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.buy" => {
                let p = parse!();
                if self.in_init {
                    return self.reject_init_phase("manage.buy", None);
                }
                let (Some(kind), Some(x), Some(y)) =
                    (p["kind"].as_str(), p["x"].as_i64(), p["y"].as_i64())
                else {
                    return err_result("BAD_PAYLOAD", "manage.buy 参数缺失");
                };
                let (code, eff) = self.world.manage_buy(kind, x as i32, y as i32);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        "manage.buy",
                        code,
                        Some(eff.id),
                        format!(
                            "购入 {:?} #{} @({},{})，支出 {} milli，余额 {} milli",
                            eff.kind,
                            eff.id,
                            eff.pos.x,
                            eff.pos.y,
                            eff.price_milli,
                            self.world.gold_milli
                        ),
                    );
                    let delta = MirrorDelta::from_buy(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        "manage.buy",
                        code,
                        None,
                        format!("kind={kind} at({x},{y})"),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "log" => {
                let p = parse!();
                let Some(line) = p["line"].as_str() else {
                    return err_result("BAD_PAYLOAD", "log 参数缺失");
                };
                if line.len() > self.cfg.log_entry_limit {
                    return err_result(
                        "LOG_LIMIT",
                        &format!(
                            "日志条目 {}B 超上限 {}B",
                            line.len(),
                            self.cfg.log_entry_limit
                        ),
                    );
                }
                self.logs.push_back((self.world.tick, line.to_string()));
                while self.logs.len() > self.cfg.log_ring_cap {
                    self.logs.pop_front();
                }
                ok_result(serde_json::json!({}))
            }
            "mirror.fetch" => {
                // 回退重建必须反映当前世界（含本 tick 已生效管理操作），
                // 不能复用 tick 开始的旧镜像——镜像是世界状态的纯函数
                //（docs/architecture/03 宿主本地查询镜像）。v4 起镜像作为
                // 原始 JSON 值内嵌（服务端 serde 产物，拼接安全），绑定层
                // 免一层 stringify+parse。
                let mirror = self.produce_mirror();
                raw_or_quoted(format!("{{\"ok\":true,\"mirror\":{mirror}}}"))
            }
            _ if op.starts_with("mem.") => self.handle_mem_op(op, payload),
            _ => err_result("UNKNOWN_OP", &format!("未知操作 {op}")),
        }
    }

    /// 管理操作成功路径的统一回复：递增世界修订号并携带镜像同步增量
    /// （docs/architecture/03 管理操作当 tick 可见）。
    fn mgmt_ok(&mut self, delta: MirrorDelta) -> Box<RawValue> {
        self.revision.mgmt_count += 1;
        let delta_json = delta.to_json();
        self.stats.delta_bytes.push(delta_json.len());
        let delta_val: serde_json::Value =
            serde_json::from_str(&delta_json).unwrap_or(serde_json::Value::Null);
        ok_result(serde_json::json!({ "code": ztw_model::codes::OK, "delta": delta_val }))
    }

    fn handle_mem_op(&mut self, op: &str, payload: &RawValue) -> Box<RawValue> {
        let p: serde_json::Value = match serde_json::from_str(payload.get()) {
            Ok(v) => v,
            Err(e) => return err_result("BAD_PAYLOAD", &e.to_string()),
        };
        let session_gen = p["gen"].as_u64().unwrap_or(u64::MAX);
        let node = p["node"].as_u64().unwrap_or(u64::MAX);
        let key = p["key"].as_str();
        let index = p["index"].as_u64().map(|i| i as usize);
        let needs_key = matches!(
            op,
            "mem.map_set" | "mem.map_delete" | "mem.map_has" | "mem.map_get"
        );
        let needs_index = matches!(op, "mem.list_get" | "mem.list_set" | "mem.list_remove");
        let needs_robot = op == "mem.robot_memory";
        if (needs_key && key.is_none())
            || (needs_index && index.is_none())
            || (needs_robot && !p.get("robot_id").map(|v| v.is_u64()).unwrap_or(false))
        {
            return err_result("BAD_PAYLOAD", &format!("{op} 缺少必需参数"));
        }
        let needs_value = matches!(op, "mem.map_set" | "mem.list_set" | "mem.list_append");
        let value: Option<MemValue> = if needs_value {
            match serde_json::from_value(p["value"].clone()) {
                Ok(v) => Some(v),
                Err(_) => return err_result("BAD_PAYLOAD", "memory 写入值非法（线值解码失败）"),
            }
        } else {
            None
        };
        let robot_id = p["robot_id"].as_u64();
        let tree = self.memory_target_mut();
        let mut run = || -> Result<serde_json::Value, crate::memory::MemOpError> {
            match op {
                "mem.map_get" => Ok(read_result_json(tree.map_get(
                    session_gen,
                    node,
                    key.unwrap_or(""),
                )?)),
                "mem.map_set" => {
                    tree.map_set(
                        session_gen,
                        node,
                        key.unwrap_or(""),
                        value.as_ref().unwrap(),
                    )?;
                    Ok(serde_json::json!({}))
                }
                "mem.map_delete" => {
                    let removed = tree.map_delete(session_gen, node, key.unwrap_or(""))?;
                    Ok(serde_json::json!({ "value": removed }))
                }
                "mem.map_has" => Ok(
                    serde_json::json!({ "value": tree.map_has(session_gen, node, key.unwrap_or(""))? }),
                ),
                "mem.map_size" => {
                    Ok(serde_json::json!({ "value": tree.map_size(session_gen, node)? }))
                }
                "mem.map_keys" => {
                    Ok(serde_json::json!({ "keys": tree.map_keys(session_gen, node)? }))
                }
                "mem.list_get" => Ok(read_result_json(tree.list_get(
                    session_gen,
                    node,
                    index.unwrap_or(usize::MAX),
                )?)),
                "mem.list_set" => {
                    tree.list_set(
                        session_gen,
                        node,
                        index.unwrap_or(usize::MAX),
                        value.as_ref().unwrap(),
                    )?;
                    Ok(serde_json::json!({}))
                }
                "mem.list_append" => {
                    tree.list_append(session_gen, node, value.as_ref().unwrap())?;
                    Ok(serde_json::json!({}))
                }
                "mem.list_remove" => {
                    tree.list_remove(session_gen, node, index.unwrap_or(usize::MAX))?;
                    Ok(serde_json::json!({}))
                }
                "mem.list_size" => {
                    Ok(serde_json::json!({ "value": tree.list_size(session_gen, node)? }))
                }
                "mem.list_entries" => Ok(serde_json::json!({
                    "entries": tree
                        .list_entries(session_gen, node)?
                        .into_iter()
                        .map(read_result_json)
                        .collect::<Vec<_>>()
                })),
                "mem.to_value" => Ok(serde_json::json!({
                    "value": serde_json::to_value(tree.to_value(session_gen, node)?)
                        .unwrap_or(serde_json::Value::Null)
                })),
                "mem.robot_memory" => {
                    let n = tree.robot_memory(session_gen, robot_id.unwrap_or(0))?;
                    Ok(serde_json::json!({ "node": n }))
                }
                _ => Err(crate::memory::MemOpError {
                    code: "UNKNOWN_OP",
                    message: format!("未知 memory 操作 {op}"),
                }),
            }
        };
        match run() {
            Ok(fields) => ok_result(fields),
            Err(e) => err_result(e.code, &e.message),
        }
    }

    /// memory 快照（根整树）。
    pub fn memory_snapshot(&mut self) -> MemValue {
        self.memory.snapshot()
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory.bytes()
    }
}

/// 标量读取返回普通 JSON 值；容器读取返回句柄描述。
fn read_result_json(r: ReadResult) -> serde_json::Value {
    match r {
        ReadResult::Scalar(v) => serde_json::json!({ "t": "scalar", "v": scalar_plain(&v) }),
        ReadResult::Handle { node, kind } => serde_json::json!({
            "t": "handle",
            "node": node,
            "kind": if kind == NodeKind::Map { "map" } else { "list" },
        }),
        ReadResult::Missing => serde_json::json!({ "t": "missing" }),
    }
}

fn scalar_plain(v: &MemValue) -> serde_json::Value {
    match v {
        MemValue::Null => serde_json::Value::Null,
        MemValue::Bool(b) => serde_json::Value::Bool(*b),
        MemValue::Num(n) => serde_json::json!(n),
        MemValue::Str(s) => serde_json::json!(s),
        _ => serde_json::Value::Null,
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.drop_host();
    }
}
