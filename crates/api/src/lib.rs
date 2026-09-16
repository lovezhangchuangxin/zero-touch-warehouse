//! `crates/api`：Game 门面定义、快照视图（宿主查询镜像）、受控 memory
//! 操作与 IPC 协议——绑定与协议的唯一来源（docs/architecture/01）。
//!
//! `harness` 是会话层（世界线程消息循环与执行生命周期），desktop 生产
//! 世界线程与全部集成测试共用；op 分发在 `ops`（payload 字段契约的
//! 单一事实源 `OP_FIELDS` 亦在此），宿主进程管理在 `host`；两宿主
//! （ztw-host-js / ztw-host-py）共享的 IPC 骨架与故障注入框架在
//! `host_ipc`（旋钮清单 `FAULT_KNOBS` 的单一事实源）。

pub mod harness;
pub(crate) mod host;
pub mod host_ipc;
pub mod memory;
pub mod mirror;
pub mod ops;
pub mod protocol;
pub mod save;

/// 宿主侧 Game 绑定（bootstrap.js）。以源码文本内嵌，由 runtime crate
/// 在执行环境重建后求值；它属于绑定层，因此以本 crate 为唯一出处。
pub const BOOTSTRAP_JS: &str = include_str!("../bindings/bootstrap.js");

/// 宿主侧 Game 绑定（bootstrap.py，Python 版）。语义逐项对照
/// bootstrap.js；人工同步点由双语言矩阵测试锚定（A1）。
pub const BOOTSTRAP_PY: &str = include_str!("../bindings/bootstrap.py");
