//! M3 经济不变量属性测试（docs/architecture/08 里程碑 3：「把经济不变量
//! 变为属性测试」）。模式沿用 B2 先例（确定性种子枚举 + 小规模穷举），
//! 不引入属性测试依赖：失败即种子复现。
//!
//! 核心不变量（会计恒等式）：测试侧独立于实现维护期望账本，逐操作逐
//! tick 对账——任何双记 / 漏记 / 舍入漂移都会以「账本 ≠ 世界」暴露：
//! 1. gold_milli 恒等于初始值 + 借款 − 还款 − 购置 + 销毁退款 + 取消净额
//!    − 接单付款 + 买单离场收款；
//! 2. debt_milli 恒等于借款 − 还款 + 取消欠款 + 逐 tick 复利利息；
//! 3. gold ≥ 0 恒成立（支出前置校验；docs/game-design/06）。
//!
//! 信用额度只约束借款本身：利息可把欠款推过额度线（否则还款压力无法
//! 形成），故不对 debt 断言上界。

mod common;

use common::{check_invariants, give_robot_a_box};
use ztw_model::{OrderSide, Position, codes};
use ztw_sim::{
    CREDIT_LIMIT_MILLI, INTEREST_DENOMINATOR, INTEREST_NUMERATOR, PRICE_CHARGER, PRICE_DOCK,
    PRICE_ROBOT, PRICE_SHELF, World, Xoshiro256,
};

/// 测试侧期望账本（独立于 World 实现，只消费各 *Effect 与结算前快照）。
#[derive(Default)]
struct Ledger {
    gold: i64,
    debt: i64,
}

/// 风暴世界：16×12 周墙（北墙留两缺口放装卸位）+ 市场启用（挂单自刷新）。
fn storm_world(seed: u64) -> World {
    let mut w = World::new_empty(16, 12, 400_000)
        .with_seed(seed)
        .with_market();
    for x in 0..16 {
        if x != 4 && x != 11 {
            w.add_wall(Position::new(x, 0));
        }
        w.add_wall(Position::new(x, 11));
    }
    for y in 1..11 {
        w.add_wall(Position::new(0, y));
        w.add_wall(Position::new(15, y));
    }
    w.add_dock(Position::new(4, 0), (0, 1));
    w.add_dock(Position::new(11, 0), (0, 1));
    w
}

/// 随机管理操作风暴：固定种子集 × 多 tick，每 tick 用独立驱动的确定性
/// 选择器从 {take, cancel, borrow, repay, destroy, buy, no-op} 取操作；
/// 每笔操作与每个 tick 收尾后都对账。
#[test]
fn accounting_identity_storm() {
    for seed in 0..8u64 {
        let ctx = format!("seed {seed}");
        let mut w = storm_world(seed);
        let mut ledger = Ledger {
            gold: 400_000,
            debt: 0,
        };
        let mut rng = Xoshiro256::derive(seed, "storm");
        // 购置候选位：内部空闲格（机器人非静态障碍，可重复用）与南墙格。
        let interior_spots: Vec<Position> = (2..14)
            .flat_map(|x| (2..10).map(move |y| Position::new(x, y)))
            .collect();
        let mut spot_idx = 0usize;
        let mut wall_idx = 0usize;
        let wall_spots: Vec<Position> = (1..15).map(|x| Position::new(x, 11)).collect();

        for _tick in 0..300 {
            let ops = 1 + rng.below(3);
            for _ in 0..ops {
                match rng.below(12) {
                    0..=3 => {
                        // take：随机侧，卖侧取最便宜（有资金压力）、买侧随机。
                        let side = if rng.below(2) == 0 {
                            OrderSide::Sell
                        } else {
                            OrderSide::Buy
                        };
                        let target: Option<(u64, i64)> = match side {
                            OrderSide::Sell => w
                                .listings
                                .values()
                                .filter(|o| o.side == OrderSide::Sell)
                                .map(|o| (o.id, o.qty as i64 * o.unit_price_milli))
                                .min_by_key(|(_, cost)| *cost),
                            OrderSide::Buy => w
                                .listings
                                .values()
                                .filter(|o| o.side == OrderSide::Buy)
                                .map(|o| (o.id, 0))
                                .next(),
                        };
                        let Some((id, cost)) = target else {
                            continue;
                        };
                        let (code, _eff) = w.manage_take(id);
                        match code {
                            codes::OK => ledger.gold -= cost,
                            codes::NO_FUNDS | codes::NO_FREE_DOCK => {}
                            other => panic!("{ctx}：take 意外码 {other}"),
                        }
                    }
                    4 => {
                        // cancel：随机已接单。
                        let Some(&id) = w.my_orders.keys().next() else {
                            continue;
                        };
                        let side = w.my_orders[&id].side;
                        let (code, eff) = w.manage_cancel(id);
                        if code == codes::OK {
                            let eff = eff.unwrap();
                            match side {
                                OrderSide::Sell => ledger.gold += eff.refund_milli,
                                OrderSide::Buy => {
                                    let pay = eff.fee_milli.min(ledger.gold);
                                    ledger.gold -= pay;
                                    ledger.debt += eff.fee_milli - pay;
                                }
                            }
                        } else {
                            assert_eq!(code, codes::GOODS_MOVED, "{ctx}：cancel 意外码");
                        }
                    }
                    5 | 6 => {
                        // borrow：随机额（覆盖额度边界附近）。
                        let amount = match rng.below(4) {
                            0 => 1,
                            1 => (CREDIT_LIMIT_MILLI - ledger.debt).max(0),
                            2 => (CREDIT_LIMIT_MILLI - ledger.debt).max(0) + 1,
                            _ => 5_000 + rng.below(200_000) as i64,
                        };
                        let (code, eff) = w.manage_borrow(amount);
                        match code {
                            codes::OK => {
                                let eff = eff.unwrap();
                                ledger.gold += eff.amount_milli;
                                ledger.debt += eff.amount_milli;
                            }
                            codes::INVALID_ARGUMENT | codes::CREDIT_EXCEEDED => {}
                            other => panic!("{ctx}：borrow 意外码 {other}"),
                        }
                    }
                    7 => {
                        // repay：随机额（覆盖欠款与余额边界）。
                        let amount = match rng.below(4) {
                            0 => 1,
                            1 => ledger.debt.max(1),
                            2 => ledger.debt.saturating_add(1).max(1),
                            _ => 1 + rng.below(600_000) as i64,
                        };
                        let (code, eff) = w.manage_repay(amount);
                        match code {
                            codes::OK => {
                                let eff = eff.unwrap();
                                ledger.gold -= eff.amount_milli;
                                ledger.debt -= eff.amount_milli;
                            }
                            codes::INVALID_ARGUMENT | codes::NO_FUNDS => {}
                            other => panic!("{ctx}：repay 意外码 {other}"),
                        }
                    }
                    8 => {
                        // destroy：随机空设备（机器人 / 货架 / 充电桩；跳过
                        // 装卸位保接单能力）。
                        let candidates: Vec<u64> = w
                            .robots
                            .values()
                            .filter(|r| r.carry.is_none())
                            .map(|r| r.id)
                            .chain(
                                w.shelves
                                    .values()
                                    .filter(|s| s.box_ids.is_empty())
                                    .map(|s| s.id),
                            )
                            .chain(w.chargers.values().map(|c| c.id))
                            .collect();
                        if candidates.is_empty() {
                            continue;
                        }
                        let id = candidates[rng.below(candidates.len())];
                        let (code, eff) = w.manage_destroy(id);
                        match code {
                            codes::OK => ledger.gold += eff.unwrap().refund_milli,
                            codes::NOT_EMPTY | codes::HAS_VEHICLE => {}
                            other => panic!("{ctx}：destroy 意外码 {other}"),
                        }
                    }
                    9 | 10 => {
                        // buy：内部格轮转（shelf/charger/robot），偶尔南墙 dock。
                        let dock = rng.below(4) == 0;
                        let (kind, pos) = if dock {
                            let p = wall_spots[wall_idx % wall_spots.len()];
                            wall_idx += 1;
                            ("dock", p)
                        } else {
                            let p = interior_spots[spot_idx % interior_spots.len()];
                            spot_idx += 1;
                            match rng.below(3) {
                                0 => ("shelf", p),
                                1 => ("charger", p),
                                _ => ("robot", p),
                            }
                        };
                        let (code, eff) = w.manage_buy(kind, pos.x, pos.y);
                        match code {
                            codes::OK => ledger.gold -= eff.unwrap().price_milli,
                            codes::NO_FUNDS
                            | codes::CELL_BLOCKED
                            | codes::CELL_OCCUPIED
                            | codes::OUT_OF_BOUNDS
                            | codes::NOT_ON_WALL => {}
                            other => panic!("{ctx}：buy 意外码 {other}"),
                        }
                    }
                    _ => {} // no-op
                }
                assert_accounting(&w, &ledger, &ctx);
            }
            w.boundary_events();
            w.settle();
            // 计息（end_tick 内同款公式，账本独立复算）。
            ledger.debt += ledger.debt / INTEREST_DENOMINATOR * INTEREST_NUMERATOR;
            w.end_tick();
            assert_accounting(&w, &ledger, &ctx);
            check_invariants(&w);
        }
    }
}

fn assert_accounting(w: &World, ledger: &Ledger, ctx: &str) {
    assert_eq!(
        w.gold_milli, ledger.gold,
        "{ctx}：金币账本不符（tick {}）",
        w.tick
    );
    assert_eq!(
        w.debt_milli, ledger.debt,
        "{ctx}：欠款账本不符（tick {}）",
        w.tick
    );
    assert!(w.gold_milli >= 0, "{ctx}：金币为负");
    assert!(w.debt_milli >= 0, "{ctx}：欠款为负");
}

/// 订单完成的收款入账：结算路径的会计恒等式（take 买入只扣款不收款的
/// 补全）。一台机器人邻接装卸位交互格：入库车卸空离场（无收款，款项
/// 已付）、出库车装满离场（收款入账）。
#[test]
fn completion_revenue_accounted() {
    // 卖单：卸空离场，无收款。
    let mut w = World::new_empty(10, 8, 200_000);
    let dock = w.add_dock(Position::new(0, 3), (1, 0));
    let robot = w.add_robot(Position::new(2, 3)); // 邻接交互格 (1,3)
    let shelf = w.add_shelf(Position::new(2, 4)); // 卸货暂存（邻接机器人）
    let o = w.add_listing(OrderSide::Sell, "water", 2, 5_000);
    assert_eq!(w.manage_take(o).0, codes::OK);
    let ledger = Ledger {
        gold: 200_000 - 10_000,
        debt: 0,
    };
    assert_accounting(&w, &ledger, "sell 接单");
    w.end_tick();
    w.boundary_events();
    let vid = w.docks[&dock].docked_vehicle.unwrap();
    for _ in 0..2 {
        let bx = w.vehicles[&vid].box_ids[0];
        assert_eq!(w.accept_take(robot, vid, bx), codes::OK);
        w.settle();
        w.end_tick();
        w.boundary_events();
        assert_eq!(w.accept_give(robot, shelf, None), codes::OK);
        w.settle();
        w.end_tick();
        w.boundary_events();
    }
    assert!(!w.vehicles.contains_key(&vid), "卸空即离场");
    assert!(w.docks[&dock].docked_vehicle.is_none(), "装卸位释放");
    assert!(!w.my_orders.contains_key(&o), "订单完成");
    assert_accounting(&w, &ledger, "sell 完成（无收款）");
    check_invariants(&w);

    // 买单：装满离场，收款入账（数量 × 单价）。
    let mut w2 = World::new_empty(10, 8, 200_000);
    let dock2 = w2.add_dock(Position::new(0, 3), (1, 0));
    let robot2 = w2.add_robot(Position::new(2, 3));
    let o2 = w2.add_listing(OrderSide::Buy, "water", 2, 9_000);
    assert_eq!(w2.manage_take(o2).0, codes::OK);
    let mut ledger2 = Ledger {
        gold: 200_000,
        debt: 0,
    };
    w2.end_tick();
    w2.boundary_events();
    let vid2 = w2.docks[&dock2].docked_vehicle.unwrap();
    for i in 0..2 {
        give_robot_a_box(&mut w2, robot2, "water"); // 布置辅助：机器人持有货
        assert_eq!(w2.accept_give(robot2, vid2, None), codes::OK);
        w2.settle();
        if i == 1 {
            ledger2.gold += 18_000; // 第二箱装满 → 同次结算内离场与收款
        }
        w2.end_tick();
        w2.boundary_events();
    }
    assert!(!w2.vehicles.contains_key(&vid2), "装满即离场");
    assert!(!w2.my_orders.contains_key(&o2), "订单完成");
    assert_accounting(&w2, &ledger2, "buy 完成（收款 18_000）");
    check_invariants(&w2);
}

/// 借贷小空间穷举：金额边界 × tick 深度的「借 → 计息 → 还」组合，
/// 逐 tick 断言精确复利与还款钳定（docs/game-design/08 管理操作）。
#[test]
fn borrow_interest_repay_exhaustive_small_matrix() {
    const AMOUNTS: [i64; 5] = [1, 999, 1_000, CREDIT_LIMIT_MILLI - 1, CREDIT_LIMIT_MILLI];
    const TICKS: [u64; 3] = [1, 5, 50];
    const G0: i64 = 5_000_000; // 覆盖上限额度 50 tick 利息的还款能力
    for &amount in &AMOUNTS {
        for &ticks in &TICKS {
            let mut w = World::new_empty(8, 8, G0);
            assert_eq!(w.manage_borrow(amount).0, codes::OK);
            let mut expected = amount;
            let mut egold = G0 + amount;
            for _ in 0..ticks {
                w.boundary_events();
                w.settle();
                w.end_tick();
                expected += expected / INTEREST_DENOMINATOR * INTEREST_NUMERATOR;
                assert_eq!(w.debt_milli, expected, "amount={amount} 复利漂移");
            }
            assert_eq!(w.gold_milli, egold, "计息不动金币");
            // 恰好全额还款 → 清零（还款额以欠款为上限的边界）。
            assert_eq!(w.manage_repay(expected).0, codes::OK);
            egold -= expected;
            assert_eq!(w.debt_milli, 0);
            assert_eq!(w.gold_milli, egold);
            assert_eq!(w.manage_repay(1).0, codes::OK, "清账后还款幂等");
            assert_eq!(w.gold_milli, egold);
            // 重新借至同额再部分还款（amount=1 时 half=0 → 非正数拒绝）。
            assert_eq!(w.manage_borrow(amount).0, codes::OK);
            egold += amount;
            let half = amount / 2;
            if half >= 1 {
                assert_eq!(w.manage_repay(half).0, codes::OK);
                egold -= half;
                assert_eq!(w.debt_milli, amount - half);
                assert_eq!(w.gold_milli, egold);
            } else {
                assert_eq!(w.manage_repay(0).0, codes::INVALID_ARGUMENT);
            }
        }
    }
}

/// 设备价格表一致性：buy 扣款与 destroy 退款围绕同一张价目（docs 06
/// 「销毁退还部分金币」与购置价同源）。
#[test]
fn buy_destroy_price_table_roundtrip() {
    for (kind, price) in [
        ("robot", PRICE_ROBOT),
        ("shelf", PRICE_SHELF),
        ("charger", PRICE_CHARGER),
        ("dock", PRICE_DOCK),
    ] {
        let mut w = World::new_empty(10, 8, 4_000_000);
        for y in 0..8 {
            w.add_wall(Position::new(9, y));
        }
        let pos = if kind == "dock" {
            Position::new(9, 3)
        } else {
            Position::new(3, 3)
        };
        let (code, eff) = w.manage_buy(kind, pos.x, pos.y);
        assert_eq!(code, codes::OK, "{kind} 购置失败");
        let eff = eff.unwrap();
        assert_eq!(eff.price_milli, price, "{kind} 价目不符");
        assert_eq!(w.gold_milli, 4_000_000 - price);
        assert_eq!(w.manage_destroy(eff.id).0, codes::OK);
        assert_eq!(
            w.gold_milli,
            4_000_000 - price + price / 2,
            "{kind} 退款不符"
        );
    }
}
