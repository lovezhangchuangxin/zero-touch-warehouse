//! 世界线程：`Session` 之上的暂停 / 单步 / 常速 / 快进状态机与快照发布
//! 门控（docs/architecture/02 §世界线程、§界面命令生效点、§状态快照与
//! 诊断事件、§变速与暂停）。不把这层塞回 harness——harness 保持 A0
//! headless 测试替身定位。
//!
//! 命令生效点：本线程只在 tick 之间的安全点读命令队列——运行中即
//! 「上一 tick 结束后、下一 tick 边界前」的 FIFO；暂停时阻塞在队列上，
//! 命令到达即处理；单步执行期间（tick 进行中）到达的命令排队到该 tick
//! 结束。三种时序由同一处自然成立（docs/architecture/02）。
//!
//! 快照发布：最多一帧在途，前端确认（ack）后只补发更新的最新帧——
//! 慢前端合并丢旧、不反压模拟；诊断事件与日志走独立环形缓冲 + 游标
//! 分页，不随快照丢弃。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use ztw_api::harness::{DiagTapKind, FaultClass, OutcomeKind, Session, SessionConfig, TickOutcome};

use crate::diag::{DiagPage, DiagRing};
use crate::scenario::{self, ScenarioSpec};
use crate::snapshot::{FaultView, snapshot_json};

/// 常速档位范围 1–10 tick/s、快进 50–200 tick/s（docs/architecture/02 待定
/// 参考值；快进实际频率受每 tick 实际耗时约束，达不到时连续推进）。
/// TPS_NORMAL_MAX/TPS_FF_MIN 只是参考档位边界：Resume 的 clamp 有意
/// 接受连续值（1..=FF_MAX），UI 档位之外的值不视为错误。
pub const TPS_MIN: u32 = 1;
#[allow(dead_code)]
pub const TPS_NORMAL_MAX: u32 = 10;
#[allow(dead_code)]
pub const TPS_FF_MIN: u32 = 50;
pub const TPS_FF_MAX: u32 = 200;
/// 调度落后超过 4 个周期即重置基准（暂停恢复 / 页面卡顿后不追帧螺旋）。
const CATCHUP_PERIODS: u32 = 4;

/// 界面命令（docs/architecture/05 §与 Rust 的通信）。
#[derive(Debug, Clone)]
pub enum Ctrl {
    Pause,
    Resume {
        tps: u32,
    },
    /// 推进一个完整 tick 后回到暂停。
    Step,
    /// 热重载：编辑代码在当前 tick 结束后暂停，保存即重建执行环境
    /// （docs/game-design/05 §执行生命周期；宿主进程不重启）。
    /// 语言与当前不同时先重建会话（= 宿主进程重启；docs/architecture/03
    /// 执行模型：切换语言重启宿主），已提交 Game.memory 保留。
    LoadCode {
        code: String,
        language: crate::hostbin::Language,
    },
    /// 重开场景：世界 / memory 重建，玩家源码保留并自动重新初始化
    /// （docs/architecture/02 §场景）。
    Reset {
        scenario: String,
    },
}

/// 控制面摘要（供命令层同步读取；详情随快照发布）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusView {
    pub scenario: String,
    pub running: bool,
    pub tps: u32,
    pub loaded: bool,
    pub fault_class: Option<String>,
    pub tick: u64,
    /// 当前玩家代码语言（"js" / "py"）。
    pub language: String,
}

/// 快照推送槽（lib 不依赖 Tauri：壳层把 Channel 适配成 `Fn(&str)`）。
pub type Sink = Arc<dyn Fn(&str) + Send + Sync>;

pub struct Shared {
    /// 最新快照与其发布序号（前端 pull 兜底 + ack 后补发源）。序号只对
    /// 「publish 被调用」单调 +1：世界修订号不覆盖控制面（loaded/running/
    /// fault），按世界修订判重会漏发纯控制面变化帧（如暂停中热重载）。
    latest: Mutex<Option<(Arc<str>, u64)>>,
    sink: Mutex<Option<Sink>>,
    inflight: AtomicBool,
    last_sent_seq: AtomicU64,
    pub_seq: AtomicU64,
    pub diag: Mutex<DiagRing>,
    pub logs: Mutex<DiagRing>,
    /// 终止按钮的外部控制柄（独立路径，不经命令队列）。
    pub host_ctl: Mutex<Option<ztw_api::harness::HostControl>>,
    pub status: Mutex<StatusView>,
    pub static_info: Mutex<Value>,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            latest: Mutex::new(None),
            sink: Mutex::new(None),
            inflight: AtomicBool::new(false),
            last_sent_seq: AtomicU64::new(0),
            pub_seq: AtomicU64::new(0),
            // 诊断：4096 条 / 256KB；日志独立配额（docs/architecture/02）。
            diag: Mutex::new(DiagRing::new(4096, 256 * 1024)),
            logs: Mutex::new(DiagRing::new(1024, 256 * 1024)),
            host_ctl: Mutex::new(None),
            status: Mutex::new(StatusView {
                scenario: String::new(),
                running: false,
                tps: 0,
                loaded: false,
                fault_class: None,
                tick: 0,
                language: "js".into(),
            }),
            static_info: Mutex::new(Value::Null),
        }
    }

    /// 注册快照推送槽，并立即补发当前最新帧。新槽未见过任何帧，绕过
    /// “同一次发布不重发”门控强制送达（否则首发早于 attach 时重放被吞，
    /// 只剩 pull 兜底）。补发在 latest 锁内取帧：publish 的 store_latest
    /// 被挡在后面，新槽不可能收到比已发更旧的帧；last_sent_seq 只进不退。
    pub fn attach_sink(&self, sink: Sink) {
        *self.sink.lock().expect("sink 锁") = Some(sink);
        let latest = self.latest.lock().expect("latest 锁").clone();
        if let Some((json, seq)) = latest {
            self.inflight.store(true, Ordering::Release);
            self.last_sent_seq.fetch_max(seq, Ordering::AcqRel);
            if let Some(s) = self.sink.lock().expect("sink 锁").clone() {
                s(&json);
            }
        }
    }

    /// 前端确认收帧：清在途标记并补发更新的最新帧（若已产生）。
    pub fn ack_snapshot(&self) {
        self.inflight.store(false, Ordering::Release);
        let latest = self.latest.lock().expect("latest 锁").clone();
        if let Some((json, seq)) = latest {
            self.offer(&json, seq);
        }
    }

    /// 发布门控：同一次发布不重发；在途未确认时只更新 latest。
    fn offer(&self, json: &Arc<str>, seq: u64) {
        if self.last_sent_seq.load(Ordering::Acquire) == seq {
            return;
        }
        if self
            .inflight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return; // 一帧仍在途：合并丢旧，等 ack 后补发最新
        }
        self.last_sent_seq.store(seq, Ordering::Release);
        let sink = self.sink.lock().expect("sink 锁").clone();
        match sink {
            Some(sink) => sink(json),
            None => self.inflight.store(false, Ordering::Release),
        }
    }

    fn store_latest(&self, json: Arc<str>, seq: u64) {
        *self.latest.lock().expect("latest 锁") = Some((json, seq));
    }

    /// 最新快照（pull 兜底通道；无快照时 None）。
    pub fn latest_snapshot(&self) -> Option<Arc<str>> {
        self.latest
            .lock()
            .expect("latest 锁")
            .as_ref()
            .map(|(j, _)| j.clone())
    }

    pub fn diag_pull(&self, after: u64, limit: usize) -> DiagPage {
        self.diag.lock().expect("diag 锁").pull(after, limit)
    }

    pub fn diag_ack(&self, cursor: u64) {
        self.diag.lock().expect("diag 锁").ack(cursor);
    }

    pub fn logs_pull(&self, after: u64, limit: usize) -> DiagPage {
        self.logs.lock().expect("logs 锁").pull(after, limit)
    }

    pub fn logs_ack(&self, cursor: u64) {
        self.logs.lock().expect("logs 锁").ack(cursor);
    }
}

/// 世界线程句柄：命令入队（非阻塞）+ 共享面读取。
pub struct WorldHandle {
    ctrl_tx: mpsc::Sender<Ctrl>,
    shared: Arc<Shared>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl WorldHandle {
    /// 启动世界线程（未知场景 id 回落默认场景，静默处理——启动阶段
    /// 前端尚未 attach，诊断事件无人消费）。
    pub fn spawn(scenario_id: &str, bins: crate::hostbin::HostBins) -> WorldHandle {
        let spec = scenario::by_id(scenario_id).unwrap_or(scenario::SCENARIOS[0]);
        let shared = Arc::new(Shared::new());
        let (tx, rx) = mpsc::channel::<Ctrl>();
        let thread_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name("ztw-world".into())
            .spawn(move || run(spec, bins, rx, thread_shared))
            .expect("spawn world thread");
        WorldHandle {
            ctrl_tx: tx,
            shared,
            join: Mutex::new(Some(join)),
        }
    }

    /// 发送界面命令（线程存活时立即返回；命令在安全点生效）。
    pub fn ctrl(&self, cmd: Ctrl) -> Result<(), String> {
        self.ctrl_tx
            .send(cmd)
            .map_err(|_| "世界线程已退出".to_string())
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// 等待世界线程退出（测试清理用；drop 句柄同样会让线程收尾）。
    /// 先 drop 命令发送端再 join——否则世界线程阻塞在 recv 上永不退出。
    pub fn join(self) {
        let WorldHandle { ctrl_tx, join, .. } = self;
        drop(ctrl_tx);
        if let Some(j) = join.lock().expect("join 锁").take() {
            let _ = j.join();
        }
    }
}

// ---------------------------------------------------------------------------
// 线程主循环
// ---------------------------------------------------------------------------

struct RunState {
    spec: &'static ScenarioSpec,
    session: Session,
    /// 玩家源码（跨 reset 保留，docs/architecture/02）。
    code: Option<String>,
    running: bool,
    tps: u32,
    /// 单步请求：安全点执行一个 tick 后回暂停。
    stepping: bool,
    next_tick_at: Option<Instant>,
    /// 当前语言与两语言宿主二进制（解析一次，切换语言直接换 bin）。
    language: crate::hostbin::Language,
    bins: crate::hostbin::HostBins,
}

fn run(
    spec: &'static ScenarioSpec,
    bins: crate::hostbin::HostBins,
    rx: mpsc::Receiver<Ctrl>,
    shared: Arc<Shared>,
) {
    let mut st = init_state(spec, bins);
    bootstrap(&st, &shared);
    publish(&mut st, &shared);
    loop {
        // 1) 命令等待：运行且到点 → 零超时轮询；否则阻塞（暂停即“立即生效”）。
        let wait = tick_wait(&st);
        match wait {
            None => match rx.recv() {
                Ok(cmd) => handle(&mut st, &shared, cmd),
                Err(_) => break,
            },
            Some(d) => match rx.recv_timeout(d) {
                Ok(cmd) => handle(&mut st, &shared, cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
        }
        // 2) 单步：一个完整 tick 后回暂停（运行位不置起）。
        if st.stepping {
            st.stepping = false;
            if can_tick(&st) {
                one_tick(&mut st, &shared);
            }
            continue;
        }
        // 3) 常速 / 快进调度：到点推进；落后超阈值重置基准（连续推进不追帧）。
        if st.running && can_tick(&st) {
            let interval = tick_interval(st.tps);
            let now = Instant::now();
            let base = *st.next_tick_at.get_or_insert(now);
            if now >= base {
                let overdue = now - base;
                if overdue > interval * CATCHUP_PERIODS {
                    st.next_tick_at = Some(now);
                } else {
                    st.next_tick_at = Some(base + interval);
                }
                one_tick(&mut st, &shared);
            }
        }
    }
}

fn fresh_session(spec: &'static ScenarioSpec, host_bin: PathBuf) -> Session {
    Session::new(SessionConfig::new(host_bin), scenario::build(spec))
}

fn init_state(spec: &'static ScenarioSpec, bins: crate::hostbin::HostBins) -> RunState {
    RunState {
        spec,
        session: fresh_session(spec, bins.js.clone()),
        code: None,
        running: false,
        tps: 0,
        stepping: false,
        next_tick_at: None,
        language: crate::hostbin::Language::Js,
        bins,
    }
}

/// 启动 / 重建后的共享面引导：静态信息、外部控制柄。
fn bootstrap(st: &RunState, shared: &Shared) {
    *shared.static_info.lock().expect("static 锁") =
        scenario::static_json(st.spec, &st.session.world);
    *shared.host_ctl.lock().expect("host_ctl 锁") = Some(st.session.host_control());
}

fn tick_wait(st: &RunState) -> Option<Duration> {
    if !(st.running && can_tick(st)) {
        return None; // 暂停 / 未加载 / 故障：无限期等命令
    }
    let interval = tick_interval(st.tps);
    let base = st.next_tick_at?;
    let now = Instant::now();
    Some(if now >= base {
        Duration::ZERO
    } else {
        (base - now).min(interval * CATCHUP_PERIODS)
    })
}

fn tick_interval(tps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / tps.max(1) as f64)
}

fn can_tick(st: &RunState) -> bool {
    st.session.initialized && st.session.fault.is_none()
}

fn handle(st: &mut RunState, shared: &Shared, cmd: Ctrl) {
    match cmd {
        Ctrl::Pause => {
            st.running = false;
            st.next_tick_at = None;
            publish(st, shared);
        }
        Ctrl::Resume { tps } => {
            st.tps = tps.clamp(TPS_MIN, TPS_FF_MAX);
            if recover_if_faulted(st, shared) {
                st.running = true;
                st.next_tick_at = Some(Instant::now());
            }
            publish(st, shared);
        }
        Ctrl::Step => {
            if st.session.fault.is_some() {
                let _ = recover_if_faulted(st, shared);
            }
            st.stepping = true;
        }
        Ctrl::LoadCode { code, language } => {
            // 编辑即暂停（docs/architecture/05：当前 tick 结束后暂停，保存即重载）。
            st.running = false;
            st.next_tick_at = None;
            // 语言切换 = 宿主进程重启（docs 03）。Session（世界与受控
            // memory 树）保留，只换宿主二进制并干净重启：restart_host 在
            // code=None 时只做 drop_host（清 killed 标记、收尸），随后
            // load_code 以新 bin spawn——已提交 Game.memory 跨语言保留。
            if language != st.language {
                diag_push(
                    shared,
                    st.session.world.tick,
                    "control",
                    json!({
                        "op": "switch_language",
                        "from": st.language.as_str(),
                        "to": language.as_str(),
                    }),
                );
                st.language = language;
                st.session.cfg.host_bin = bin_of(st);
                st.code = None;
                let _ = st.session.restart_host();
                *shared.host_ctl.lock().expect("host_ctl 锁") = None;
            }
            load_code(st, shared, code);
            publish(st, shared);
        }
        Ctrl::Reset { scenario: id } => {
            let Some(spec) = scenario::by_id(&id) else {
                diag_push(
                    shared,
                    0,
                    "control",
                    json!({ "op": "reset", "ok": false, "error": format!("未知场景 {id}") }),
                );
                return;
            };
            let bin = bin_of(st);
            st.spec = spec;
            st.session = fresh_session(spec, bin);
            st.running = false;
            st.stepping = false; // 单步请求不跨场景代次存活
            st.next_tick_at = None;
            // 内容清空但 seq 单调延续：旧游标不误报新事件。
            shared.diag.lock().expect("diag 锁").clear_keep_seq();
            shared.logs.lock().expect("logs 锁").clear_keep_seq();
            bootstrap(st, shared);
            diag_push(
                shared,
                0,
                "control",
                json!({ "op": "reset", "ok": true, "scenario": id }),
            );
            if let Some(code) = st.code.clone() {
                load_code(st, shared, code);
            }
            publish(st, shared);
        }
    }
}

/// 按当前语言取宿主二进制。
fn bin_of(st: &RunState) -> PathBuf {
    match st.language {
        crate::hostbin::Language::Js => st.bins.js.clone(),
        crate::hostbin::Language::Py => st.bins.py.clone(),
    }
}

fn load_code(st: &mut RunState, shared: &Shared, code: String) {
    let outcome = st.session.load_code(&code);
    if outcome.ok {
        st.code = Some(code);
    }
    *shared.host_ctl.lock().expect("host_ctl 锁") = Some(st.session.host_control());
    diag_push(
        shared,
        st.session.world.tick,
        "control",
        json!({
            "op": "load_code",
            "ok": outcome.ok,
            "fault": outcome.fault.as_ref().map(|f| FaultView::from_record(&f.class, f)),
        }),
    );
}

/// 故障三分类恢复（docs/architecture/03 §故障分级）。返回是否可继续推进。
fn recover_if_faulted(st: &mut RunState, shared: &Shared) -> bool {
    let Some(fault) = st.session.fault.clone() else {
        return true;
    };
    let ok = match &fault.class {
        FaultClass::Script => st.session.resume_after_script_error(),
        FaultClass::Environment => st.session.reinit_after_env_fault().ok,
        FaultClass::HostTerminated(_) => {
            let out = st.session.restart_host();
            *shared.host_ctl.lock().expect("host_ctl 锁") = Some(st.session.host_control());
            out.ok
        }
    };
    if ok {
        diag_push(
            shared,
            st.session.world.tick,
            "control",
            json!({ "op": "recover", "from": FaultView::from_record(&fault.class, &fault).class, "ok": true }),
        );
    }
    ok
}

fn one_tick(st: &mut RunState, shared: &Shared) {
    let out: TickOutcome = st.session.tick();
    // 结算结果 → 诊断事件（含失败码；docs/architecture/02“结算结果均有记录”）。
    {
        let mut diag = shared.diag.lock().expect("diag 锁");
        for (id, lr) in st.session.world.last_results.iter() {
            diag.push(
                out.tick,
                "settle",
                json!({
                    "robot_id": id, "action": lr.action, "arg": lr.arg, "code": lr.code,
                    "ok": lr.code == ztw_model::codes::OK,
                }),
            );
        }
        for tap in st.session.take_diag() {
            let kind = match tap.kind {
                DiagTapKind::AcceptFail => "accept_fail",
                DiagTapKind::Manage => "manage",
                DiagTapKind::OrderDone => "order_done",
            };
            let tick = tap.tick;
            diag.push(
                tick,
                kind,
                serde_json::to_value(&tap).expect("采集事件序列化"),
            );
        }
    }
    // Game.log → 独立日志环（刷屏不挤掉诊断与故障）。
    {
        let mut logs = shared.logs.lock().expect("logs 锁");
        for (tick, line) in st.session.logs.drain(..) {
            logs.push(tick, "log", json!({ "line": line }));
        }
    }
    // 运行时故障 → 事件 + 自动暂停（已受理动作已在结算后，docs 03 §故障分级）。
    if let OutcomeKind::Fault(class) = &out.kind {
        let rec = st.session.fault.clone().expect("故障已记录");
        let view = FaultView::from_record(class, &rec);
        shared.diag.lock().expect("diag 锁").push(
            out.tick,
            "fault",
            serde_json::to_value(&view).expect("故障事件序列化"),
        );
        st.running = false;
        st.next_tick_at = None;
    }
    publish(st, shared);
}

/// 产出快照：存 latest + 门控推送 + 刷新状态摘要。
fn publish(st: &mut RunState, shared: &Shared) {
    let json: Arc<str> = Arc::from(snapshot_json(&st.session, st.spec.id, st.running, st.tps));
    let seq = shared.pub_seq.fetch_add(1, Ordering::Relaxed) + 1;
    shared.store_latest(json.clone(), seq);
    shared.offer(&json, seq);
    let fault_class = st.session.fault.as_ref().map(|f| match f.class {
        FaultClass::Script => "script".to_string(),
        FaultClass::Environment => "environment".to_string(),
        FaultClass::HostTerminated(r) => format!("host_terminated:{r}"),
    });
    *shared.status.lock().expect("status 锁") = StatusView {
        scenario: st.spec.id.to_string(),
        running: st.running,
        tps: st.tps,
        loaded: st.session.initialized,
        fault_class,
        tick: st.session.world.tick,
        language: st.language.as_str().to_string(),
    };
}

fn diag_push(shared: &Shared, tick: u64, kind: &str, payload: Value) {
    shared
        .diag
        .lock()
        .expect("diag 锁")
        .push(tick, kind, payload);
}
