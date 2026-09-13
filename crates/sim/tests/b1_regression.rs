//! B1 统一回归（docs/architecture/08 原型 B 模拟回归清单）：
//! move/drop 落点交互、被动转交与主动取放、销毁与取消后的已受理动作、
//! 以及固定管理操作后的全排列提交顺序确定性。

mod common;

use common::*;
use ztw_model::{OrderSide, Position, codes};
use ztw_sim::World;

// -----------------------------------------------------------------------
// 清单 3：move 与 drop 争同格；drop 到成功 / 未成功离开的机器人格
// -----------------------------------------------------------------------

#[test]
fn move_drop_contest_both_ways() {
    // 落点 (3,4)：drop 一方 id 更小 → drop 胜、move CELL_CONTESTED。
    {
        let mut w = World::new_empty(10, 10, 0);
        let d = w.add_robot(Position::new(3, 5)); // id 1：携带箱，向上放 (3,4)
        let bx = give_robot_a_box(&mut w, d, "water");
        let m = w.add_robot(Position::new(2, 4)); // id 2：向东移 (3,4)
        assert_eq!(w.accept_move(m, 1, 0), codes::OK);
        assert_eq!(w.accept_drop(d, 3, 4, None), codes::OK);
        let res = w.settle();
        assert_eq!(res[&d].code, codes::OK);
        assert_eq!(res[&m].code, codes::CELL_CONTESTED);
        assert_eq!(w.ground_boxes[&bx].pos, Position::new(3, 4));
        assert_eq!(w.ground_boxes[&bx].holder, None);
        assert_eq!(w.robots[&m].pos, Position::new(2, 4));
        check_invariants(&w);
    }
    // 反转 id：move 一方 id 更小 → move 胜、drop CELL_CONTESTED。
    {
        let mut w = World::new_empty(10, 10, 0);
        let m = w.add_robot(Position::new(2, 4)); // id 1
        let d = w.add_robot(Position::new(3, 5)); // id 2
        give_robot_a_box(&mut w, d, "water");
        assert_eq!(w.accept_move(m, 1, 0), codes::OK);
        assert_eq!(w.accept_drop(d, 3, 4, None), codes::OK);
        let res = w.settle();
        assert_eq!(res[&m].code, codes::OK);
        assert_eq!(res[&d].code, codes::CELL_CONTESTED);
        assert_eq!(w.robots[&m].pos, Position::new(3, 4));
        assert!(w.robots[&d].carry.is_some()); // 货仍在手
        check_invariants(&w);
    }
}

#[test]
fn drop_into_cell_of_departing_and_staying_robot() {
    // 占位机器人成功离开 → drop 可放。
    {
        let mut w = World::new_empty(10, 10, 0);
        let occupant = w.add_robot(Position::new(3, 4));
        let d = w.add_robot(Position::new(3, 5));
        give_robot_a_box(&mut w, d, "water");
        assert_eq!(w.accept_move(occupant, 1, 0), codes::OK); // 离开 (3,4)
        assert_eq!(w.accept_drop(d, 3, 4, None), codes::OK);
        let res = w.settle();
        assert_eq!(res[&d].code, codes::OK);
        assert_eq!(w.robots[&occupant].pos, Position::new(4, 4));
        check_invariants(&w);
    }
    // 占位机器人未离开（受阻）→ CELL_OCCUPIED，货仍在手。
    {
        let mut w = World::new_empty(10, 10, 0);
        let blocker = w.add_robot(Position::new(4, 4)); // 静止堵路
        let occupant = w.add_robot(Position::new(3, 4));
        let d = w.add_robot(Position::new(3, 5));
        let bx = give_robot_a_box(&mut w, d, "water");
        assert_eq!(w.accept_move(occupant, 1, 0), codes::OK); // 被 blocker 挡
        assert_eq!(w.accept_drop(d, 3, 4, None), codes::OK);
        let res = w.settle();
        assert_eq!(res[&occupant].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&d].code, codes::CELL_OCCUPIED);
        assert_eq!(w.robots[&d].carry, Some(bx));
        assert_eq!(w.robots[&blocker].pos, Position::new(4, 4));
        check_invariants(&w);
    }
}

// -----------------------------------------------------------------------
// 清单 4：被动转交与主动取放争同箱或容量；目标移动成败两分支
// -----------------------------------------------------------------------

#[test]
fn passive_receive_vs_active_take_both_id_orders() {
    // 低 id 机器人先 give：R 被动收到箱，R 自己的 take 失败 TARGET_CONTESTED。
    {
        let mut w = World::new_empty(10, 10, 0);
        let giver = w.add_robot(Position::new(1, 1)); // id 小
        let taker = w.add_robot(Position::new(2, 1));
        let shelf = w.add_shelf(Position::new(3, 1));
        let sb = put_box_on_shelf(&mut w, shelf, "water");
        let hand = give_robot_a_box(&mut w, giver, "water");

        assert_eq!(w.accept_take(taker, shelf, sb), codes::OK);
        assert_eq!(w.accept_give(giver, taker, Some(hand)), codes::OK);
        let res = w.settle();
        assert_eq!(res[&giver].code, codes::OK);
        assert_eq!(res[&taker].code, codes::TARGET_CONTESTED); // 被动收取占了容量
        assert_eq!(w.robots[&taker].carry, Some(hand));
        assert_eq!(w.shelves[&shelf].box_ids, vec![sb]); // 货架原样
        check_invariants(&w);
    }
    // 反转 id：taker 先取 → 成功；giver 的 give 落空 TARGET_CONTESTED。
    {
        let mut w = World::new_empty(10, 10, 0);
        let taker = w.add_robot(Position::new(2, 1)); // id 小
        let giver = w.add_robot(Position::new(1, 1));
        let shelf = w.add_shelf(Position::new(3, 1));
        let sb = put_box_on_shelf(&mut w, shelf, "water");
        let hand = give_robot_a_box(&mut w, giver, "water");

        assert_eq!(w.accept_give(giver, taker, Some(hand)), codes::OK);
        assert_eq!(w.accept_take(taker, shelf, sb), codes::OK);
        let res = w.settle();
        assert_eq!(res[&taker].code, codes::OK);
        assert_eq!(res[&giver].code, codes::TARGET_CONTESTED);
        assert_eq!(w.robots[&taker].carry, Some(sb));
        assert_eq!(w.robots[&giver].carry, Some(hand)); // 货退回原处
        check_invariants(&w);
    }
}

#[test]
fn transfer_target_moved_both_branches() {
    // 目标机器人同 tick 成功移动 → 转交失败 TARGET_MOVED。
    {
        let mut w = World::new_empty(10, 10, 0);
        let mover = w.add_robot(Position::new(2, 1)); // 携带箱，将移走
        let bx = give_robot_a_box(&mut w, mover, "water");
        let taker = w.add_robot(Position::new(1, 1));
        assert_eq!(w.accept_move(mover, 0, 1), codes::OK); // → (2,2)
        assert_eq!(w.accept_take(taker, mover, bx), codes::OK);
        let res = w.settle();
        assert_eq!(res[&mover].code, codes::OK);
        assert_eq!(res[&taker].code, codes::TARGET_MOVED);
        assert_eq!(w.robots[&mover].carry, Some(bx)); // 货随目标移动
        check_invariants(&w);
    }
    // 目标移动失败 → 按原位置判定，转交成功。
    {
        let mut w = World::new_empty(10, 10, 0);
        let blocker = w.add_robot(Position::new(2, 2)); // 静止堵路
        let mover = w.add_robot(Position::new(2, 1));
        let bx = give_robot_a_box(&mut w, mover, "water");
        let taker = w.add_robot(Position::new(1, 1));
        assert_eq!(w.accept_move(mover, 0, 1), codes::OK); // 被 blocker 挡
        assert_eq!(w.accept_take(taker, mover, bx), codes::OK);
        let res = w.settle();
        assert_eq!(res[&mover].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&taker].code, codes::OK);
        assert_eq!(w.robots[&taker].carry, Some(bx));
        let _ = blocker;
        check_invariants(&w);
    }
}

// -----------------------------------------------------------------------
// 清单 5：销毁主体 / 目标、取消车辆后已受理动作失败；不扣电、不复制遗失
// -----------------------------------------------------------------------

#[test]
fn destroy_acting_robot_after_accept() {
    let mut w = World::new_empty(10, 10, 0);
    let r = w.add_robot(Position::new(2, 1));
    let shelf = w.add_shelf(Position::new(3, 1));
    let sb = put_box_on_shelf(&mut w, shelf, "water");
    assert_eq!(w.accept_take(r, shelf, sb), codes::OK);
    // 受理后销毁主体（空载可毁）。
    assert_eq!(w.manage_destroy(r).0, codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::TARGET_GONE); // 结果保留在诊断记录
    assert_eq!(w.shelves[&shelf].box_ids, vec![sb]); // 货物守恒
    check_invariants(&w);
}

#[test]
fn destroy_transfer_target_after_accept() {
    let mut w = World::new_empty(10, 10, 0);
    let a = w.add_robot(Position::new(1, 1));
    let b = w.add_robot(Position::new(2, 1)); // give 目标
    let bx = give_robot_a_box(&mut w, a, "water");
    let e0 = w.robots[&a].energy;
    assert_eq!(w.accept_give(a, b, None), codes::OK);
    assert_eq!(w.manage_destroy(b).0, codes::OK);
    let res = w.settle();
    assert_eq!(res[&a].code, codes::TARGET_GONE);
    assert_eq!(w.robots[&a].carry, Some(bx)); // 货退回，不遗失
    assert_eq!(w.robots[&a].energy, e0); // 失败不扣电
    check_invariants(&w);
}

#[test]
fn cancel_vehicle_with_pending_take() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let dock = w.add_dock(Position::new(0, 4), (0, 1));
    let o = w.add_listing(OrderSide::Sell, "battery", 1, 5000);
    w.manage_take(o);
    w.end_tick();
    w.boundary_events();
    let vid = w.docks[&dock].docked_vehicle.unwrap();
    let r = w.add_robot(Position::new(1, 5));
    let bx = w.vehicles[&vid].box_ids[0];
    let e0 = w.robots[&r].energy;
    assert_eq!(w.accept_take(r, vid, bx), codes::OK);
    // 车满（初态）→ 可取消；已受理 take 在结算中失败。
    assert_eq!(w.manage_cancel(o).0, codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::TARGET_GONE);
    assert_eq!(w.robots[&r].carry, None);
    assert_eq!(w.robots[&r].energy, e0); // 失败不扣电
    assert!(!w.ground_boxes.contains_key(&bx)); // 箱随车退回对方，无复制
    check_invariants(&w);
}

// -----------------------------------------------------------------------
// 清单 8：固定管理操作后排列提交顺序（≤5 台穷举），受理集合、结算结果、
// 世界哈希三者一致；场景含混合动作（move/take/give/pick/charge）。
// -----------------------------------------------------------------------

#[test]
fn permutation_invariant_mixed_actions() {
    let build = |w: &mut World| -> [u64; 5] {
        let shelf = w.add_shelf(Position::new(2, 0));
        let sb = put_box_on_shelf(w, shelf, "battery");
        w.add_ground_box("water", Position::new(4, 0));
        w.add_charger(Position::new(6, 0));
        // 固定管理操作：放一台多余机器人后销毁（影响 id 序与世界状态）。
        let extra = w.add_robot(Position::new(9, 9));
        assert_eq!(w.manage_destroy(extra).0, codes::OK);
        let ids = [
            w.add_robot(Position::new(2, 1)), // A：take(shelf, sb)
            w.add_robot(Position::new(3, 1)), // B：give(A)（被动接收争容量）
            w.add_robot(Position::new(4, 1)), // C：pick(4,0)
            w.add_robot(Position::new(5, 0)), // D：charge（桩在 (6,0)）
            w.add_robot(Position::new(1, 3)), // E：move EAST
        ];
        let _ = sb;
        ids
    };
    // 动作只依赖机器人 id（解析目标已在 build 中固定），与提交顺序无关。
    let submit = |w: &mut World, ids: &[u64; 5], i: usize| -> &'static str {
        let shelf_id = w
            .shelves
            .values()
            .find(|s| s.pos == Position::new(2, 0))
            .unwrap()
            .id;
        let sb = w.shelves[&shelf_id].box_ids[0];
        match i {
            0 => w.accept_take(ids[0], shelf_id, sb),
            1 => w.accept_give(ids[1], ids[0], None), // B 携带箱由 build 放置
            2 => w.accept_pick(ids[2], 4, 0),
            3 => w.accept_charge(ids[3]),
            _ => w.accept_move(ids[4], 1, 0),
        }
    };
    let finish = |w: &mut World, ids: &[u64; 5]| -> (Vec<String>, Vec<(u64, String)>, u64) {
        let mut codes_out = Vec::new();
        for i in 0..5 {
            codes_out.push(submit(w, ids, i).to_string());
        }
        let res = w.settle();
        check_invariants(w);
        let results: Vec<(u64, String)> = res.iter().map(|(id, r)| (*id, r.code.clone())).collect();
        (codes_out, results, w.state_hash())
    };
    let baseline = {
        let mut w = World::new_empty(12, 12, 500_000);
        let ids = build(&mut w);
        // B 的携带箱在 build 后补（不占 id 比较位次）。
        give_robot_a_box(&mut w, ids[1], "water");
        let out = finish(&mut w, &ids);
        (out, ids)
    };
    let mut perm = [0usize, 1, 2, 3, 4];
    loop {
        let mut w = World::new_empty(12, 12, 500_000);
        let ids = build(&mut w);
        give_robot_a_box(&mut w, ids[1], "water");
        let mut codes_p = vec![String::new(); 5];
        for &i in &perm {
            codes_p[i] = submit(&mut w, &ids, i).to_string();
        }
        let res = w.settle();
        check_invariants(&w);
        let results: Vec<(u64, String)> = res.iter().map(|(id, r)| (*id, r.code.clone())).collect();
        assert_eq!(codes_p, baseline.0.0, "受理集合不一致：perm {perm:?}");
        assert_eq!(results, baseline.0.1, "结算结果不一致：perm {perm:?}");
        assert_eq!(
            w.state_hash(),
            baseline.0.2,
            "世界哈希不一致：perm {perm:?}"
        );
        if !next_perm(&mut perm) {
            break;
        }
    }
    // 语义抽查（排列无关的确定结局）：A 的 id 小于 B，A 先取得货架箱，
    // B 的被动转交因 A 已满载而落空 TARGET_CONTESTED。
    let ((codes0, results, _hash), ids) = &baseline;
    assert!(codes0.iter().all(|c| c == codes::OK), "受理全通过");
    let code_of = |rid: u64| {
        results
            .iter()
            .find(|(id, _)| *id == rid)
            .map(|(_, c)| c.clone())
    };
    assert_eq!(code_of(ids[0]).as_deref(), Some(codes::OK)); // A take 成功
    assert_eq!(code_of(ids[1]).as_deref(), Some(codes::TARGET_CONTESTED)); // B give 落空
    assert_eq!(code_of(ids[2]).as_deref(), Some(codes::OK)); // C pick 成功
    assert_eq!(code_of(ids[3]).as_deref(), Some(codes::OK)); // D charge 成功
    assert_eq!(code_of(ids[4]).as_deref(), Some(codes::OK)); // E move 成功
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
