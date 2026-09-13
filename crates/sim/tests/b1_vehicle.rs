//! B1 车辆生命周期与市场取消（docs/game-design/04 市场与交易）：
//! 入库车卸空离场、出库车装满离场并收款、cancel 的恢复条件与手续费、
//! 离场 → 订单完成与收款 → 装卸位释放的次序。

mod common;

use common::*;
use ztw_model::{OrderSide, Position, VehicleKind, codes};
use ztw_sim::{CANCEL_FEE_DENOMINATOR, World};

/// 接单并推进到车辆到场，返回 (订单 id, 车辆 id, 装卸口 id)。
fn docked_vehicle(w: &mut World, side: OrderSide, qty: u32, price: i64) -> (u64, u64, u64) {
    let port = w.add_port(Position::new(0, 5));
    let o = w.add_listing(side, "battery", qty, price);
    let (code, _) = w.manage_take(o);
    assert_eq!(code, codes::OK);
    w.end_tick();
    w.boundary_events();
    let vid = w.ports[&port].docked_vehicle.expect("车辆已到场");
    (o, vid, port)
}

#[test]
fn in_vehicle_empties_then_departs() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (o, vid, port) = docked_vehicle(&mut w, OrderSide::Sell, 2, 5000);
    let gold_after_take = w.gold_milli;
    let r = w.add_robot(Position::new(1, 5)); // 与 (0,5) 相邻

    let b1 = w.vehicles[&vid].box_ids[0];
    let b2 = w.vehicles[&vid].box_ids[1];
    assert_eq!(w.accept_take(r, vid, b1), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    // 未卸空：车辆仍在场。
    assert!(w.vehicles.contains_key(&vid));
    assert_eq!(w.my_orders[&o].vehicle, Some(vid));
    check_invariants(&w);

    // 单箱携带：先放到地面暂存格 (1,4)，再回来取第二箱。
    w.end_tick();
    assert_eq!(w.accept_drop(r, 1, 4, None), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    assert_eq!(w.ground_boxes[&b1].pos, Position::new(1, 4));
    assert!(w.vehicles.contains_key(&vid));
    check_invariants(&w);

    w.end_tick();
    assert_eq!(w.accept_take(r, vid, b2), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    // 卸空即离场：同一结算内完成离场、订单完成、装卸位释放。
    assert!(!w.vehicles.contains_key(&vid));
    assert!(!w.my_orders.contains_key(&o));
    assert_eq!(w.ports[&port].docked_vehicle, None);
    assert_eq!(w.gold_milli, gold_after_take); // 卖单（买入）已付，离场无收款
    assert_eq!(w.robots[&r].carry, Some(b2)); // 箱已归玩家
    check_invariants(&w);
}

#[test]
fn out_vehicle_fills_departs_and_pays() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (o, vid, port) = docked_vehicle(&mut w, OrderSide::Buy, 2, 7000);
    let gold_before = w.gold_milli;
    assert_eq!(w.gold_milli, gold_before); // 买单不预付
    let r = w.add_robot(Position::new(1, 5));

    let b1 = w.add_ground_box("battery", Position::new(1, 4));
    let b2 = w.add_ground_box("battery", Position::new(1, 6));
    assert_eq!(w.accept_pick(r, 1, 4), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    w.end_tick();
    assert_eq!(w.accept_give(r, vid, Some(b1)), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    assert!(w.vehicles.contains_key(&vid)); // 装一半不离场
    assert_eq!(w.vehicles[&vid].box_ids, vec![b1]);
    check_invariants(&w);

    w.end_tick();
    assert_eq!(w.accept_pick(r, 1, 6), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    w.end_tick();
    assert_eq!(w.accept_give(r, vid, Some(b2)), codes::OK);
    let res = w.settle();
    assert_eq!(res[&r].code, codes::OK);
    // 装满即离场并收款。
    assert!(!w.vehicles.contains_key(&vid));
    assert!(!w.my_orders.contains_key(&o));
    assert_eq!(w.ports[&port].docked_vehicle, None);
    assert_eq!(w.gold_milli, gold_before + 2 * 7000);
    assert!(!w.ground_boxes.contains_key(&b1)); // 货随车退出世界
    assert!(!w.ground_boxes.contains_key(&b2));
    check_invariants(&w);
}

#[test]
fn vehicle_only_accepts_order_goods_type() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (_o, vid, _port) = docked_vehicle(&mut w, OrderSide::Buy, 2, 7000); // battery
    let r = w.add_robot(Position::new(1, 5));
    let wrong = w.add_ground_box("chip", Position::new(1, 4));
    assert_eq!(w.accept_pick(r, 1, 4), codes::OK);
    w.settle();
    w.end_tick();
    // 类型不符的 give 被拒（docs/game-design/04 货物通用）。
    assert_eq!(w.accept_give(r, vid, Some(wrong)), codes::WRONG_GOODS);
    assert_eq!(w.vehicles[&vid].box_ids.len(), 0);
    check_invariants(&w);
}

#[test]
fn cancel_before_arrival_refunds_minus_fee() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let port = w.add_port(Position::new(0, 5));
    let o = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
    w.manage_take(o);
    let paid = w.gold_milli;
    let fee = 2 * 5000 / CANCEL_FEE_DENOMINATOR;

    let (code, eff) = w.manage_cancel(o);
    assert_eq!(code, codes::OK);
    assert_eq!(eff.unwrap().fee_milli, fee);
    assert_eq!(w.gold_milli, paid + 2 * 5000 - fee);
    assert!(!w.my_orders.contains_key(&o));
    assert_eq!(w.ports[&port].reserved_for, None);
    // 到场事件已撤销。
    w.end_tick();
    w.boundary_events();
    assert!(w.vehicles.is_empty());
}

#[test]
fn cancel_arrived_full_in_vehicle_restores() {
    // 入库车原样未动（车满）→ 可取消；车与货物退回对方，装卸口释放。
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (o, vid, port) = docked_vehicle(&mut w, OrderSide::Sell, 2, 5000);
    let gold_after_take = w.gold_milli;
    let boxes_before = w.ground_boxes.len();

    let (code, _) = w.manage_cancel(o);
    assert_eq!(code, codes::OK);
    assert_eq!(
        w.gold_milli,
        gold_after_take + 2 * 5000 - 2 * 5000 / CANCEL_FEE_DENOMINATOR
    );
    assert!(!w.vehicles.contains_key(&vid));
    assert_eq!(w.ground_boxes.len(), boxes_before - 2); // 车上箱随之移除
    assert_eq!(w.ports[&port].docked_vehicle, None);
    check_invariants(&w);
}

#[test]
fn cancel_after_partial_unload_rejects() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (o, vid, _port) = docked_vehicle(&mut w, OrderSide::Sell, 2, 5000);
    let r = w.add_robot(Position::new(1, 5));
    let b1 = w.vehicles[&vid].box_ids[0];
    assert_eq!(w.accept_take(r, vid, b1), codes::OK);
    w.settle();
    // 车少一箱：未恢复到场初态 → GOODS_MOVED。
    assert_eq!(w.manage_cancel(o).0, codes::GOODS_MOVED);
    assert!(w.my_orders.contains_key(&o));
    assert!(w.vehicles.contains_key(&vid));
}

#[test]
fn cancel_buy_side_fee_can_create_debt() {
    let mut w = World::new_empty(10, 10, 0); // 零金币
    let (o, vid, _port) = docked_vehicle(&mut w, OrderSide::Buy, 2, 7000);
    // 出库车为空（初态）→ 可取消；手续费从金币扣、不足自动计欠款。
    let (code, eff) = w.manage_cancel(o);
    assert_eq!(code, codes::OK);
    let fee = eff.unwrap().fee_milli;
    assert_eq!(fee, 2 * 7000 / CANCEL_FEE_DENOMINATOR);
    assert_eq!(w.gold_milli, 0);
    assert_eq!(w.debt_milli, fee);
    assert!(!w.vehicles.contains_key(&vid));
    check_invariants(&w);
}

#[test]
fn restore_out_vehicle_by_taking_back_then_cancel() {
    // 误装一箱进出库车：取回即恢复初态，可取消（docs/game-design/04）。
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (o, vid, _port) = docked_vehicle(&mut w, OrderSide::Buy, 2, 7000);
    let r = w.add_robot(Position::new(1, 5));
    let b = w.add_ground_box("battery", Position::new(1, 4));
    assert_eq!(w.accept_pick(r, 1, 4), codes::OK);
    w.settle();
    w.end_tick();
    assert_eq!(w.accept_give(r, vid, Some(b)), codes::OK);
    w.settle();
    assert_eq!(w.manage_cancel(o).0, codes::GOODS_MOVED); // 车非空不可取消
    w.end_tick();
    assert_eq!(w.accept_take(r, vid, b), codes::OK); // 从出库车取回
    w.settle();
    let (code, _) = w.manage_cancel(o);
    assert_eq!(code, codes::OK);
    assert_eq!(w.robots[&r].carry, Some(b)); // 箱归玩家保留
    check_invariants(&w);
}

#[test]
fn boxes_on_vehicle_cannot_be_destroyed() {
    // 买入的货物在卸离车辆前不可销毁。
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (_o, vid, _port) = docked_vehicle(&mut w, OrderSide::Sell, 1, 5000);
    let bx = w.vehicles[&vid].box_ids[0];
    assert_eq!(w.manage_destroy(bx).0, codes::ON_VEHICLE);
    // 卸离后即可销毁（止损）。
    let r = w.add_robot(Position::new(1, 5));
    assert_eq!(w.accept_take(r, vid, bx), codes::OK);
    w.settle();
    w.end_tick();
    assert_eq!(w.manage_destroy(bx).0, codes::OK);
    assert!(!w.ground_boxes.contains_key(&bx));
    assert_eq!(w.robots[&r].carry, None);
    check_invariants(&w);
}

#[test]
fn vehicle_cannot_be_destroyed_directly() {
    let mut w = World::new_empty(10, 10, 1_000_000);
    let (_o, vid, _port) = docked_vehicle(&mut w, OrderSide::Sell, 1, 5000);
    assert_eq!(w.manage_destroy(vid).0, codes::INVALID_TARGET); // cancel 是唯一路径
    assert_eq!(w.vehicles[&vid].kind, VehicleKind::In);
}
