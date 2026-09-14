//! `crates/api`：Game 门面定义、快照视图（宿主查询镜像）、受控 memory
//! 操作与 IPC 协议——绑定与协议的唯一来源（docs/architecture/01）。
//!
//! `harness` 是 A0 的 headless 主进程替身：扮演 docs 里 `crates/desktop`
//! 的宿主生命周期管理与世界线程角色，仅供 `cargo test` 与量测使用。

pub mod harness;
pub mod memory;
pub mod mirror;
pub mod protocol;

/// 宿主侧 Game 绑定（bootstrap.js）。以源码文本内嵌，由 runtime crate
/// 在执行环境重建后求值；它属于绑定层，因此以本 crate 为唯一出处。
pub const BOOTSTRAP_JS: &str = include_str!("../bindings/bootstrap.js");

/// 宿主侧 Game 绑定（bootstrap.py，Python 版）。语义逐项对照
/// bootstrap.js；人工同步点由双语言矩阵测试锚定（A1）。
pub const BOOTSTRAP_PY: &str = include_str!("../bindings/bootstrap.py");
