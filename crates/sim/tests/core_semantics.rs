//! 核心语义测试（原 lib.rs 内嵌 tests 迁出；仅用公开 API）。
//! move 冲突裁决、市场 / 车辆生命周期冒烟与 code review 加固回归；
//! 完整 B1 回归见 b1_*.rs。

use ztw_model::{Id, OrderSide, Position, VehicleKind, codes};
use ztw_sim::{
    CANCEL_FEE_DENOMINATOR, CHARGE_PER_TICK, ENERGY_COST_DROP, ENERGY_COST_PICK, PRICE_SHELF, World,
};

fn demo_world() -> World {
    let mut w = World::new_empty(10, 10, 100_000);
    w.add_robot(Position::new(1, 1));
    w
}

#[test]
fn accept_static_checks() {
    let mut w = demo_world(); // 机器人 1 在 (1,1)
    let edge = w.add_robot(Position::new(0, 0));
    assert_eq!(w.accept_move(1, 2, 0), codes::INVALID_ARGUMENT);
    assert_eq!(w.accept_move(99, 1, 0), codes::NO_SUCH_OBJECT);
    assert_eq!(w.accept_move(edge, 0, -1), codes::OUT_OF_BOUNDS);
    assert_eq!(w.accept_move(edge, -1, 0), codes::OUT_OF_BOUNDS);
    w.add_wall(Position::new(2, 1));
    assert_eq!(w.accept_move(1, 1, 0), codes::CELL_BLOCKED);
    // 受理失败不消耗行动机会：EAST 被墙挡后，改提 SOUTH 可受理通过。
    assert_eq!(w.accept_move(1, 0, 1), codes::OK);
    // 受理通过即占用本 tick 行动机会，重复提交不静默覆盖。
    assert_eq!(w.accept_move(1, 0, 1), codes::ALREADY_ACTED);
    assert_eq!(w.accept_move(1, 1, 0), codes::ALREADY_ACTED);
}

#[test]
fn simple_move_settles() {
    let mut w = demo_world();
    assert_eq!(w.accept_move(1, 1, 0), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(w.robots[&1].pos, Position::new(2, 1));
    w.end_tick();
    assert_eq!(w.tick, 1);
    // 无受理动作 → last_result 为 None 语义（该机器人无记录）。
    let res = w.settle();
    assert!(!res.contains_key(&1));
}

#[test]
fn contested_cell_min_id_wins_no_promotion() {
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1)); // id 小
    let b = w.add_robot(Position::new(3, 1));
    assert_eq!(w.accept_move(a, 1, 0), codes::OK);
    assert_eq!(w.accept_move(b, -1, 0), codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::OK);
    assert_eq!(res[&b].code, codes::CELL_CONTESTED);
    assert_eq!(w.robots[&a].pos, Position::new(2, 1));
    assert_eq!(w.robots[&b].pos, Position::new(3, 1));
}

#[test]
fn swap_fails_ring_succeeds() {
    // 二元交换：不允许。
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1));
    let b = w.add_robot(Position::new(2, 1));
    assert_eq!(w.accept_move(a, 1, 0), codes::OK);
    assert_eq!(w.accept_move(b, -1, 0), codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
    assert_eq!(res[&b].code, codes::CHAIN_BLOCKED);
    assert_eq!(w.robots[&a].pos, Position::new(1, 1));
    assert_eq!(w.robots[&b].pos, Position::new(2, 1));

    // 网格上最小的“圈”是四格方形轮转（≥3 个机器人允许）。
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1));
    let b = w.add_robot(Position::new(2, 1));
    let c = w.add_robot(Position::new(2, 2));
    let d = w.add_robot(Position::new(1, 2));
    assert_eq!(w.accept_move(a, 1, 0), codes::OK);
    assert_eq!(w.accept_move(b, 0, 1), codes::OK);
    assert_eq!(w.accept_move(c, -1, 0), codes::OK);
    assert_eq!(w.accept_move(d, 0, -1), codes::OK);
    let res = w.settle();
    for id in [a, b, c, d] {
        assert_eq!(res[&id].code, codes::OK, "robot {id}");
    }
    assert_eq!(w.robots[&a].pos, Position::new(2, 1));
    assert_eq!(w.robots[&b].pos, Position::new(2, 2));
    assert_eq!(w.robots[&c].pos, Position::new(1, 2));
    assert_eq!(w.robots[&d].pos, Position::new(1, 1));
}

#[test]
fn chain_blocked_propagates() {
    // c 静止占格，b 试图进 c 的格，a 跟在 b 后面：b、a 均 CHAIN_BLOCKED。
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1));
    let b = w.add_robot(Position::new(2, 1));
    let c = w.add_robot(Position::new(3, 1));
    assert_eq!(w.accept_move(b, 1, 0), codes::OK);
    assert_eq!(w.accept_move(a, 1, 0), codes::OK);
    let res = w.settle();
    assert_eq!(res[&b].code, codes::CHAIN_BLOCKED);
    assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
    assert_eq!(w.robots[&a].pos, Position::new(1, 1));
    let _ = c;
}

#[test]
fn follow_into_vacated_cell() {
    // 前方机器人成功离开 → 后续允许跟进（目标机器人本 tick 成功离开）。
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1));
    let b = w.add_robot(Position::new(2, 1));
    assert_eq!(w.accept_move(b, 1, 0), codes::OK); // b → (3,1)
    assert_eq!(w.accept_move(a, 1, 0), codes::OK); // a → (2,1)（b 离开）
    let res = w.settle();
    assert_eq!(res[&b].code, codes::OK);
    assert_eq!(res[&a].code, codes::OK);
    assert_eq!(w.robots[&a].pos, Position::new(2, 1));
    assert_eq!(w.robots[&b].pos, Position::new(3, 1));
}

#[test]
fn take_order_flow() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let dock = w.add_dock(Position::new(0, 4), (0, 1));
    let o1 = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
    let o2 = w.add_listing(OrderSide::Buy, "chip", 1, 7000);
    let (code, eff) = w.manage_take(o1);
    assert_eq!(code, codes::OK);
    let eff = eff.unwrap();
    assert_eq!(eff.dock_id, dock);
    assert_eq!(w.gold_milli, 1_000_000 - 2 * 5000);
    assert!(w.listings.contains_key(&o2) && !w.listings.contains_key(&o1));
    assert_eq!(w.my_orders[&o1].dock, Some(dock));
    // 车辆下一 tick 边界到场。
    w.end_tick();
    w.boundary_events();
    assert_eq!(w.vehicles.len(), 1);
    let v = w.vehicles.values().next().unwrap();
    assert_eq!(v.kind, VehicleKind::In);
    assert_eq!(v.box_ids.len(), 2);
    assert_eq!(w.docks[&dock].docked_vehicle, Some(v.id));
    assert_eq!(w.my_orders[&o1].vehicle, Some(v.id));
}

#[test]
fn take_rejects() {
    let mut w = World::new_empty(10, 10, 100);
    w.add_dock(Position::new(0, 4), (0, 1));
    let o = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
    assert_eq!(w.manage_take(o).0, codes::NO_FUNDS);
    let mut w2 = World::new_empty(10, 10, 1_000_000);
    let o2 = w2.add_listing(OrderSide::Sell, "battery", 1, 100);
    assert_eq!(w2.manage_take(o2).0, codes::NO_FREE_DOCK);
    assert_eq!(w.manage_take(999).0, codes::ORDER_GONE);
}

#[test]
fn no_promotion_when_winner_dependency_fails() {
    // c 静止占格；a、b 争 c 的格：a（id 小）取得候选但因 c 不离开而
    // CHAIN_BLOCKED；b 保持 CELL_CONTESTED——候选失败不递补。
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1));
    let b = w.add_robot(Position::new(3, 1));
    let c = w.add_robot(Position::new(2, 1)); // id 最大却占着目标格
    assert_eq!(w.accept_move(a, 1, 0), codes::OK);
    assert_eq!(w.accept_move(b, -1, 0), codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
    assert_eq!(res[&b].code, codes::CELL_CONTESTED);
    assert_eq!(w.robots[&a].pos, Position::new(1, 1));
    assert_eq!(w.robots[&b].pos, Position::new(3, 1));
    assert_eq!(w.robots[&c].pos, Position::new(2, 1));
}

/// 受理集合与结算结果与提交顺序无关：≤5 台全排列（docs/architecture/08
/// 原型 B 回归的 move 子集；混合动作场景见 b1_regression.rs）。
#[test]
fn settlement_permutation_invariant() {
    use ztw_model::EAST;
    // 布局：5 台机器人，两两争抢两个格、含一条链与一个静止占位者。
    let build = |w: &mut World| -> Vec<Id> {
        let mut ids = Vec::new();
        for (x, y) in [(1, 1), (3, 1), (5, 1), (2, 1), (4, 1)] {
            ids.push(w.add_robot(Position::new(x, y)));
        }
        ids
    };
    let dirs = [EAST, (0, 1), (-1, 0), EAST, (0, 1)];
    let accepts = |w: &mut World, ids: &Vec<Id>| -> Vec<&'static str> {
        ids.iter()
            .zip(dirs)
            .map(|(id, d)| w.accept_move(*id, d.0, d.1))
            .collect()
    };
    let baseline = {
        let mut w = World::new_empty(10, 10, 0);
        let ids = build(&mut w);
        let codes0 = accepts(&mut w, &ids);
        w.settle();
        (w.state_hash(), w.robots.clone(), codes0)
    };
    let mut perm = [0usize, 1, 2, 3, 4];
    loop {
        let mut w = World::new_empty(10, 10, 0);
        let ids = build(&mut w);
        let mut codes_p = vec![ztw_model::codes::OK; 5];
        for &i in &perm {
            let d = dirs[i];
            codes_p[i] = w.accept_move(ids[i], d.0, d.1);
        }
        w.settle();
        assert_eq!(w.state_hash(), baseline.0, "perm {perm:?}");
        assert_eq!(w.robots, baseline.1);
        assert_eq!(codes_p, baseline.2, "受理集合也必须一致：perm {perm:?}");
        if !next_perm(&mut perm) {
            break;
        }
    }
}

fn next_perm(p: &mut [usize; 5]) -> bool {
    let n = p.len();
    let mut i = n - 1;
    while i > 0 && p[i - 1] >= p[i] {
        i -= 1;
    }
    if i == 0 {
        return false;
    }
    let mut j = n - 1;
    while p[j] <= p[i - 1] {
        j -= 1;
    }
    p.swap(i - 1, j);
    p[i..].reverse();
    true
}

// ------------------------------------------------------------------
// B1 核心语义冒烟（完整回归见 tests/b1_*.rs）
// ------------------------------------------------------------------

#[test]
fn charge_flow_and_contention() {
    let mut w = demo_world();
    let c = w.add_charger(Position::new(1, 2)); // 与 (1,1) 相邻
    let other = w.add_robot(Position::new(1, 3)); // 也相邻 (1,2)
    w.robots.get_mut(&1).unwrap().energy = 40;
    assert_eq!(w.accept_charge(1), codes::OK);
    assert_eq!(w.accept_charge(other), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(res[&other].code, codes::CHARGER_BUSY);
    assert_eq!(w.robots[&1].energy, 40 + CHARGE_PER_TICK);
    assert_eq!(w.robots[&other].energy, 100);
    // 每 tick 重新申请：下一 tick 另一台可充。
    w.end_tick();
    assert_eq!(w.accept_charge(other), codes::OK);
    let res = w.settle();
    assert_eq!(res[&other].code, codes::OK);
    let _ = c;
}

#[test]
fn pick_drop_and_energy_costs() {
    let mut w = demo_world();
    let b = w.add_ground_box("water", Position::new(2, 1));
    assert_eq!(w.accept_pick(1, 2, 1), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(w.robots[&1].carry, Some(b));
    assert_eq!(w.robots[&1].energy, 100 - ENERGY_COST_PICK);
    // 放下是下一个 tick 的动作（每 tick 至多一个动作）。
    w.end_tick();
    assert_eq!(w.accept_drop(1, 2, 1, None), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(w.robots[&1].carry, None);
    assert_eq!(w.ground_boxes[&b].pos, Position::new(2, 1));
    assert_eq!(w.ground_boxes[&b].holder, None);
    assert_eq!(
        w.robots[&1].energy,
        100 - ENERGY_COST_PICK - ENERGY_COST_DROP
    );
}

#[test]
fn in_vehicle_emptying_departs() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let dock = w.add_dock(Position::new(0, 4), (0, 1));
    let o = w.add_listing(OrderSide::Sell, "battery", 1, 5000);
    w.manage_take(o);
    w.end_tick();
    w.boundary_events();
    let vid = w.docks[&dock].docked_vehicle.expect("车辆已到场");
    let v = &w.vehicles[&vid];
    let box_id = v.box_ids[0];
    // 车停在装卸位 (0,4)+(0,5)，机器人 (1,5) 与交互格 (0,5) 相邻，卸货即可清空。
    let r = w.add_robot(Position::new(1, 5));
    assert_eq!(w.accept_take(r, vid, box_id), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    // 同一结算内：车辆离场、订单完成、装卸位释放。
    assert!(!w.vehicles.contains_key(&vid));
    assert!(!w.my_orders.contains_key(&o));
    assert_eq!(w.docks[&dock].docked_vehicle, None);
    assert_eq!(w.robots[&r].carry, Some(box_id));
    assert_eq!(w.gold_milli, 1_000_000 - 5000); // 卖单无离场收款
}

#[test]
fn out_vehicle_filling_departs_and_pays() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let dock = w.add_dock(Position::new(0, 4), (0, 1));
    let o = w.add_listing(OrderSide::Buy, "chip", 1, 7000);
    w.manage_take(o);
    w.end_tick();
    w.boundary_events();
    let vid = w.docks[&dock].docked_vehicle.unwrap();
    let r = w.add_robot(Position::new(1, 5));
    let b = w.add_ground_box("chip", Position::new(2, 5));
    assert_eq!(w.accept_pick(r, 2, 5), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    w.end_tick();
    assert_eq!(w.accept_give(r, vid, None), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    assert!(!w.vehicles.contains_key(&vid));
    assert!(!w.my_orders.contains_key(&o));
    assert_eq!(w.gold_milli, 1_000_000 + 7000); // 买单不预付、装满收款
    assert!(!w.ground_boxes.contains_key(&b)); // 货随车离场
    assert_eq!(w.robots[&r].carry, None);
}

#[test]
fn cancel_restores_and_charges_fee() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let dock = w.add_dock(Position::new(0, 4), (0, 1));
    let o = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
    w.manage_take(o);
    let paid = w.gold_milli;
    // 车辆未到场（初态）即可取消，退回购价扣 10% 手续费。
    let (code, eff) = w.manage_cancel(o);
    assert_eq!(code, codes::OK);
    let eff = eff.unwrap();
    assert_eq!(eff.fee_milli, 2 * 5000 / CANCEL_FEE_DENOMINATOR);
    assert_eq!(
        w.gold_milli,
        paid + 2 * 5000 - 2 * 5000 / CANCEL_FEE_DENOMINATOR
    );
    assert!(!w.my_orders.contains_key(&o));
    assert_eq!(w.docks[&dock].reserved_for, None);
    w.end_tick();
    w.boundary_events();
    assert!(w.vehicles.is_empty()); // arrival 已撤销
}

#[test]
fn destroy_rejects_and_refunds() {
    let mut w = demo_world();
    let s = w.add_shelf(Position::new(4, 4));
    let gold = w.gold_milli;
    assert_eq!(w.manage_destroy(s).0, codes::OK);
    assert_eq!(w.gold_milli, gold + PRICE_SHELF / 2);
    assert_eq!(w.manage_destroy(999).0, codes::NO_SUCH_OBJECT);
    assert_eq!(w.manage_destroy(1).0, codes::OK); // 空载机器人可毁
}

// ------------------------------------------------------------------
// code review 加固回归（P1/P2 修复锁定）
// ------------------------------------------------------------------

#[test]
fn end_tick_drops_stale_intents() {
    // 跳过 settle 直接 end_tick：陈旧意图不跨 tick 执行、不锁死行动机会。
    let mut w = demo_world();
    assert_eq!(w.accept_move(1, 1, 0), codes::OK); // EAST
    w.end_tick();
    assert_eq!(w.accept_move(1, 0, 1), codes::OK); // SOUTH 未被 ALREADY_ACTED 挡
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(w.robots[&1].pos, Position::new(1, 2)); // 执行 SOUTH，EAST 未复活
}

#[test]
fn late_boundary_event_still_arrives() {
    // 跳过一次 boundary_events 后，迟到事件补发、装卸位不悬挂。
    let mut w = World::new_empty(10, 10, 1_000_000);
    let dock = w.add_dock(Position::new(0, 4), (0, 1));
    let o = w.add_listing(OrderSide::Sell, "battery", 1, 5000);
    w.manage_take(o);
    w.end_tick();
    w.end_tick(); // 跳过 tick 1 的边界处理
    w.boundary_events(); // tick 2 补发
    let vid = w.docks[&dock].docked_vehicle;
    assert!(vid.is_some(), "迟到到场事件应补发");
    assert_eq!(w.my_orders[&o].vehicle, vid);
}

#[test]
fn dock_footprint_blocks_and_destroy_frees() {
    // 装卸位两格（锚点 + 库内第二格）均为静态障碍：机器人不能走进任一格、
    // 不能 drop 到第二格；销毁装卸位后两格恢复可通行。
    let mut w = World::new_empty(10, 10, 1_000_000);
    let dock = w.add_dock(Position::new(1, 4), (0, 1)); // 足迹 (1,4)+(1,5)
    let r = w.add_robot(Position::new(2, 5));
    assert_eq!(w.accept_move(r, -1, 0), codes::CELL_BLOCKED); // 第二格
    assert!(
        !w.statically_passable(Position::new(1, 4)),
        "锚点格应为障碍"
    );
    let bx = w.add_ground_box("battery", Position::new(2, 4));
    assert_eq!(w.accept_pick(r, 2, 4), codes::OK);
    w.settle();
    w.end_tick();
    assert_eq!(w.accept_drop(r, 1, 5, None), codes::CELL_BLOCKED); // 第二格
    // 销毁装卸位：两格同时解除占用、恢复可通行。
    assert_eq!(w.manage_destroy(dock).0, codes::OK);
    assert!(w.statically_passable(Position::new(1, 4)));
    assert!(w.statically_passable(Position::new(1, 5)));
    assert_eq!(w.accept_move(r, -1, 0), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    assert_eq!(w.robots[&r].pos, Position::new(1, 5));
    assert_eq!(w.robots[&r].carry, Some(bx));
}

#[test]
fn swap_pair_with_follower_both_id_orders() {
    // 交换对 A↔B 与跟进 C（目标 = A 原格）：交换判负在不动点前完成，
    // 失败沿链传播。两种 id 次序的结局都必须确定。
    //
    // C id 最小：C 赢 (1,1) 落点候选，B 出局 CELL_CONTESTED → 交换配对
    // 消失；A 的目标 (2,1) 仍被 B 占据且 B 不离开 → A CHAIN_BLOCKED；
    // C 依赖 A 离开 → 链传播同样失败。
    {
        let mut w = World::new_empty(10, 10, 0);
        let c = w.add_robot(Position::new(0, 1)); // id 最小，向东进 (1,1)
        let a = w.add_robot(Position::new(1, 1)); // 向东
        let b = w.add_robot(Position::new(2, 1)); // 向西（与 A 交换）
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        assert_eq!(w.accept_move(c, 1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&c].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&b].code, codes::CELL_CONTESTED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        assert_eq!(w.robots[&b].pos, Position::new(2, 1));
        assert_eq!(w.robots[&c].pos, Position::new(0, 1));
    }
    // C id 最大：B 赢 (1,1) 候选 → 交换判负传播，C 落点竞争出局。
    {
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        let c = w.add_robot(Position::new(0, 1)); // id 最大
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        assert_eq!(w.accept_move(c, 1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&b].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&c].code, codes::CELL_CONTESTED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        assert_eq!(w.robots[&b].pos, Position::new(2, 1));
        assert_eq!(w.robots[&c].pos, Position::new(0, 1));
    }
}
