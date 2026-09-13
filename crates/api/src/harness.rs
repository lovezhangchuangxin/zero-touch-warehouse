//! A0 headless 主进程替身。扮演 docs/architecture/01 中 `crates/desktop`
//! 的宿主生命周期管理与世界线程角色，仅供 `cargo test` 与量测使用，
//! 不属于正式发布的主进程。
//!
//! 世界线程按消息循环实现（docs/architecture/03）：发出执行请求后只处理
//! 四类事件——Game 请求应答、执行完成、执行故障、宿主终止（EOF/坏帧）。
//! 看门狗在独立线程对执行计时，超宽限期直接终止宿主进程，不经过
//! Game 请求队列。

use std::collections::VecDeque;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ztw_model::MemValue;
use ztw_sim::World;

use crate::memory::{MemoryLimits, MemoryTree, NodeKind, ReadResult};
use crate::mirror::{MirrorDelta, MirrorView, WorldRevision};
use crate::protocol::{
    ExecStats, HostFrame, MainFrame, err_result, ok_result, read_frame, write_frame,
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
            env: Vec::new(),
        }
    }

    /// 故障注入（宿主测试钩子，见 runtime main.rs 的 ZTW_FAULT）。
    pub fn with_fault(mut self, faults: &str) -> SessionConfig {
        self.env.push(("ZTW_FAULT".to_string(), faults.to_string()));
        self
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

enum Ev {
    Frame(HostFrame),
    Eof,
    Invalid(String),
}

struct HostProc {
    child: Arc<Mutex<Child>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    /// 读线程见到 EOF / 坏帧（进程退出）。
    dead: Arc<AtomicBool>,
    #[allow(dead_code)]
    reader: JoinHandle<()>,
    #[allow(dead_code)]
    stderr_thread: JoinHandle<()>,
}

impl HostProc {
    fn spawn(cfg: &SessionConfig) -> std::io::Result<(HostProc, ChildStdin, mpsc::Receiver<Ev>)> {
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
        let (tx, rx) = mpsc::channel();
        let limit = cfg.frame_limit as u64;
        let dead = Arc::new(AtomicBool::new(false));
        let dead_r = dead.clone();
        let reader = std::thread::Builder::new()
            .name("ztw-host-reader".into())
            .spawn(move || {
                let mut stdout = stdout;
                loop {
                    match read_frame::<_, HostFrame>(&mut stdout, limit) {
                        Ok(f) => {
                            if tx.send(Ev::Frame(f)).is_err() {
                                break;
                            }
                        }
                        Err(crate::protocol::FrameError::Io(_))
                        | Err(crate::protocol::FrameError::Closed) => {
                            dead_r.store(true, Ordering::SeqCst);
                            let _ = tx.send(Ev::Eof);
                            break;
                        }
                        Err(e) => {
                            dead_r.store(true, Ordering::SeqCst);
                            let _ = tx.send(Ev::Invalid(e.to_string()));
                            break;
                        }
                    }
                }
            })
            .expect("spawn reader");
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
            reader,
            stderr_thread,
        };
        Ok((proc, stdin, rx))
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
// Session
// ---------------------------------------------------------------------------

pub struct Session {
    pub cfg: SessionConfig,
    pub world: World,
    pub memory: MemoryTree,
    pub logs: VecDeque<(u64, String)>,
    host: Option<HostProc>,
    host_stdin: Option<ChildStdin>,
    host_rx: Option<mpsc::Receiver<Ev>>,
    code: Option<String>,
    pub initialized: bool,
    pub fault: Option<FaultRecord>,
    execution_id: u64,
    last_request_id: u64,
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
            host_rx: None,
            code: None,
            initialized: false,
            fault: None,
            execution_id: 0,
            last_request_id: 0,
            revision: WorldRevision::default(),
            mirror_cache: None,
            stats: SessionStats::default(),
            init_branch: None,
            in_init: false,
            killed_by_us: Arc::new(AtomicBool::new(false)),
            last_op: String::new(),
        }
    }

    // -- 对外：初始化 / tick / 恢复 ------------------------------------------

    /// 加载玩家源码：启动（或复用）宿主并运行初始化执行。
    /// 失败时世界与原 memory 不变（docs/architecture/06）。
    pub fn load_code(&mut self, code: &str) -> InitOutcome {
        if self.host.is_none() {
            match HostProc::spawn(&self.cfg) {
                Ok((proc, stdin, rx)) => {
                    self.host = Some(proc);
                    self.host_stdin = Some(stdin);
                    self.host_rx = Some(rx);
                }
                Err(e) => {
                    return InitOutcome {
                        ok: false,
                        fault: Some(FaultRecord {
                            class: FaultClass::HostTerminated("spawn"),
                            code: "SPAWN_FAILED".into(),
                            message: e.to_string(),
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
        self.code = Some(code.to_string());
        self.run_init(code)
    }

    fn run_init(&mut self, code: &str) -> InitOutcome {
        self.init_branch = Some(self.memory.fork());
        self.in_init = true;
        let mirror = self.produce_mirror();
        let exec_id = self.next_exec_id();
        let result = self.run_exec(exec_id, "init", self.cfg.init_budget_ms, mirror, Some(code));
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
        let settle_map = self.world.settle();
        let settle: Vec<(ztw_model::Id, String)> = settle_map
            .iter()
            .map(|(id, r)| (*id, r.code.clone()))
            .collect();
        // 阶段 5：收尾（计息 / 渲染快照不在 A0）。
        self.revision.tick = self.world.tick;
        self.world.end_tick();
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

    /// 环境级故障恢复：宿主内销毁重建执行环境，重新初始化同一源码。
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
        let code = self.code.clone().expect("有已加载源码");
        self.run_init(&code)
    }

    /// 宿主终止恢复：重启宿主进程并重新初始化。
    pub fn restart_host(&mut self) -> InitOutcome {
        self.drop_host();
        self.stats.host_restarts += 1;
        let code = match &self.code {
            Some(c) => c.clone(),
            None => {
                return InitOutcome {
                    ok: false,
                    fault: None,
                    has_loop: false,
                };
            }
        };
        self.load_code(&code)
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
        self.host_rx = None;
        // 请求号在宿主代次内单调；重启即新 id 空间（host_epoch 属 A1）。
        self.last_request_id = 0;
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
            + self.world.ports.len()
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
    /// rx / stdin 移出到局部变量，避免与 handle_op 的可变借用冲突——
    /// 世界线程本体只是「收事件 → 应答」的循环，不嵌套阻塞调用宿主。
    fn run_exec(
        &mut self,
        exec_id: u64,
        kind: &str,
        budget_ms: u64,
        mirror: String,
        source: Option<&str>,
    ) -> ExecResult {
        // 看门狗：预算 + 宽限期后直接终止宿主（独立线程，不经 Game 队列）。
        // 完成信号经 condvar 即时唤醒，避免快进时堆积沉睡线程，
        // 也消除「恰在宽限边界完成仍被杀」的窗口。
        let done = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let fired = Arc::new(AtomicBool::new(false));
        {
            let child = self.host.as_ref().expect("宿主在场").child.clone();
            let done = done.clone();
            let fired = fired.clone();
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
                        kill_and_reap(&child);
                    }
                });
        }
        let mut stdin = self.host_stdin.take().expect("宿主在场");
        let rx = self.host_rx.take().expect("宿主在场");
        let frame = MainFrame::Exec {
            v: crate::protocol::PROTOCOL_VERSION,
            execution_id: exec_id,
            kind: kind.to_string(),
            tick: self.world.tick,
            budget_ms,
            source: source.map(|s| s.to_string()),
            mirror: Some(mirror),
            memory_gen: self.memory_generation(),
        };
        let write_failed = write_frame(&mut stdin, &frame).is_err();
        let mut requests_served = 0u64;
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
                match rx.recv() {
                    Ok(Ev::Frame(HostFrame::Request {
                        v,
                        request_id,
                        op,
                        payload,
                    })) => {
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
                        // 协议健全性：请求号单调递增、无缺口（A0 不做去重）。
                        if request_id != self.last_request_id + 1 {
                            if let Some(h) = &self.host {
                                h.kill();
                            }
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "REQUEST_ID_GAP".into(),
                                message: format!(
                                    "请求号跳变：期望 {}，得到 {request_id}",
                                    self.last_request_id + 1
                                ),
                                stack: String::new(),
                                stats: ExecStats::default(),
                                requests_served,
                                last_request_id: self.last_request_id,
                            };
                        }
                        self.last_request_id = request_id;
                        self.last_op = op.clone();
                        let t0 = Instant::now();
                        let reply = self.handle_op(&op, &payload);
                        self.stats
                            .op_us
                            .push(t0.elapsed().as_micros().max(1) as u64);
                        requests_served += 1;
                        if write_frame(
                            &mut stdin,
                            &MainFrame::Reply {
                                v: crate::protocol::PROTOCOL_VERSION,
                                request_id,
                                result: reply,
                            },
                        )
                        .is_err()
                        {
                            // 同上：回复写失败可能只是宿主刚被看门狗 / 终止按钮
                            // 杀掉的下游症状，因果优先于症状归类。
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
                    }
                    Ok(Ev::Frame(HostFrame::Complete {
                        v,
                        last_request_id,
                        has_loop,
                        stats,
                    })) => {
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
                        if last_request_id != self.last_request_id {
                            break ExecResult::Faulted {
                                class: FaultClass::HostTerminated("protocol"),
                                code: "LAST_REQUEST_MISMATCH".into(),
                                message: format!(
                                    "完成消息引用 {last_request_id}，已处理 {}",
                                    self.last_request_id
                                ),
                                stack: String::new(),
                                stats,
                                requests_served,
                                last_request_id,
                            };
                        }
                        self.stats.last_exec = stats.clone();
                        break ExecResult::Complete {
                            has_loop,
                            stats,
                            requests_served,
                        };
                    }
                    Ok(Ev::Frame(HostFrame::Fault {
                        v,
                        class,
                        code,
                        message,
                        stack,
                        last_request_id,
                        stats,
                    })) => {
                        let _ = v; // A0：故障帧不因版本差异拒收（诊断优先）
                        self.stats.last_exec = stats.clone();
                        let fc = match class.as_str() {
                            "script" => FaultClass::Script,
                            "environment" => FaultClass::Environment,
                            _ => FaultClass::HostTerminated("protocol"),
                        };
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
                    Ok(Ev::Eof) => {
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
                    Ok(Ev::Invalid(msg)) => {
                        if let Some(h) = &self.host {
                            h.kill();
                        }
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
                    Err(_) => {
                        let stderr = self.stderr_text();
                        break ExecResult::Faulted {
                            class: FaultClass::HostTerminated("crash"),
                            code: "HOST_EXITED".into(),
                            message: format!("宿主进程退出：{stderr}"),
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
        // 宿主仍活着才放回通道与管道（死亡则丢弃，drop_host 已处理）。
        if self.host.is_some() {
            self.host_stdin = Some(stdin);
            self.host_rx = Some(rx);
        }
        result
    }

    fn memory_generation(&self) -> u64 {
        match (&self.in_init, &self.init_branch) {
            (true, Some(branch)) => branch.generation(),
            _ => self.memory.generation(),
        }
    }

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

    fn handle_op(&mut self, op: &str, payload: &str) -> String {
        macro_rules! parse {
            () => {
                match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(v) => v,
                    Err(e) => return err_result("BAD_PAYLOAD", &e.to_string()),
                }
            };
        }
        match op {
            "robot.move" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let (Some(robot_id), Some(dx), Some(dy)) =
                    (p["robot_id"].as_u64(), p["dx"].as_i64(), p["dy"].as_i64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.move 参数缺失");
                };
                let code = self.world.accept_move(robot_id, dx as i32, dy as i32);
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.charge" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let Some(robot_id) = p["robot_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "robot.charge 参数缺失");
                };
                let code = self.world.accept_charge(robot_id);
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.take" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let (Some(robot_id), Some(target_id), Some(box_id)) = (
                    p["robot_id"].as_u64(),
                    p["target_id"].as_u64(),
                    p["box_id"].as_u64(),
                ) else {
                    return err_result("BAD_PAYLOAD", "robot.take 参数缺失");
                };
                let code = self.world.accept_take(robot_id, target_id, box_id);
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.give" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let (Some(robot_id), Some(target_id)) =
                    (p["robot_id"].as_u64(), p["target_id"].as_u64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.give 参数缺失");
                };
                let box_id = p["box_id"].as_u64(); // 缺省 = 当前携带物
                let code = self.world.accept_give(robot_id, target_id, box_id);
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.pick" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let (Some(robot_id), Some(x), Some(y)) =
                    (p["robot_id"].as_u64(), p["x"].as_i64(), p["y"].as_i64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.pick 参数缺失");
                };
                let code = self.world.accept_pick(robot_id, x as i32, y as i32);
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.drop" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let (Some(robot_id), Some(x), Some(y)) =
                    (p["robot_id"].as_u64(), p["x"].as_i64(), p["y"].as_i64())
                else {
                    return err_result("BAD_PAYLOAD", "robot.drop 参数缺失");
                };
                let box_id = p["box_id"].as_u64(); // 缺省 = 当前携带物
                let code = self.world.accept_drop(robot_id, x as i32, y as i32, box_id);
                ok_result(serde_json::json!({ "code": code }))
            }
            "market.take" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let Some(order_id) = p["order_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "market.take 参数缺失");
                };
                let (code, eff) = self.world.manage_take(order_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    let delta = MirrorDelta::from_take(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "market.cancel" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let Some(order_id) = p["order_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "market.cancel 参数缺失");
                };
                let (code, eff) = self.world.manage_cancel(order_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    let delta = MirrorDelta::from_cancel(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.destroy" => {
                let p = parse!();
                if self.in_init {
                    return ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}));
                }
                let Some(target_id) = p["target_id"].as_u64() else {
                    return err_result("BAD_PAYLOAD", "manage.destroy 参数缺失");
                };
                let (code, eff) = self.world.manage_destroy(target_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    let delta = MirrorDelta::from_destroy(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
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
                //（docs/architecture/03 宿主本地查询镜像）。
                let mirror = self.produce_mirror();
                ok_result(serde_json::json!({ "mirror": mirror }))
            }
            _ if op.starts_with("mem.") => self.handle_mem_op(op, payload),
            _ => err_result("UNKNOWN_OP", &format!("未知操作 {op}")),
        }
    }

    /// 管理操作成功路径的统一回复：递增世界修订号并携带镜像同步增量
    /// （docs/architecture/03 管理操作当 tick 可见）。
    fn mgmt_ok(&mut self, delta: MirrorDelta) -> String {
        self.revision.mgmt_count += 1;
        let delta_json = delta.to_json();
        self.stats.delta_bytes.push(delta_json.len());
        let delta_val: serde_json::Value =
            serde_json::from_str(&delta_json).unwrap_or(serde_json::Value::Null);
        ok_result(serde_json::json!({ "code": ztw_model::codes::OK, "delta": delta_val }))
    }

    fn handle_mem_op(&mut self, op: &str, payload: &str) -> String {
        let p: serde_json::Value = match serde_json::from_str(payload) {
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
