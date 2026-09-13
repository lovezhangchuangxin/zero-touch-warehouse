//! B1 电力与充电语义（docs/game-design/03 电力与充电）：
//! 每 tick 申请制、相邻 id 最小桩且不换桩、同 tick 竞争 id 裁决、
//! 充电与转交不冲突、不预支同 tick 充入电量。

mod common;

use common::*;
use ztw_model::{Position, codes};
use ztw_sim::{CHARGE_PER_TICK, ENERGY_COST_GIVE, World};

#[test]
fn charge_selects_min_id_charger_and_never_switches() {
    let mut w = demo_world(); // 机器人 1 在 (1,1)
    let c_low = w.add_charger(Position::new(1, 2)); // id 小
    let _c_high = w.add_charger(Position::new(2, 1));
    set_energy(&mut w, 1, 10);

    assert_eq!(w.accept_charge(1), codes::OK);
    // 受理后销毁 id 最小的桩：不自动换桩，结算失败 TARGET_GONE。
    assert_eq!(w.manage_destroy(c_low).0, codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::TARGET_GONE);
    assert_eq!(w.robots[&1].energy, 10); // 失败不充电
    check_invariants(&w);

    // 下一 tick 重新申请：改选剩余的相邻桩（id 次小）。
    w.end_tick();
    assert_eq!(w.accept_charge(1), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(w.robots[&1].energy, 10 + CHARGE_PER_TICK);
    assert_eq!(w.last_results[&1].arg, _c_high.to_string()); // 目标即次小桩
}

#[test]
fn charge_contention_three_robots_one_charger() {
    let mut w = World::new_empty(10, 10, 0);
    let c = w.add_charger(Position::new(5, 5));
    let a = w.add_robot(Position::new(5, 4)); // id 最小
    let b = w.add_robot(Position::new(5, 6));
    let d = w.add_robot(Position::new(4, 5));
    set_energy(&mut w, a, 1);
    set_energy(&mut w, b, 2);
    set_energy(&mut w, d, 3);
    // 逆序提交：结果仍按机器人 id 裁决。
    assert_eq!(w.accept_charge(d), codes::OK);
    assert_eq!(w.accept_charge(b), codes::OK);
    assert_eq!(w.accept_charge(a), codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::OK);
    assert_eq!(res[&b].code, codes::CHARGER_BUSY);
    assert_eq!(res[&d].code, codes::CHARGER_BUSY);
    assert_eq!(w.robots[&a].energy, 1 + CHARGE_PER_TICK);
    assert_eq!(w.robots[&b].energy, 2);
    assert_eq!(w.robots[&d].energy, 3);
    check_invariants(&w);

    // 每 tick 重新申请：下一 tick 换 b（此时 id 次小）成功。
    w.end_tick();
    assert_eq!(w.accept_charge(b), codes::OK);
    let res = w.settle();
    assert_eq!(res[&b].code, codes::OK);
    assert_eq!(w.robots[&b].energy, 2 + CHARGE_PER_TICK);
    let _ = c;
}

#[test]
fn charge_caps_at_energy_max() {
    let mut w = demo_world();
    w.add_charger(Position::new(1, 2));
    set_energy(&mut w, 1, 90);
    assert_eq!(w.accept_charge(1), codes::OK);
    let res = w.settle();
    assert_eq!(res[&1].code, codes::OK);
    assert_eq!(w.robots[&1].energy, 100); // 封顶，不溢出
}

#[test]
fn passive_receive_does_not_conflict_with_charging() {
    // 被动方充电与转交不冲突：R 在充电，S 同时把箱转交给 R。
    let mut w = World::new_empty(10, 10, 0);
    let r = w.add_robot(Position::new(5, 5));
    w.add_charger(Position::new(5, 6)); // 与 R 相邻
    let s = w.add_robot(Position::new(4, 5)); // 与 R 相邻
    let bx = give_robot_a_box(&mut w, s, "water");
    set_energy(&mut w, r, 10);
    let s_energy = w.robots[&s].energy;

    assert_eq!(w.accept_charge(r), codes::OK);
    assert_eq!(w.accept_give(s, r, None), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    assert_eq!(res[&s].code, codes::OK);
    assert_eq!(w.robots[&r].energy, 10 + CHARGE_PER_TICK); // 充电生效
    assert_eq!(w.robots[&r].carry, Some(bx)); // 且同时收到货
    assert_eq!(w.robots[&s].energy, s_energy - ENERGY_COST_GIVE); // 耗电只算主动方
    check_invariants(&w);
}

#[test]
fn transfer_energy_validated_at_settlement_snapshot() {
    // 电量恰好等于耗电：成功并归零；不预支同 tick 充入电量（本机器人
    // 不可能同时充电与转交，快照值即结算值）。
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1));
    let b = w.add_robot(Position::new(2, 1));
    give_robot_a_box(&mut w, a, "water");
    set_energy(&mut w, a, ENERGY_COST_GIVE);
    assert_eq!(w.accept_give(a, b, None), codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::OK);
    assert_eq!(w.robots[&a].energy, 0);
    check_invariants(&w);

    // 电量 0 的机器人移动不受限（移动不耗电），可走到桩旁再充。
    w.end_tick();
    assert_eq!(w.accept_move(a, 0, 1), codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::OK);
}

#[test]
fn accept_charge_requires_adjacent_charger() {
    let mut w = demo_world();
    w.add_charger(Position::new(8, 8));
    assert_eq!(w.accept_charge(1), codes::NOT_ADJACENT);
    assert_eq!(w.accept_charge(99), codes::NO_SUCH_OBJECT);
}
