//! 市场生成器（docs/game-design/09）：基准价均值回归随机游走、板面挂单
//! 生成与周期刷新。f64 只承担中间计算（价格演化与采样），进入世界状态
//! 前按 half-even 量化回 milli——舍入规则随规则与存档版本冻结
//! （docs/architecture/02 确定性规则·数学）。
//!
//! 板面维护性的「替换最旧挂单」不是 docs/game-design/07 排除的「挂单撤回
//! 与改价」玩家侧风险事件，而是市场生成器职责：挂单价格生成后不再变动，
//! 价格漂移下的不变量（任一时刻同类型 bid < ask、价差恒大于手续费率）由
//! 生成时对对侧在挂极值钳制保证——新 ask 不低于最高 bid × (1 + 裕量)、
//! 新 bid 不高于最低 ask × (1 − 裕量)，裕量大于取消手续费率，交叉与
//! 价差收窄在结构上不可发生。

use std::collections::BTreeMap;

use ztw_model::{Id, MilliGold, Order, OrderSide};

use crate::rng::Xoshiro256;
use crate::world::World;
use crate::{GOODS, GoodsSpec, MARKET_CLAMP_MARGIN_PER_MILLE, MARKET_REFRESH_INTERVAL};

/// 市场状态：各货物当前基准价（milli）+ 独立 PRNG 流。流身份 "market"
/// 随存档格式冻结（与 "dock" 同款约定），改名即换流。
#[derive(Debug, Clone, PartialEq)]
pub struct MarketState {
    pub(crate) prices: BTreeMap<String, MilliGold>,
    pub(crate) rng: Xoshiro256,
}

/// f64 → milli 的 half-even 量化（规则版本冻结点；值域为正价格）。
fn quantize_half_even(x: f64) -> MilliGold {
    let floor = x.floor();
    let frac = x - floor;
    let even = (floor as i64) % 2 == 0;
    let rounded = if frac > 0.5 || (frac == 0.5 && !even) {
        floor + 1.0
    } else {
        floor
    };
    rounded as i64
}

/// [0,1) 均匀 → [-1,1) 零均值单位噪声（仅移位与乘减，位级确定；
/// 不调用语言运行时数学函数）。
fn unit_noise(rng: &mut Xoshiro256) -> f64 {
    let u = rng.next_u64();
    ((u >> 11) as f64) * (1.0 / 9007199254740992.0) * 2.0 - 1.0
}

fn side_qty_range(g: &GoodsSpec, side: OrderSide) -> (u32, u32) {
    match side {
        OrderSide::Sell => g.sell_qty,
        OrderSide::Buy => g.buy_qty,
    }
}

fn is_side(o: &Order, name: &str, side: OrderSide) -> bool {
    o.side == side && o.goods_type == name
}

/// 生成一张挂单：数量按侧采样，价格围绕当前基准价 ∓ 半价差并对对侧
/// 在挂极值钳制。借用拆分：next_id / listings / market 是不同字段，
/// 借用检查允许在持有 market 可变借用时直接推进。
#[allow(clippy::too_many_arguments)]
fn market_add_listing(
    listings: &mut BTreeMap<Id, Order>,
    next_id: &mut Id,
    m: &mut MarketState,
    g: &GoodsSpec,
    side: OrderSide,
) {
    let price = m.prices[g.name];
    let hs_span = (g.half_spread_max_per_mille - g.half_spread_min_per_mille + 1) as usize;
    let hs_per_mille = g.half_spread_min_per_mille as usize + m.rng.below(hs_span);
    // 卖单（ask）钳制对象是最高买价；买单（bid）是最低卖价。
    let opposite_extreme: Option<MilliGold> = match side {
        OrderSide::Sell => listings
            .values()
            .filter(|o| is_side(o, g.name, OrderSide::Buy))
            .map(|o| o.unit_price_milli)
            .max(),
        OrderSide::Buy => listings
            .values()
            .filter(|o| is_side(o, g.name, OrderSide::Sell))
            .map(|o| o.unit_price_milli)
            .min(),
    };
    let margin = MARKET_CLAMP_MARGIN_PER_MILLE as f64 / 1000.0;
    let unit_price_milli = match side {
        OrderSide::Sell => {
            let mut ask = price as f64 * (1.0 + hs_per_mille as f64 / 1000.0);
            if let Some(max_bid) = opposite_extreme {
                ask = ask.max(max_bid as f64 * (1.0 + margin));
            }
            quantize_half_even(ask).max(1)
        }
        OrderSide::Buy => {
            let mut bid = price as f64 * (1.0 - hs_per_mille as f64 / 1000.0);
            if let Some(min_ask) = opposite_extreme {
                bid = bid.min(min_ask as f64 * (1.0 - margin));
            }
            quantize_half_even(bid).max(1)
        }
    };
    let (lo, hi) = side_qty_range(g, side);
    let qty = (lo as usize + m.rng.below((hi - lo + 1) as usize)) as u32;
    let id = *next_id;
    *next_id += 1;
    listings.insert(
        id,
        Order {
            id,
            side,
            goods_type: g.name.to_string(),
            qty,
            unit_price_milli,
            vehicle: None,
            dock: None,
        },
    );
}

impl World {
    /// 启用市场生成器：以货物锚价播种基准价并生成初始板面（卖侧先行，
    /// 买单生成时对最小卖价钳制）。须在 `with_seed` 之后调用。
    pub fn with_market(mut self) -> World {
        debug_assert!(self.market.is_none(), "市场重复启用");
        let mut m = MarketState {
            prices: GOODS
                .iter()
                .map(|g| (g.name.to_string(), g.anchor_milli))
                .collect(),
            rng: Xoshiro256::derive(self.seed, "market"),
        };
        for g in GOODS {
            for side in [OrderSide::Sell, OrderSide::Buy] {
                let span = (g.board_max - g.board_min + 1) as usize;
                let target = g.board_min as usize + m.rng.below(span);
                for _ in 0..target {
                    market_add_listing(&mut self.listings, &mut self.next_id, &mut m, g, side);
                }
            }
        }
        self.market = Some(m);
        self
    }

    /// 当前基准价（市场未启用返回 None）。价格模型输出，测试与校准用。
    pub fn market_price(&self, goods: &str) -> Option<MilliGold> {
        self.market
            .as_ref()
            .and_then(|m| m.prices.get(goods).copied())
    }

    /// 市场 tick（阶段 1 的一部分，每 tick 边界调用一次）：先逐货物演化
    /// 基准价，再在刷新点（间隔见 MARKET_REFRESH_INTERVAL）替换每型每侧
    /// 最旧挂单并补足至采样目标数。
    pub(crate) fn market_tick(&mut self) {
        let Some(m) = self.market.as_mut() else {
            return;
        };
        for g in GOODS {
            let p = m.prices[g.name];
            let k = g.k_num as f64 / g.k_den as f64;
            let noise = unit_noise(&mut m.rng);
            let next =
                p as f64 + k * (g.anchor_milli as f64 - p as f64) + g.sigma_milli as f64 * noise;
            m.prices
                .insert(g.name.to_string(), quantize_half_even(next).max(1));
        }
        if self.tick == 0 || !self.tick.is_multiple_of(MARKET_REFRESH_INTERVAL) {
            return;
        }
        for g in GOODS {
            for side in [OrderSide::Sell, OrderSide::Buy] {
                let oldest = self
                    .listings
                    .values()
                    .filter(|o| is_side(o, g.name, side))
                    .map(|o| o.id)
                    .min();
                if let Some(id) = oldest {
                    self.listings.remove(&id);
                }
                let span = (g.board_max - g.board_min + 1) as usize;
                let target = g.board_min as usize + m.rng.below(span);
                loop {
                    let count = self
                        .listings
                        .values()
                        .filter(|o| is_side(o, g.name, side))
                        .count();
                    if count >= target {
                        break;
                    }
                    market_add_listing(&mut self.listings, &mut self.next_id, m, g, side);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_even_edges() {
        assert_eq!(quantize_half_even(0.5), 0); // 偶向下
        assert_eq!(quantize_half_even(1.5), 2); // 偶向上
        assert_eq!(quantize_half_even(2.5), 2);
        assert_eq!(quantize_half_even(2.4), 2);
        assert_eq!(quantize_half_even(2.6), 3);
        assert_eq!(quantize_half_even(-0.5), 0); // -0 → 0
        assert_eq!(quantize_half_even(-1.5), -2);
    }

    #[test]
    fn noise_in_unit_range() {
        let mut rng = Xoshiro256::derive(1, "test");
        for _ in 0..1000 {
            let n = unit_noise(&mut rng);
            assert!((-1.0..1.0).contains(&n));
        }
    }
}
