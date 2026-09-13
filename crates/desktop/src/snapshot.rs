//! 渲染快照（docs/architecture/02 §状态快照与诊断事件、05 §与 Rust 的通信）。
//!
//! 派生自 MirrorView（世界查询视图）叠加控制面状态（运行模式、故障摘要）：
//! 只携带最新可见状态、tick 与世界修订号；同一时刻最多一帧在途，慢前端
//! 合并丢旧（发布门控见 world_thread）。经济值沿用镜像的十进制字符串，
//! 前端显示时再换算。

use serde::Serialize;
use ztw_api::harness::{FaultClass, FaultRecord, Session};
use ztw_api::mirror::MirrorView;

/// 控制面摘要（随快照发布，暂停 / 变速 / 故障对前端可见）。
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    #[serde(flatten)]
    pub view: MirrorView,
    pub scenario: String,
    pub running: bool,
    pub tps: u32,
    /// 玩家代码已成功初始化（未加载时世界线程不推进）。
    pub loaded: bool,
    pub fault: Option<FaultView>,
}

/// 故障摘要：独立于日志环保留最近一次（docs/architecture/02：
/// “故障摘要独立保留最近一次”，刷屏不挤掉）。
#[derive(Debug, Clone, Serialize)]
pub struct FaultView {
    /// script | environment | host_terminated:<reason>
    pub class: String,
    pub code: String,
    pub message: String,
    pub stack: String,
    pub tick: u64,
    pub last_request_id: u64,
    pub last_op: String,
    pub requests_served: u64,
}

impl FaultView {
    pub fn from_record(class: &FaultClass, rec: &FaultRecord) -> FaultView {
        let class = match class {
            FaultClass::Script => "script".to_string(),
            FaultClass::Environment => "environment".to_string(),
            FaultClass::HostTerminated(reason) => format!("host_terminated:{reason}"),
        };
        FaultView {
            class,
            code: rec.code.clone(),
            message: rec.message.clone(),
            stack: rec.stack.clone(),
            tick: rec.tick,
            last_request_id: rec.last_request_id,
            last_op: rec.last_op.clone(),
            requests_served: rec.requests_served,
        }
    }
}

/// 从 Session 派生快照 JSON（世界线程独占 Session，调用点即安全点）。
pub fn snapshot_json(session: &Session, scenario: &str, running: bool, tps: u32) -> String {
    let snap = Snapshot {
        view: MirrorView::from_world(&session.world, session.world_revision().get()),
        scenario: scenario.to_string(),
        running,
        tps,
        loaded: session.initialized,
        fault: session
            .fault
            .as_ref()
            .map(|rec| FaultView::from_record(&rec.class, rec)),
    };
    serde_json::to_string(&snap).expect("快照序列化不失败")
}
