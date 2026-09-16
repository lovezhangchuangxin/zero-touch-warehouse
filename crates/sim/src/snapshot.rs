//! 存档快照数据面（docs/architecture/06 §文件格式与落盘）：`World` 的
//! 显式镜像 DTO 与双向转换。不给 `World` 直接挂 serde——镜像结构以
//! 穷举字段字面量转换，新增世界字段时编译器强制同步此处，state_hash
//! 往返测试兜底（镜像漏字段即测试红）。serde 派生走非默认 feature
//! `serde`，ztw-sim 默认构建保持零外部依赖。
//!
//! 完备性基准 = `World::state_hash` 的覆盖面（全部影响后续行为的状态）。
//! `intents` 是 tick 内暂态，安全点（settle / end_tick 之后）必空、不入档。

use std::collections::{BTreeMap, BTreeSet};

use ztw_model::{Charger, Dock, GroundBox, Id, MilliGold, Order, Position, Robot, Shelf, Vehicle};

use crate::MarketState;
use crate::World;
use crate::intent::{ACTION_NAMES, LastResult};
use crate::rng::Xoshiro256;
use crate::world::PendingArrival;

/// 上一 tick 结算结果的存档镜像：`action: &'static str` 收窄为 String
/// 入档，恢复按 [ACTION_NAMES] 码表反查（值域冻结，见 intent.rs）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct LastResultSnapshot {
    pub action: String,
    pub arg: String,
    pub code: String,
}

/// 市场生成器状态镜像：基准价表 + "market" 流状态字（流身份随存档
/// 格式冻结，marketgen.rs 模块注释）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct MarketSnapshot {
    pub prices: BTreeMap<String, MilliGold>,
    pub rng_words: [u64; 4],
}

/// 待到场车辆事件镜像（tick 边界补发语义见 world.rs boundary_events）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct ArrivalSnapshot {
    pub order_id: Id,
    pub arrive_tick: u64,
}

/// `World` 的存档镜像：与 state_hash 同一完备性基准的全部行为字段。
/// 不派生 PartialEq——实体类型无此派生，等价判据是 state_hash（测试
/// 与读档对账均以它为准）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct WorldSnapshot {
    pub tick: u64,
    pub map_w: i32,
    pub map_h: i32,
    /// 静态障碍之外的墙（含 buy dock 开墙后的残余）。
    pub walls: BTreeSet<Position>,
    pub seed: u64,
    /// 装卸位分配流（"dock"）状态字。
    pub rng_dock_words: [u64; 4],
    pub next_id: Id,
    pub gold_milli: MilliGold,
    pub debt_milli: MilliGold,
    pub robots: BTreeMap<Id, Robot>,
    pub shelves: BTreeMap<Id, Shelf>,
    pub chargers: BTreeMap<Id, Charger>,
    pub docks: BTreeMap<Id, Dock>,
    pub vehicles: BTreeMap<Id, Vehicle>,
    pub ground_boxes: BTreeMap<Id, GroundBox>,
    pub listings: BTreeMap<Id, Order>,
    pub my_orders: BTreeMap<Id, Order>,
    pub market: Option<MarketSnapshot>,
    pub arrivals: Vec<ArrivalSnapshot>,
    pub last_results: BTreeMap<Id, LastResultSnapshot>,
}

impl World {
    /// 安全点取快照（docs/architecture/06：存档只在初始化结束且无活动
    /// tick 的安全点取数）。
    pub fn to_snapshot(&self) -> WorldSnapshot {
        debug_assert!(
            self.intents.is_empty(),
            "安全点快照：intents 必为空（settle / end_tick 后调用）"
        );
        WorldSnapshot {
            tick: self.tick,
            map_w: self.map_w,
            map_h: self.map_h,
            walls: self.walls.clone(),
            seed: self.seed,
            rng_dock_words: self.rng_dock.state_words(),
            next_id: self.next_id,
            gold_milli: self.gold_milli,
            debt_milli: self.debt_milli,
            robots: self.robots.clone(),
            shelves: self.shelves.clone(),
            chargers: self.chargers.clone(),
            docks: self.docks.clone(),
            vehicles: self.vehicles.clone(),
            ground_boxes: self.ground_boxes.clone(),
            listings: self.listings.clone(),
            my_orders: self.my_orders.clone(),
            market: self.market.as_ref().map(|m| MarketSnapshot {
                prices: m.prices.clone(),
                rng_words: m.rng.state_words(),
            }),
            arrivals: self
                .arrivals
                .iter()
                .map(|a| ArrivalSnapshot {
                    order_id: a.order_id,
                    arrive_tick: a.arrive_tick,
                })
                .collect(),
            last_results: self
                .last_results
                .iter()
                .map(|(id, r)| {
                    (
                        *id,
                        LastResultSnapshot {
                            action: r.action.to_string(),
                            arg: r.arg.clone(),
                            code: r.code.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// 自快照恢复（读档路径）。恢复的世界不带本 tick 意图。动作名超出
    /// 码表视为存档损坏（值域冻结，ACTION_NAMES）。
    pub fn from_snapshot(s: WorldSnapshot) -> Result<World, String> {
        let mut last_results = BTreeMap::new();
        for (id, r) in s.last_results {
            let Some(action) = ACTION_NAMES.iter().find(|a| **a == r.action) else {
                return Err(format!(
                    "存档包含未知动作名「{}」（规则版本不匹配）",
                    r.action
                ));
            };
            last_results.insert(
                id,
                LastResult {
                    action,
                    arg: r.arg,
                    code: r.code,
                },
            );
        }
        Ok(World {
            tick: s.tick,
            map_w: s.map_w,
            map_h: s.map_h,
            walls: s.walls,
            seed: s.seed,
            rng_dock: Xoshiro256::from_state_words(s.rng_dock_words),
            next_id: s.next_id,
            gold_milli: s.gold_milli,
            debt_milli: s.debt_milli,
            robots: s.robots,
            shelves: s.shelves,
            chargers: s.chargers,
            docks: s.docks,
            vehicles: s.vehicles,
            ground_boxes: s.ground_boxes,
            listings: s.listings,
            my_orders: s.my_orders,
            market: s.market.map(|m| MarketState {
                prices: m.prices,
                rng: Xoshiro256::from_state_words(m.rng_words),
            }),
            arrivals: s
                .arrivals
                .into_iter()
                .map(|a| PendingArrival {
                    order_id: a.order_id,
                    arrive_tick: a.arrive_tick,
                })
                .collect(),
            intents: Vec::new(),
            last_results,
        })
    }
}
