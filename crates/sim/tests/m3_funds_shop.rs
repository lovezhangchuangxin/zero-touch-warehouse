//! M3 借贷与商店验收（docs/game-design/06 经营、商店与销毁 / 08 管理操作）：
//! borrow / repay 全码路径与金额边界矩阵、计息复利精确性、buy 四类对象的
//! 放置 / 资金 / 错误码矩阵（含 dock 朝向推导）。

mod common;

use common::check_invariants;
use ztw_model::{OrderSide, Position, codes};
use ztw_sim::{
    CREDIT_LIMIT_MILLI, INTEREST_DENOMINATOR, INTEREST_NUMERATOR, PRICE_ROBOT, PRICE_SHELF, World,
};

#[test]
fn borrow_repay_code_paths_and_boundaries() {
    let mut w = World::new_empty(8, 8, 500_000);
    // 非正数拒绝。
    assert_eq!(w.manage_borrow(0).0, codes::INVALID_ARGUMENT);
    assert_eq!(w.manage_borrow(-1).0, codes::INVALID_ARGUMENT);
    assert_eq!(w.manage_repay(0).0, codes::INVALID_ARGUMENT);
    // 基本路径：借款到账、还款同减。
    assert_eq!(w.manage_borrow(200_000).0, codes::OK);
    assert_eq!(w.gold_milli, 700_000);
    assert_eq!(w.debt_milli, 200_000);
    assert_eq!(w.manage_repay(50_000).0, codes::OK);
    assert_eq!(w.gold_milli, 650_000);
    assert_eq!(w.debt_milli, 150_000);
    // 还款金额以欠款为上限：超额部分静默钳定（docs 08「金额以欠款为上限」）。
    assert_eq!(w.manage_repay(999_000_000).0, codes::OK);
    assert_eq!(w.debt_milli, 0);
    assert_eq!(w.gold_milli, 500_000);
    // 无欠款时还款：钳定为 0，OK 且无变化（幂等清账）。
    assert_eq!(w.manage_repay(1).0, codes::OK);
    assert_eq!(w.debt_milli, 0);
    assert_eq!(w.gold_milli, 500_000);
}

#[test]
fn borrow_credit_limit_boundaries() {
    let mut w = World::new_empty(8, 8, 0);
    // 恰好到额度：通过。
    assert_eq!(w.manage_borrow(CREDIT_LIMIT_MILLI).0, codes::OK);
    assert_eq!(w.debt_milli, CREDIT_LIMIT_MILLI);
    assert_eq!(w.gold_milli, CREDIT_LIMIT_MILLI);
    // 再借 1 milli：超额拒绝，状态不变。
    assert_eq!(w.manage_borrow(1).0, codes::CREDIT_EXCEEDED);
    assert_eq!(w.debt_milli, CREDIT_LIMIT_MILLI);
    // 还掉一点后恰好可再借等额。
    assert_eq!(w.manage_repay(1_000).0, codes::OK);
    assert_eq!(w.manage_borrow(1_000).0, codes::OK);
    assert_eq!(w.debt_milli, CREDIT_LIMIT_MILLI);
    // 额度须高于购置一台机器人的解围成本（docs 06 冻结前提；编译期钉住）。
    const _: () = assert!(CREDIT_LIMIT_MILLI > PRICE_ROBOT);
}

#[test]
fn repay_no_funds_boundary() {
    let mut w = World::new_empty(8, 8, 10_000);
    assert_eq!(w.manage_borrow(50_000).0, codes::OK);
    // 花光金币（买入扣款到恰好 0），欠款还在：还款 NO_FUNDS、不自动借贷。
    let o = w.add_listing(OrderSide::Sell, "water", 4, 15_000); // 60 gold = 全部余额
    w.add_dock(Position::new(0, 3), (1, 0));
    assert_eq!(w.manage_take(o).0, codes::OK);
    assert_eq!(w.gold_milli, 0);
    assert_eq!(w.manage_repay(1).0, codes::NO_FUNDS);
    assert_eq!(w.debt_milli, 50_000);
}

/// 计息：每 tick 结算完成后按欠款 × 利率复利（整数、先除后乘向下取整）。
#[test]
fn interest_compounds_exactly() {
    let mut w = World::new_empty(8, 8, 0);
    w.manage_borrow(1_000_000);
    let mut expected = 1_000_000i64;
    for _ in 0..50 {
        w.boundary_events();
        w.settle();
        w.end_tick();
        expected += expected / INTEREST_DENOMINATOR * INTEREST_NUMERATOR;
        assert_eq!(w.debt_milli, expected, "计息偏离期望值");
    }
    // 欠款不足 1 milli 利息时利息为 0（向下取整）。
    let mut tiny = World::new_empty(8, 8, 0);
    tiny.manage_borrow(500);
    tiny.end_tick();
    assert_eq!(tiny.debt_milli, 500, "0.5 gold 欠款利息向下取整为 0");
}

/// 四类对象的购买矩阵：放置校验次序（越界 → 地面货物 → 静态障碍 → 资金）。
#[test]
fn buy_placement_and_funds_matrix() {
    // 初始金覆盖货架 + 充电桩 + 机器人（175 + 300 + 650 = 1125 gold）。
    let mut w = World::new_empty(8, 8, 1_200_000);
    w.add_shelf(Position::new(3, 3));
    w.add_ground_box("water", Position::new(4, 4));

    // 未知 kind。
    assert_eq!(w.manage_buy("crane", 2, 2).0, codes::INVALID_ARGUMENT);
    // 越界。
    assert_eq!(w.manage_buy("shelf", -1, 2).0, codes::OUT_OF_BOUNDS);
    assert_eq!(w.manage_buy("shelf", 8, 2).0, codes::OUT_OF_BOUNDS);
    // 地面货物占格。
    assert_eq!(w.manage_buy("shelf", 4, 4).0, codes::CELL_OCCUPIED);
    // 静态障碍（既有货架）。
    assert_eq!(w.manage_buy("shelf", 3, 3).0, codes::CELL_BLOCKED);

    // 成功路径：货架，即时扣费、即时建成、blocked 生效。
    let (code, eff) = w.manage_buy("shelf", 2, 2);
    assert_eq!(code, codes::OK);
    let eff = eff.unwrap();
    assert_eq!(eff.price_milli, PRICE_SHELF);
    assert_eq!(eff.blocked_cells, vec![Position::new(2, 2)]);
    assert_eq!(w.gold_milli, 1_200_000 - PRICE_SHELF);
    assert!(w.shelves.contains_key(&eff.id));
    assert!(!w.statically_passable(Position::new(2, 2)));
    // 充电桩同理；机器人满电出生、不占静态格。
    assert_eq!(w.manage_buy("charger", 5, 5).0, codes::OK);
    assert!(!w.statically_passable(Position::new(5, 5)));
    let (code, eff) = w.manage_buy("robot", 6, 6);
    assert_eq!(code, codes::OK);
    let robot = &w.robots[&eff.unwrap().id];
    assert_eq!(robot.energy, robot.energy_max);
    assert!(w.statically_passable(Position::new(6, 6)));
    check_invariants(&w);

    // 资金不足：放置合法但钱不够（放置校验在前、资金在后）。
    let mut poor = World::new_empty(8, 8, PRICE_SHELF - 1);
    assert_eq!(poor.manage_buy("shelf", 2, 2).0, codes::NO_FUNDS);
    assert_eq!(poor.manage_buy("robot", 2, 2).0, codes::NO_FUNDS);
    assert!(poor.shelves.is_empty() && poor.robots.is_empty());
    assert_eq!(poor.gold_milli, PRICE_SHELF - 1); // 失败不扣费
}

/// dock 购买：朝向唯一推导、角格歧义与非墙格拒绝、锚点开墙。
#[test]
fn buy_dock_orientation_and_wall_rules() {
    let mut w = World::new_empty(8, 6, 2_500_000);
    // 周边墙环，(4,0) 留洞制造「边界格但无墙」用例。
    for x in 0..8 {
        if x != 4 {
            w.add_wall(Position::new(x, 0));
        }
        w.add_wall(Position::new(x, 5));
    }
    for y in 1..5 {
        w.add_wall(Position::new(0, y));
        w.add_wall(Position::new(7, y));
    }
    // 内部格：不在边界上。
    assert_eq!(w.manage_buy("dock", 3, 3).0, codes::NOT_ON_WALL);
    // 边界格但不是墙。
    assert_eq!(w.manage_buy("dock", 4, 0).0, codes::NOT_ON_WALL);
    // 四个角格：两个内向法向歧义。
    assert_eq!(w.manage_buy("dock", 0, 0).0, codes::NOT_ON_WALL);
    assert_eq!(w.manage_buy("dock", 7, 5).0, codes::NOT_ON_WALL);
    assert_eq!(w.manage_buy("dock", 7, 0).0, codes::NOT_ON_WALL);
    // 北墙非角格：ext 唯一推导为 (0,1)，锚点开墙、两格成障。
    let (code, eff) = w.manage_buy("dock", 5, 0);
    assert_eq!(code, codes::OK);
    let eff = eff.unwrap();
    assert_eq!(eff.ext, Some((0, 1)));
    assert_eq!(
        eff.blocked_cells,
        vec![Position::new(5, 0), Position::new(5, 1)]
    );
    assert_eq!(w.gold_milli, 2_500_000 - 500_000);
    assert!(!w.statically_passable(Position::new(5, 0)));
    assert!(!w.statically_passable(Position::new(5, 1)));
    assert_eq!(w.docks[&eff.id].ext, (0, 1));
    // 墙已开：同格重复购买 NOT_ON_WALL。
    assert_eq!(w.manage_buy("dock", 5, 0).0, codes::NOT_ON_WALL);
    // 东墙：ext (-1,0)；西墙：ext (1,0)；南墙：ext (0,-1)。
    let (_, e2) = w.manage_buy("dock", 7, 2);
    assert_eq!(e2.unwrap().ext, Some((-1, 0)));
    let (_, e3) = w.manage_buy("dock", 0, 2);
    assert_eq!(e3.unwrap().ext, Some((1, 0)));
    let (_, e4) = w.manage_buy("dock", 2, 5);
    assert_eq!(e4.unwrap().ext, Some((0, -1)));
    check_invariants(&w);

    // 第二格被静态障碍 / 地面货物挡住。
    let mut w2 = World::new_empty(8, 6, 2_000_000);
    for y in 0..6 {
        w2.add_wall(Position::new(7, y));
    }
    w2.add_shelf(Position::new(6, 1));
    assert_eq!(w2.manage_buy("dock", 7, 1).0, codes::CELL_BLOCKED);
    w2.add_ground_box("water", Position::new(6, 2));
    assert_eq!(w2.manage_buy("dock", 7, 2).0, codes::CELL_OCCUPIED);
    assert_eq!(w2.manage_buy("dock", 7, 4).0, codes::OK);
}

/// 已购对象可销毁回收；借贷 → 购机 → 销毁 → 还款的解围闭环资金守恒
/// （docs 06：借贷是重启经营的保底）。
#[test]
fn bought_objects_destroyable_and_rescue_loop() {
    let mut w = World::new_empty(8, 8, 1_000_000);
    let (_, eff) = w.manage_buy("shelf", 2, 2);
    let id = eff.unwrap().id;
    assert_eq!(w.manage_destroy(id).0, codes::OK);
    assert!(w.shelves.is_empty());
    assert_eq!(w.gold_milli, 1_000_000 - PRICE_SHELF + PRICE_SHELF / 2); // 半价退款
    assert!(w.statically_passable(Position::new(2, 2)));

    // 零金币零库存重启：借款购机 → 销毁回半价 → 还掉可还的部分。
    let mut w2 = World::new_empty(8, 8, 0);
    assert_eq!(w2.manage_borrow(PRICE_ROBOT).0, codes::OK);
    let (_, eff) = w2.manage_buy("robot", 3, 3);
    assert_eq!(w2.robots.len(), 1);
    assert_eq!(w2.gold_milli, 0);
    assert_eq!(w2.manage_destroy(eff.unwrap().id).0, codes::OK);
    assert_eq!(w2.gold_milli, PRICE_ROBOT / 2);
    // 全额还款超出金币 → NO_FUNDS；还掉可还的半价。
    assert_eq!(w2.manage_repay(PRICE_ROBOT).0, codes::NO_FUNDS);
    assert_eq!(w2.manage_repay(PRICE_ROBOT / 2).0, codes::OK);
    assert_eq!(w2.gold_milli, 0);
    assert_eq!(w2.debt_milli, PRICE_ROBOT / 2);
}

/// 购买的装卸位可立即接单（docs 08「新购对象当 tick 即可查询并使用」）。
#[test]
fn bought_dock_usable_same_tick() {
    let mut w = World::new_empty(8, 6, 2_000_000);
    for y in 0..6 {
        w.add_wall(Position::new(0, y));
    }
    let (code, eff) = w.manage_buy("dock", 0, 3);
    assert_eq!(code, codes::OK);
    let dock_id = eff.unwrap().id;
    let o = w.add_listing(OrderSide::Sell, "water", 2, 3_000);
    let (code, _) = w.manage_take(o);
    assert_eq!(code, codes::OK);
    assert_eq!(w.my_orders[&o].dock, Some(dock_id), "新购装卸位可立即接单");
}
