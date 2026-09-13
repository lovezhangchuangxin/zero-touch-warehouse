//! B1 货物动作语义：take / give / pick / drop 的受理静态检查与结算资源裁决
//! （docs/game-design/03 动作受理与机器人间转交）。

mod common;

use common::*;
use ztw_model::{OrderSide, Position, codes};
use ztw_sim::{ENERGY_COST_GIVE, ENERGY_COST_TAKE, World};

#[test]
fn take_accept_static_checks() {
    let mut w = demo_world();
    let shelf = w.add_shelf(Position::new(2, 1)); // 与 (1,1) 相邻
    let bx = put_box_on_shelf(&mut w, shelf, "water");
    let far = w.add_shelf(Position::new(8, 8));
    let charger = w.add_charger(Position::new(1, 2));

    assert_eq!(w.accept_take(1, far, bx), codes::NOT_ADJACENT);
    assert_eq!(w.accept_take(1, shelf, 999), codes::BOX_NOT_FOUND);
    assert_eq!(w.accept_take(1, charger, bx), codes::INVALID_TARGET);
    assert_eq!(w.accept_take(1, 999, bx), codes::NO_SUCH_OBJECT);
    assert_eq!(w.accept_take(99, shelf, bx), codes::NO_SUCH_OBJECT);

    // 电量不足属受理阶段失败，且不消耗行动机会。
    set_energy(&mut w, 1, ztw_sim::ENERGY_COST_TAKE - 1);
    assert_eq!(w.accept_take(1, shelf, bx), codes::NOT_ENOUGH_ENERGY);

    // 已携带 → LOADED；同样不占用行动机会。
    give_robot_a_box(&mut w, 1, "water");
    assert_eq!(w.accept_take(1, shelf, bx), codes::LOADED);
    w.robots.get_mut(&1).unwrap().carry = None;
    set_energy(&mut w, 1, 100);
    assert_eq!(w.accept_take(1, shelf, bx), codes::OK);
    assert_eq!(w.accept_take(1, shelf, bx), codes::ALREADY_ACTED);
}

#[test]
fn give_accept_static_checks() {
    let mut w = demo_world();
    let shelf = w.add_shelf(Position::new(2, 1));
    let far = w.add_shelf(Position::new(8, 8));
    give_robot_a_box(&mut w, 1, "water");

    assert_eq!(w.accept_give(1, far, None), codes::NOT_ADJACENT);
    assert_eq!(w.accept_give(1, shelf, Some(999)), codes::BOX_NOT_FOUND); // 不在自身携带
    assert_eq!(w.accept_give(99, shelf, None), codes::NO_SUCH_OBJECT);

    // 货架满（受理按快照判定，不预测同 tick 取出腾位）。
    let full = w.add_shelf_sized(Position::new(1, 2), 0);
    assert_eq!(w.accept_give(1, full, None), codes::TARGET_FULL);

    // 车辆类型不符 / 接收机器人非空载。车停 (0,5)，用相邻机器人 (1,5) 交互。
    let port = w.add_port(Position::new(0, 5));
    let o = w.add_listing(OrderSide::Buy, "chip", 2, 7000);
    w.manage_take(o);
    w.end_tick();
    w.boundary_events();
    let vid = w.ports[&port].docked_vehicle.unwrap();
    let dock_worker = w.add_robot(Position::new(1, 5));
    let chip_box = give_robot_a_box(&mut w, dock_worker, "water"); // 类型不符
    assert_eq!(
        w.accept_give(dock_worker, vid, Some(chip_box)),
        codes::WRONG_GOODS
    );
    let loaded = w.add_robot(Position::new(1, 0)); // 与 (1,1) 相邻
    give_robot_a_box(&mut w, loaded, "water");
    assert_eq!(w.accept_give(1, loaded, None), codes::TARGET_FULL);

    // 空载提交 give / 电量不足。
    w.robots.get_mut(&1).unwrap().carry = None;
    assert_eq!(w.accept_give(1, shelf, None), codes::NOT_CARRYING);
    let bx2 = give_robot_a_box(&mut w, 1, "water");
    set_energy(&mut w, 1, ENERGY_COST_GIVE - 1);
    assert_eq!(w.accept_give(1, shelf, Some(bx2)), codes::NOT_ENOUGH_ENERGY);
}

#[test]
fn pick_drop_accept_static_checks() {
    let mut w = demo_world();
    w.add_ground_box("water", Position::new(2, 1));
    assert_eq!(w.accept_pick(1, 10, 10), codes::OUT_OF_BOUNDS);
    assert_eq!(w.accept_pick(1, 3, 1), codes::NOT_ADJACENT);
    assert_eq!(w.accept_pick(1, 1, 2), codes::BOX_NOT_FOUND); // 相邻但无箱
    give_robot_a_box(&mut w, 1, "water");
    assert_eq!(w.accept_pick(1, 2, 1), codes::LOADED);
    w.robots.get_mut(&1).unwrap().carry = None;

    // drop：越界 / 静态障碍 / 地面已有箱 / 不相邻 / 空载。
    w.add_wall(Position::new(1, 2)); // 与 (1,1) 相邻
    give_robot_a_box(&mut w, 1, "water");
    assert_eq!(w.accept_drop(1, 10, 10, None), codes::OUT_OF_BOUNDS);
    assert_eq!(w.accept_drop(1, 1, 2, None), codes::CELL_BLOCKED);
    assert_eq!(w.accept_drop(1, 2, 1, None), codes::CELL_OCCUPIED); // 地面有箱
    assert_eq!(w.accept_drop(1, 3, 3, None), codes::NOT_ADJACENT);
    w.robots.get_mut(&1).unwrap().carry = None;
    assert_eq!(w.accept_drop(1, 1, 2, None), codes::NOT_CARRYING);
}

#[test]
fn shelf_to_robot_handover_with_energy() {
    // 机器人 A 从货架取箱 → 下一 tick 转交给机器人 B（被动接收，B 无动作）。
    let mut w = demo_world();
    let shelf = w.add_shelf(Position::new(2, 1));
    let bx = put_box_on_shelf(&mut w, shelf, "battery");
    let b = w.add_robot(Position::new(1, 2)); // 与 (1,1) 相邻
    let e0 = w.robots[&1].energy;

    assert_eq!(w.accept_take(1, shelf, bx), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(w.robots[&1].carry, Some(bx));
    assert_eq!(w.shelves[&shelf].box_ids.len(), 0);
    assert_eq!(w.robots[&1].energy, e0 - ENERGY_COST_TAKE);
    check_invariants(&w);

    w.end_tick();
    assert_eq!(w.accept_give(1, b, None), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert!(!res.contains_key(&b)); // 被动方无动作记录
    assert_eq!(w.robots[&b].carry, Some(bx));
    assert_eq!(w.robots[&1].carry, None);
    assert_eq!(
        w.robots[&1].energy,
        e0 - ENERGY_COST_TAKE - ENERGY_COST_GIVE
    );
    check_invariants(&w);
}

#[test]
fn same_ground_box_double_pick_first_id_wins() {
    let mut w = World::new_empty(10, 10, 0);
    let bx = w.add_ground_box("water", Position::new(2, 1));
    let a = w.add_robot(Position::new(1, 1)); // id 小
    let b = w.add_robot(Position::new(3, 1));
    assert_eq!(w.accept_pick(b, 2, 1), codes::OK); // 大 id 先提交
    assert_eq!(w.accept_pick(a, 2, 1), codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::OK);
    assert_eq!(res[&b].code, codes::TARGET_CONTESTED); // 同箱每 tick 至多转移一次
    assert_eq!(w.robots[&a].carry, Some(bx));
    assert_eq!(w.robots[&b].carry, None);
    check_invariants(&w);
}

#[test]
fn carried_box_taken_by_lower_id_robot() {
    // 主动取货可从携带机器人取得（被动交出）；同箱竞争小 id 胜。
    let mut w = World::new_empty(10, 10, 0);
    let carrier = w.add_robot(Position::new(3, 1));
    let low = w.add_robot(Position::new(2, 1));
    let high = w.add_robot(Position::new(4, 1));
    let bx = give_robot_a_box(&mut w, carrier, "water");

    assert_eq!(w.accept_take(high, carrier, bx), codes::OK); // 大 id 先提交
    assert_eq!(w.accept_take(low, carrier, bx), codes::OK);
    let res = w.settle();
    assert_eq!(res[&low].code, codes::OK);
    assert_eq!(res[&high].code, codes::TARGET_CONTESTED);
    assert_eq!(w.robots[&low].carry, Some(bx));
    assert_eq!(w.robots[&carrier].carry, None);
    check_invariants(&w);
}
