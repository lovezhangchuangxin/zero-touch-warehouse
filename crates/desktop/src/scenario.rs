//! B2 固定场景（docs/architecture/08 原型 B：固定小地图与订单，完成入库、
//! 地面暂存、上架、充电、交付）。数值为 B2 锚点，市场刷新与价格波动属
//! 里程碑 3；买卖价差沿用 B1 验收基线（cargo_loop 低买高卖）。
//!
//! 地图 16×12：北墙两个缺口放装卸位（1×2 占地：缺口锚点格 + 库内第二格，
//! 两格都算障碍、都不铺墙瓦——docs/game-design/02）；中部两排货架留 2 格宽
//! 走廊（五台机器人贪心走位会在此对穿堵塞，供“玩家改码疏堵”验收）；
//! 南墙双充电桩。

use serde_json::{Value, json};
use ztw_model::{OrderSide, Position};
use ztw_sim::World;

/// 装卸位场景规格：锚点格 + 朝向（指向库内第二格）。
pub struct DockSpec {
    pub x: i32,
    pub y: i32,
    pub ext: (i32, i32),
}

pub struct ScenarioSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub map_w: i32,
    pub map_h: i32,
    pub gold_milli: i64,
    pub seed: u64,
    pub robots: &'static [(i32, i32)],
    pub shelves: &'static [(i32, i32)],
    pub chargers: &'static [(i32, i32)],
    pub docks: &'static [DockSpec],
    /// (side, goods, qty, unit_price_milli)。
    pub listings: &'static [(OrderSide, &'static str, u32, i64)],
    /// 启用市场生成器（docs/game-design/09）：基准价随机游走 + 板面刷新；
    /// false = 固定挂单（B2 验收锚，挂单吃光不补）。
    pub market: bool,
}

/// 北墙缺口（装卸位锚点格不铺墙瓦；第二格在库内，本来就不是墙）。
const NORTH_HOLES: [(i32, i32); 2] = [(3, 0), (12, 0)];

/// 周边墙环减缺口。
fn perimeter_walls(w: i32, h: i32) -> Vec<(i32, i32)> {
    let mut walls = Vec::new();
    for x in 0..w {
        walls.push((x, 0));
        walls.push((x, h - 1));
    }
    for y in 1..h - 1 {
        walls.push((0, y));
        walls.push((w - 1, y));
    }
    walls.retain(|c| !NORTH_HOLES.contains(c));
    walls
}

macro_rules! docks {
    ($($x:expr, $y:expr, $ex:expr, $ey:expr;)*) => {
        &[ $(DockSpec { x: $x, y: $y, ext: ($ex, $ey) },)* ]
    };
}

/// 一台机器人：验证单机闭环的最小配置。
pub static B2_ONE: ScenarioSpec = ScenarioSpec {
    id: "b2-one",
    name: "B2 · 单机闭环",
    desc: "一台机器人完成入库、地面暂存、上架、充电与交付。",
    map_w: 16,
    map_h: 12,
    gold_milli: 300_000,
    seed: 20260913,
    robots: &[(2, 3)],
    shelves: &[(5, 3), (6, 3), (7, 3), (5, 6), (6, 6), (7, 6)],
    chargers: &[(6, 10), (7, 10)],
    docks: docks!(3, 0, 0, 1; 12, 0, 0, 1;),
    // 数量按单机能量预算收敛（take6+drop3+pick4+give5，移动不耗电）：
    // 全部六单 ≈ 10 组取放 ≈ 90 电量 < 初始 100，单机无需中途充电。
    listings: &[
        (OrderSide::Sell, "water", 2, 3_800),
        (OrderSide::Sell, "water", 2, 3_800),
        (OrderSide::Buy, "water", 2, 5_200),
        (OrderSide::Buy, "water", 2, 5_200),
        (OrderSide::Sell, "battery", 1, 12_000),
        (OrderSide::Buy, "battery", 1, 18_000),
    ],
    market: false,
};

/// 五台机器人：任务分配与交通规划改变吞吐的对比配置（同一地图）。
pub static B2_FIVE: ScenarioSpec = ScenarioSpec {
    id: "b2-five",
    name: "B2 · 五机吞吐",
    desc: "五台机器人同场：贪心抢道会堵塞，分道与分工显著改变吞吐。",
    map_w: 16,
    map_h: 12,
    gold_milli: 300_000,
    seed: 20260913,
    robots: &[(2, 2), (2, 4), (2, 6), (2, 8), (2, 10)],
    shelves: &[(5, 3), (6, 3), (7, 3), (5, 6), (6, 6), (7, 6)],
    chargers: &[(6, 10), (7, 10)],
    docks: docks!(3, 0, 0, 1; 12, 0, 0, 1;),
    // 数量按单机能量预算收敛（take6+drop3+pick4+give5，移动不耗电）：
    // 全部六单 ≈ 10 组取放 ≈ 90 电量 < 初始 100，单机无需中途充电。
    listings: &[
        (OrderSide::Sell, "water", 2, 3_800),
        (OrderSide::Sell, "water", 2, 3_800),
        (OrderSide::Buy, "water", 2, 5_200),
        (OrderSide::Buy, "water", 2, 5_200),
        (OrderSide::Sell, "battery", 1, 12_000),
        (OrderSide::Buy, "battery", 1, 18_000),
    ],
    market: false,
};

pub static SCENARIOS: [&ScenarioSpec; 2] = [&B2_ONE, &B2_FIVE];
pub const DEFAULT_ID: &str = "b2-one";

pub fn by_id(id: &str) -> Option<&'static ScenarioSpec> {
    SCENARIOS.iter().find(|s| s.id == id).copied()
}

/// 构造世界（添加顺序固定 → id 分配确定）。
pub fn build(spec: &ScenarioSpec) -> World {
    let mut w = World::new_empty(spec.map_w, spec.map_h, spec.gold_milli).with_seed(spec.seed);
    for (x, y) in perimeter_walls(spec.map_w, spec.map_h) {
        w.add_wall(Position::new(x, y));
    }
    for &(x, y) in spec.shelves {
        w.add_shelf(Position::new(x, y));
    }
    for &(x, y) in spec.chargers {
        w.add_charger(Position::new(x, y));
    }
    for p in spec.docks {
        w.add_dock(Position::new(p.x, p.y), p.ext);
    }
    for &(x, y) in spec.robots {
        w.add_robot(Position::new(x, y));
    }
    for &(side, goods, qty, price) in spec.listings {
        w.add_listing(side, goods, qty, price);
    }
    if spec.market {
        w = w.with_market();
    }
    w
}

/// 静态信息（前端一次拉取）：墙格、装卸位（含构造后的权威 id 与朝向）、
/// 地图尺寸。不随 tick 快照重复发送。
pub fn static_json(spec: &ScenarioSpec, world: &World) -> Value {
    let walls: Vec<(i32, i32)> = perimeter_walls(spec.map_w, spec.map_h);
    let docks: Vec<Value> = spec
        .docks
        .iter()
        .map(|p| {
            // 按位置找构造后的权威 id（build 顺序固定，位置一一对应）。
            let id = world
                .docks
                .values()
                .find(|d| d.pos.x == p.x && d.pos.y == p.y)
                .map(|d| d.id)
                .unwrap_or(0);
            json!({ "id": id, "x": p.x, "y": p.y, "ext": [p.ext.0, p.ext.1] })
        })
        .collect();
    json!({
        "id": spec.id,
        "name": spec.name,
        "desc": spec.desc,
        "map_w": spec.map_w,
        "map_h": spec.map_h,
        "walls": walls,
        "docks": docks,
        "scenarios": SCENARIOS.iter().map(|s| json!({
            "id": s.id, "name": s.name, "desc": s.desc, "robots": s.robots.len(),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztw_model::Position;

    /// 场景合法性：机器人在可通行格、装卸位两格成立且可供交互、关键点互不重叠。
    #[test]
    fn scenario_invariants() {
        for spec in SCENARIOS {
            let w = build(spec);
            // 机器人出生点可通行且互不重叠（add_robot 不查占位，靠场景保证）。
            let mut seen = std::collections::HashSet::new();
            for &(x, y) in spec.robots {
                let p = Position::new(x, y);
                assert!(
                    w.statically_passable(p),
                    "{} 机器人出生点 {p:?} 不可通行",
                    spec.id
                );
                assert!(seen.insert((x, y)), "{} 机器人出生点重叠 {p:?}", spec.id);
            }
            // 装卸位锚点在北墙缺口上，两格均为障碍；第二格外侧（交互站位）
            // 可通行，机器人可绕到第二格周围。
            for p in spec.docks {
                assert_eq!(p.y, 0, "装卸位贴北墙");
                assert!(p.ext.0.abs() + p.ext.1.abs() == 1, "ext 须为单位偏移");
                let second = Position::new(p.x + p.ext.0, p.y + p.ext.1);
                assert!(
                    !perimeter_walls(spec.map_w, spec.map_h).contains(&(p.x, p.y)),
                    "锚点格必须开墙"
                );
                assert!(
                    w.statically_passable(second.step(p.ext.0, p.ext.1)),
                    "交互位被占"
                );
                assert!(
                    !w.statically_passable(Position::new(p.x, p.y)),
                    "锚点格应为障碍"
                );
                assert!(!w.statically_passable(second), "第二格应为障碍");
            }
            // 反向闭合：每个缺口都必须有装卸位，否则该格既非墙又非障碍，
            // 机器人可站进周墙缺口。
            for h in NORTH_HOLES {
                assert!(
                    spec.docks.iter().any(|d| d.x == h.0 && d.y == h.1),
                    "缺口 {h:?} 没有对应装卸位"
                );
            }
            // 货架 / 充电桩不压机器人出生点。
            for &(x, y) in spec.shelves {
                assert!(!spec.robots.contains(&(x, y)), "货架压出生点");
            }
            for &(x, y) in spec.chargers {
                assert!(!spec.robots.contains(&(x, y)), "充电桩压出生点");
            }
            // 资金足够吃下全部卖单（B2 无借贷）。
            let buy_cost: i64 = spec
                .listings
                .iter()
                .filter(|(s, _, _, _)| *s == OrderSide::Sell)
                .map(|(_, _, q, p)| *q as i64 * p)
                .sum();
            assert!(spec.gold_milli >= buy_cost, "初始金不足以吃满卖单");
        }
    }

    /// 同一 spec 两次构造的确定性（含 PRNG 派生与 id 分配）。
    #[test]
    fn build_is_deterministic() {
        for spec in SCENARIOS {
            let a = build(spec);
            let b = build(spec);
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }

    /// B2 场景冻结锚：市场生成器合入后，固定挂单场景的初始哈希不得漂移
    /// （five_contrast 的验收基线——tick 81 / 六单完成——依赖板面不变）。
    /// 哈希值随 state_hash 覆盖面演进时须有意更新并复核 B2 验收记录。
    #[test]
    fn b2_initial_hashes_are_frozen() {
        assert_eq!(build(&B2_ONE).state_hash(), 0x1715_4103_b28b_08e9);
        assert_eq!(build(&B2_FIVE).state_hash(), 0x3803_4631_d84b_1230);
    }

    /// B2 场景不启用市场：固定挂单长期不变（吃光不补、无价格演化）。
    #[test]
    fn b2_fixed_listings_do_not_refresh() {
        let mut w = build(&B2_ONE);
        for _ in 0..100 {
            w.boundary_events();
            w.end_tick();
        }
        assert_eq!(w.listings.len(), 6, "固定挂单不增不减");
    }

    #[test]
    fn static_json_carries_walls_and_docks() {
        let w = build(&B2_ONE);
        let j = static_json(&B2_ONE, &w);
        assert_eq!(j["walls"].as_array().unwrap().len(), 50); // 周长 52 − 2 缺口
        let docks = j["docks"].as_array().unwrap();
        assert_eq!(docks.len(), 2);
        assert_eq!(docks[0]["ext"], json!([0, 1]));
        // 权威 id 与世界一致。
        let did = docks[1]["id"].as_u64().unwrap();
        assert!(w.docks.contains_key(&did));
    }
}
