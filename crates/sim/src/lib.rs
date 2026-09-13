//! tick 状态机、动作受理与统一结算、固定挂单市场与车辆生命周期。
//! 语义以 docs/game-design/03、04 为准，结构以 docs/architecture/02
//! 「结算三段式」为准。
//!
//! 模块划分：`world`（结构与构造、静态查询、边界事件、摘要）、`accept`
//! （动作受理静态检查）、`market`（管理操作与效果摘要）、`settle`（统一
//! 结算）、`intent`（意图与结果类型）、`rng`（分流 PRNG）。
//!
//! B1 范围（docs/architecture/08 原型 B 的模拟核心）：move 之外补全 charge /
//! take / give / pick / drop 与机器人间被动转交；车辆离场 → 订单完成 → 收款 →
//! 装卸位释放闭环；market.cancel 与最小 manage_destroy；xoshiro 分流 PRNG
//! 承担装卸位分配。市场挂单刷新与价格波动、购买类管理操作、find_path 属
//! 后续里程碑。

pub mod accept;
pub mod intent;
pub mod market;
pub mod rng;
pub mod settle;
pub mod world;

pub use intent::{Intent, LastResult, TargetRef};
pub use market::{CancelEffect, DestroyEffect, DestroyedKind, TakeEffect};
pub use rng::Xoshiro256;
pub use settle::{Departure, SettlementPlan};
pub use world::World;

// ---------------------------------------------------------------------------
// 数值锚点（临时；docs/game-design/07 列为随原型平衡的待定项）
// ---------------------------------------------------------------------------

/// 每 tick 充电量（封顶 energy_max）。
pub const CHARGE_PER_TICK: u32 = 25;
/// 动作耗电（移动不耗电；四者数值不同是文档要求）。
pub const ENERGY_COST_TAKE: u32 = 6;
pub const ENERGY_COST_GIVE: u32 = 5;
pub const ENERGY_COST_PICK: u32 = 4;
pub const ENERGY_COST_DROP: u32 = 3;
/// 取消手续费 = 订单额 × 10%（向下取整，退款下限 0；docs/game-design/04）。
pub const CANCEL_FEE_NUMERATOR: MilliGold = 1;
pub const CANCEL_FEE_DENOMINATOR: MilliGold = 10;
/// 销毁退款 = 设备价 × 50%；设备价取 docs/game-design/09 锚点（千分金币）。
pub const DESTROY_REFUND_NUMERATOR: MilliGold = 1;
pub const DESTROY_REFUND_DENOMINATOR: MilliGold = 2;
pub const PRICE_ROBOT: MilliGold = 650_000;
pub const PRICE_SHELF: MilliGold = 175_000;
pub const PRICE_CHARGER: MilliGold = 300_000;
pub const PRICE_PORT: MilliGold = 500_000;

use ztw_model::MilliGold;
