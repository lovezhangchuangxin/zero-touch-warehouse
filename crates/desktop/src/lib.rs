//! `crates/desktop` 核心：世界线程状态机、渲染快照、诊断环形缓冲、场景
//! 与宿主二进制解析（docs/architecture/01 §代码布局）。
//!
//! 本 lib 不依赖 Tauri，可独立测试；Tauri 壳（main.rs）只做命令面适配与
//! 窗口组装。世界线程在 `ztw-api` 的 Session 之上叠加暂停 / 单步 / 变速
//! 与发布门控，不回填 harness（保持其 A0 headless 测试替身定位）。

pub mod diag;
pub mod hostbin;
pub mod scenario;
pub mod snapshot;
pub mod world_thread;

pub use diag::{DiagEvent, DiagPage, DiagRing, Gap};
pub use scenario::{B2_FIVE, B2_ONE, DEFAULT_ID, SCENARIOS};
pub use snapshot::{FaultView, Snapshot};
pub use world_thread::{
    Ctrl, Shared, StatusView, TPS_FF_MAX, TPS_FF_MIN, TPS_NORMAL_MAX, WorldHandle,
};
