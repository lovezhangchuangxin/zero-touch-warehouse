//! tick 状态机、动作受理与统一结算、市场（挂单生成 / 刷新 / 管理操作）
//! 与车辆生命周期。语义以 docs/game-design/03、04、09 为准，结构以
//! docs/architecture/02「结算三段式」为准。
//!
//! 模块划分（模块私有，公开面只经本文件再导出冻结）：`world`（结构与
//! 构造、静态查询、交互目标解析、边界事件、收尾与确定性摘要）、`accept`
//! （动作受理静态检查）、`market`（管理操作与效果摘要）、`marketgen`
//! （市场生成器：价格演化与板面刷新）、`settle`（统一结算）、`intent`
//! （意图与结果类型）、`rng`（分流 PRNG）。
//!
//! B1 范围（docs/architecture/08 原型 B 的模拟核心）：move 之外补全 charge /
//! take / give / pick / drop 与机器人间被动转交；车辆离场 → 订单完成 → 收款 →
//! 装卸位释放闭环；market.cancel 与最小 manage_destroy；xoshiro 分流 PRNG
//! 承担装卸位分配。M3 起市场生成器落地（marketgen）与借贷 / 商店管理
//! 操作（market）；find_path 属后续里程碑。

mod accept;
mod intent;
mod market;
mod marketgen;
mod rng;
mod settle;
mod world;

pub use intent::{Intent, LastResult, TargetRef};
pub use market::{
    BorrowEffect, BoughtKind, BuyEffect, CancelEffect, DestroyEffect, DestroyedKind, RepayEffect,
    TakeEffect,
};
pub use marketgen::MarketState;
pub use rng::Xoshiro256;
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
pub const PRICE_DOCK: MilliGold = 500_000;
/// 借贷利率：每 tick 复利 debt += debt / DEN × NUM（向下取整，与手续费
/// 同款舍入；规则版本冻结点）。锚点 0.1%/tick——短期周转便宜、长期囤债
/// 受罚；随里程碑 4 校准冻结。
pub const INTEREST_NUMERATOR: MilliGold = 1;
pub const INTEREST_DENOMINATOR: MilliGold = 1000;
/// 信用额度（milli）。锚点 1000 gold > 机器人价 650（docs/game-design/06：
/// 额度须高于购置一台机器人的解围成本）；随校准冻结。
pub const CREDIT_LIMIT_MILLI: MilliGold = 1_000_000;

/// 板面刷新间隔（docs/game-design/09：30–60 tick 窗口内取 40）。
pub const MARKET_REFRESH_INTERVAL: u64 = 40;
/// 新挂单对对侧在挂极值的钳制裕量（基准价千分比）。须大于取消手续费率
/// （100‰）：「任一时刻同类型 bid < ask」与「价差恒大于手续费率」两条
/// 不变量由此结构成立（见 marketgen.rs 模块注释）。
pub const MARKET_CLAMP_MARGIN_PER_MILLE: u32 = 120;

/// 市场货物目录条目（docs/game-design/09；全部数值为示例锚点，随原型
/// 平衡——里程碑 4 校准冻结）。平稳标准差 = σ / √(2k − k²)，带宽按
/// 锚价 ± 4 倍平稳标准差推导（见 m3_market 测试）。
#[derive(Debug, Clone, Copy)]
pub struct GoodsSpec {
    pub name: &'static str,
    /// 回归锚价 P̄（milli）。
    pub anchor_milli: MilliGold,
    /// 每 tick 回归系数 k = k_num / k_den。
    pub k_num: u32,
    pub k_den: u32,
    /// 每 tick 波动幅度 σ（milli）。
    pub sigma_milli: MilliGold,
    /// 半价差采样范围（基准价的千分比；两侧全价差落在示例 20%–40% 带）。
    pub half_spread_min_per_mille: u32,
    pub half_spread_max_per_mille: u32,
    /// 卖单（玩家买入）数量区间——买入单量大。
    pub sell_qty: (u32, u32),
    /// 买单（玩家卖出）数量区间——卖出单量小而频繁。
    pub buy_qty: (u32, u32),
    /// 每侧板面常驻下限 / 采样上限。
    pub board_min: u32,
    pub board_max: u32,
}

/// 三货物目录：water 前期薄利多销、battery 中期囤货投机、chip 后期
/// 资金蓄水池（挂单稀疏、波动大）。
pub const GOODS: &[GoodsSpec] = &[
    GoodsSpec {
        name: "water",
        anchor_milli: 4_500,
        k_num: 1,
        k_den: 64,
        sigma_milli: 150,
        half_spread_min_per_mille: 100,
        half_spread_max_per_mille: 160,
        sell_qty: (8, 16),
        buy_qty: (2, 6),
        board_min: 2,
        board_max: 4,
    },
    GoodsSpec {
        name: "battery",
        anchor_milli: 15_000,
        k_num: 1,
        k_den: 64,
        sigma_milli: 400,
        half_spread_min_per_mille: 110,
        half_spread_max_per_mille: 170,
        sell_qty: (4, 8),
        buy_qty: (2, 4),
        board_min: 2,
        board_max: 4,
    },
    GoodsSpec {
        name: "chip",
        anchor_milli: 60_000,
        k_num: 1,
        k_den: 48,
        sigma_milli: 2_500,
        half_spread_min_per_mille: 130,
        half_spread_max_per_mille: 200,
        sell_qty: (2, 4),
        buy_qty: (1, 2),
        board_min: 2,
        board_max: 3,
    },
];

use ztw_model::MilliGold;
