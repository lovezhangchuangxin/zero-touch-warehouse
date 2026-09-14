//! M3 市场生成器验收（docs/game-design/09「不变量」五条 + 刷新 + 确定性）。
//! 模式沿用 B2 先例：固定种子集 × 逐 tick 断言，不引入属性测试依赖。

use ztw_model::{MilliGold, Order, OrderSide, Position};
use ztw_sim::{GOODS, MARKET_REFRESH_INTERVAL, World};

fn market_world(seed: u64, gold_milli: MilliGold) -> World {
    World::new_empty(12, 12, gold_milli)
        .with_seed(seed)
        .with_market()
}

fn of_side<'a>(w: &'a World, name: &str, side: OrderSide) -> Vec<&'a Order> {
    w.listings
        .values()
        .filter(|o| o.side == side && o.goods_type == name)
        .collect()
}

/// 均值回归平稳标准差 σ / √(2k − k²) 的带宽倍数（不变量 3 的显式界）。
fn band_milli(g: &ztw_sim::GoodsSpec) -> MilliGold {
    let k = g.k_num as f64 / g.k_den as f64;
    let var = (g.sigma_milli as f64 * g.sigma_milli as f64) / (2.0 * k - k * k);
    (4.0 * var.sqrt()) as MilliGold
}

/// 五条市场不变量（docs/game-design/09；优先级高于任何具体数值）。
/// 调用前提：板面未被玩家 take 清空（扫描测试不 take；take 场景只断言
/// 刷新点回升，见 refresh_tops_up_after_take）。
fn check_market_invariants(w: &World, ctx: &str) {
    for g in GOODS {
        let sells = of_side(w, g.name, OrderSide::Sell);
        let buys = of_side(w, g.name, OrderSide::Buy);
        // 4. 每类型双向挂单常驻。
        assert!(
            sells.len() >= g.board_min as usize && buys.len() >= g.board_min as usize,
            "{ctx}：{} 板面 sell={} buy={} 低于常驻下限 {}",
            g.name,
            sells.len(),
            buys.len(),
            g.board_min
        );
        // 5. 买卖数量错配恒成立。
        for o in &sells {
            assert!(
                (g.sell_qty.0..=g.sell_qty.1).contains(&o.qty),
                "{ctx}：{} 卖单数量 {} 越界 {:?}",
                g.name,
                o.qty,
                g.sell_qty
            );
        }
        for o in &buys {
            assert!(
                (g.buy_qty.0..=g.buy_qty.1).contains(&o.qty),
                "{ctx}：{} 买单数量 {} 越界 {:?}",
                g.name,
                o.qty,
                g.buy_qty
            );
        }
        // 1. 任一时刻同类型 bid < ask。2. 价差恒大于取消手续费率（10%）：
        //    (ask − bid) / mid > 10% 的整数等价形式 20(ask − bid) > ask + bid。
        let min_ask = sells.iter().map(|o| o.unit_price_milli).min().unwrap();
        let max_bid = buys.iter().map(|o| o.unit_price_milli).max().unwrap();
        assert!(
            max_bid < min_ask,
            "{ctx}：{} 出现交叉 bid={max_bid} ≥ ask={min_ask}",
            g.name
        );
        assert!(
            20 * (min_ask - max_bid) > min_ask + max_bid,
            "{ctx}：{} 价差率 ≤ 手续费率：ask={min_ask} bid={max_bid}",
            g.name
        );
        // 板面跟踪：报价不得与基准价时代错位（旧低价 bid 与新高价 ask 共存
        // 会把价差拉到跨时代宽度，卖出门永不可及——校准实测暴露的病理，
        // 由刷新的时代带清除 + 半板换手维持）。
        assert!(
            40 * (min_ask - max_bid) <= 13 * (min_ask + max_bid),
            "{ctx}：{} 板面价差率超带宽：ask={min_ask} bid={max_bid}",
            g.name
        );
        // 3. 均值回归保证价格有界且为正（锚价 ± 4 倍平稳标准差）。
        let price = w.market_price(g.name).expect("市场启用");
        assert!(price > 0, "{ctx}：{} 基准价非正 {price}", g.name);
        assert!(
            (price - g.anchor_milli).abs() <= band_milli(g),
            "{ctx}：{} 基准价 {price} 越出锚 {} ± {}",
            g.name,
            g.anchor_milli,
            band_milli(g)
        );
    }
}

/// 推进 n 个完整 tick（边界事件 → 收尾；无机器人意图，结算为空）。
fn advance(w: &mut World, n: u64) {
    for _ in 0..n {
        w.boundary_events();
        w.settle();
        w.end_tick();
    }
}

/// 不变量 1–5：多种子 × 600 tick 逐 tick 扫描（含初始板面）。
#[test]
fn invariants_sweep() {
    for seed in 0..8u64 {
        let mut w = market_world(seed, 1_000_000);
        check_market_invariants(&w, &format!("seed {seed} tick 0 初始板面"));
        for _ in 0..600 {
            let ctx = format!("seed {seed} tick {}", w.tick);
            w.boundary_events();
            check_market_invariants(&w, &ctx);
            w.end_tick();
        }
    }
}

/// 刷新节奏：每 MARKET_REFRESH_INTERVAL tick 替换每型每侧最旧挂单并
/// 补足至采样目标数（板面维护性替换，见 gen.rs 模块注释）。
#[test]
fn refresh_replaces_oldest_and_keeps_board() {
    let mut w = market_world(7, 1_000_000);
    w.boundary_events(); // tick 0 不刷新
    let oldest_sell_water = of_side(&w, "water", OrderSide::Sell)
        .iter()
        .map(|o| o.id)
        .min()
        .unwrap();
    // 推进到首个刷新点：advance 处理 tick 0..=39 的边界，第 41 次边界
    // （tick 40）触发刷新——直接再调一次 boundary_events。
    advance(&mut w, MARKET_REFRESH_INTERVAL);
    w.boundary_events();
    assert!(
        !w.listings.contains_key(&oldest_sell_water),
        "最旧 water 卖单应被板面替换移除"
    );
    check_market_invariants(&w, "刷新后");
    // 两个刷新周期后仍然满足全部不变量。
    advance(&mut w, MARKET_REFRESH_INTERVAL);
    w.boundary_events();
    check_market_invariants(&w, "两个刷新周期后");
}

/// 挂单被接走后板面枯竭，下一刷新点补足常驻下限。
#[test]
fn refresh_tops_up_after_take() {
    let mut w = market_world(3, 10_000_000);
    for y in [1, 3, 5, 7, 9] {
        w.add_dock(Position::new(0, y), (1, 0));
    }
    // 吃光 water 全部卖单（reserve 消耗装卸位，故多放几个）。
    let sells: Vec<u64> = of_side(&w, "water", OrderSide::Sell)
        .iter()
        .map(|o| o.id)
        .collect();
    assert!(!sells.is_empty());
    for id in sells {
        let (code, _) = w.manage_take(id);
        assert_eq!(code, ztw_model::codes::OK);
    }
    assert!(
        of_side(&w, "water", OrderSide::Sell).is_empty(),
        "water 卖单应已吃光"
    );
    // 推进到下一刷新点（tick % 40 == 0 且 tick > 0）。
    advance(&mut w, MARKET_REFRESH_INTERVAL + 1);
    let count = of_side(&w, "water", OrderSide::Sell).len();
    let board_min = GOODS.iter().find(|g| g.name == "water").unwrap().board_min;
    assert!(
        count >= board_min as usize,
        "刷新后 water 卖单应回升至常驻下限，实际 {count}"
    );
    // 其余不变量仍成立（water 买单侧与别的内容物未受影响）。
    check_market_invariants(&w, "回升后");
}

/// 确定性（docs/architecture/02）：同种子两跑逐 tick 哈希一致；
/// 不同种子轨迹不同（"market" 流确实参与演化）。
#[test]
fn determinism_two_runs() {
    let run = |seed: u64| {
        let mut w = market_world(seed, 1_000_000);
        let mut hashes = Vec::new();
        for _ in 0..500 {
            w.boundary_events();
            w.settle();
            w.end_tick();
            hashes.push(w.state_hash());
        }
        hashes
    };
    for seed in [0u64, 42, 20260913] {
        assert_eq!(run(seed), run(seed), "seed {seed} 两跑不一致");
    }
    assert_ne!(run(1)[..100], run(2)[..100], "不同种子应产生不同轨迹");
}

/// 市场状态随世界克隆携带（快照 / 排列测试的前提）。
#[test]
fn market_state_clones_with_world() {
    let w = market_world(5, 1_000_000);
    let cloned = w.clone();
    assert_eq!(w.state_hash(), cloned.state_hash());
    assert!(
        GOODS.iter().all(|g| cloned.market_price(g.name).is_some()),
        "克隆世界应保留全部货物基准价"
    );
}
