//! 宿主本地查询镜像（docs/architecture/03「宿主本地查询镜像」）。
//!
//! tick 阶段 2 由世界线程产出（世界查询视图、tick、世界修订号）；初始化
//! 执行前同样产出。镜像内容是世界状态的纯函数。管理操作（A0 仅
//! market.take）的响应携带镜像同步增量，宿主按同一 FIFO 回放；回放失败
//! 即整体失效重建（mirror.fetch 回退）。
//!
//! 经济值以 i64 千分金币为权威；镜像中以十进制字符串传输，避免 JSON
//! 浮点精度问题，宿主侧再转显示值。

use serde::Serialize;
use ztw_model::{Id, MilliGold, Order, OrderSide, Position};
use ztw_sim::{
    BorrowEffect, BoughtKind, BuyEffect, CancelEffect, DestroyEffect, DestroyedKind, RepayEffect,
    TakeEffect, World,
};

fn money(v: MilliGold) -> String {
    v.to_string()
}

#[derive(Serialize, Debug, Clone)]
pub struct MirrorView {
    pub tick: u64,
    pub revision: u64,
    pub gold_milli: String,
    pub debt_milli: String,
    pub map_w: i32,
    pub map_h: i32,
    /// 静态障碍格（墙 / 货架 / 充电桩 / 装卸位 / 地面货物）。
    pub blocked: Vec<(i32, i32)>,
    pub robots: Vec<RobotView>,
    pub shelves: Vec<ShelfView>,
    pub chargers: Vec<ChargerView>,
    pub docks: Vec<DockView>,
    pub vehicles: Vec<VehicleView>,
    pub ground_boxes: Vec<BoxView>,
    pub sell_orders: Vec<OrderView>,
    pub buy_orders: Vec<OrderView>,
    pub my_orders: Vec<OrderView>,
}

#[derive(Serialize, Debug, Clone)]
pub struct RobotView {
    pub id: Id,
    pub x: i32,
    pub y: i32,
    pub energy: u32,
    pub energy_max: u32,
    pub carry: Option<BoxView>,
    pub last_result: Option<LastResultView>,
}

#[derive(Serialize, Debug, Clone)]
pub struct LastResultView {
    pub action: String,
    pub arg: String,
    pub code: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct ShelfView {
    pub id: Id,
    pub x: i32,
    pub y: i32,
    pub boxes: Vec<BoxView>,
    pub capacity: usize,
}

#[derive(Serialize, Debug, Clone)]
pub struct ChargerView {
    pub id: Id,
    pub x: i32,
    pub y: i32,
}

/// 装卸位视图：`x`/`y` 为靠墙缺口锚点格，`ext` 指向库内第二格。
#[derive(Serialize, Debug, Clone)]
pub struct DockView {
    pub id: Id,
    pub x: i32,
    pub y: i32,
    pub ext: (i32, i32),
    pub docked_vehicle: Option<Id>,
}

#[derive(Serialize, Debug, Clone)]
pub struct VehicleView {
    pub id: Id,
    pub kind: String,
    pub goods_type: String,
    pub x: i32,
    pub y: i32,
    pub order_id: Id,
    /// 所停靠装卸位 id。
    pub dock: Id,
    pub boxes: Vec<BoxView>,
}

#[derive(Serialize, Debug, Clone)]
pub struct BoxView {
    pub id: Id,
    pub goods_type: String,
    pub holder: Option<Id>,
    pub x: i32,
    pub y: i32,
}

#[derive(Serialize, Debug, Clone)]
pub struct OrderView {
    pub id: Id,
    pub side: String,
    pub goods_type: String,
    pub qty: u32,
    pub unit_price_milli: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dock: Option<Id>,
}

fn order_view(o: &Order) -> OrderView {
    OrderView {
        id: o.id,
        side: o.side.as_str().to_string(),
        goods_type: o.goods_type.clone(),
        qty: o.qty,
        unit_price_milli: money(o.unit_price_milli),
        vehicle: o.vehicle,
        dock: o.dock,
    }
}

fn ground_box_view(b: &ztw_model::GroundBox) -> BoxView {
    BoxView {
        id: b.id,
        goods_type: b.goods_type.clone(),
        holder: None,
        x: b.pos.x,
        y: b.pos.y,
    }
}

/// 实体 → 视图（from_world 全量镜像与 from_buy 增量共用，保证同源）。
fn robot_view_of(world: &World, r: &ztw_model::Robot) -> RobotView {
    RobotView {
        id: r.id,
        x: r.pos.x,
        y: r.pos.y,
        energy: r.energy,
        energy_max: r.energy_max,
        carry: r.carry.map(|id| ground_box_view(&world.ground_boxes[&id])),
        last_result: world.last_results.get(&r.id).map(|lr| LastResultView {
            action: lr.action.to_string(),
            arg: lr.arg.clone(),
            code: lr.code.clone(),
        }),
    }
}

fn shelf_view_of(world: &World, s: &ztw_model::Shelf) -> ShelfView {
    ShelfView {
        id: s.id,
        x: s.pos.x,
        y: s.pos.y,
        boxes: s
            .box_ids
            .iter()
            .map(|id| ground_box_view(&world.ground_boxes[id]))
            .collect(),
        capacity: s.capacity,
    }
}

fn charger_view_of(c: &ztw_model::Charger) -> ChargerView {
    ChargerView {
        id: c.id,
        x: c.pos.x,
        y: c.pos.y,
    }
}

fn dock_view_of(d: &ztw_model::Dock) -> DockView {
    DockView {
        id: d.id,
        x: d.pos.x,
        y: d.pos.y,
        ext: d.ext,
        docked_vehicle: d.docked_vehicle,
    }
}

impl MirrorView {
    pub fn from_world(world: &World, revision: u64) -> MirrorView {
        let mut blocked = Vec::new();
        for x in 0..world.map_w {
            for y in 0..world.map_h {
                let p = Position::new(x, y);
                if !world.statically_passable(p) {
                    blocked.push((x, y));
                }
            }
        }
        let box_view = |id: Id| -> BoxView {
            let b = world.ground_boxes.get(&id).expect("货物存在");
            ground_box_view(b)
        };
        MirrorView {
            tick: world.tick,
            revision,
            gold_milli: money(world.gold_milli),
            debt_milli: money(world.debt_milli),
            map_w: world.map_w,
            map_h: world.map_h,
            blocked,
            robots: world
                .robots
                .values()
                .map(|r| robot_view_of(world, r))
                .collect(),
            shelves: world
                .shelves
                .values()
                .map(|s| shelf_view_of(world, s))
                .collect(),
            chargers: world.chargers.values().map(charger_view_of).collect(),
            docks: world.docks.values().map(dock_view_of).collect(),
            vehicles: world
                .vehicles
                .values()
                .map(|v| {
                    let dock = world
                        .my_orders
                        .get(&v.order_id)
                        .and_then(|o| o.dock)
                        .expect("在场车辆订单必有预留装卸位");
                    VehicleView {
                        id: v.id,
                        kind: v.kind.as_str().to_string(),
                        goods_type: v.goods_type.clone(),
                        x: v.interact_pos.x,
                        y: v.interact_pos.y,
                        order_id: v.order_id,
                        dock,
                        boxes: v.box_ids.iter().map(|id| box_view(*id)).collect(),
                    }
                })
                .collect(),
            ground_boxes: world
                .ground_boxes
                .values()
                .filter(|b| b.holder.is_none())
                .map(ground_box_view)
                .collect(),
            sell_orders: world
                .listings
                .values()
                .filter(|o| o.side == OrderSide::Sell)
                .map(order_view)
                .collect(),
            buy_orders: world
                .listings
                .values()
                .filter(|o| o.side == OrderSide::Buy)
                .map(order_view)
                .collect(),
            my_orders: world.my_orders.values().map(order_view).collect(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("镜像序列化不失败")
    }
}

/// 管理操作的镜像同步增量：宿主按同一 FIFO 回放到本地镜像，字段与宿主
/// 回放逻辑一一对应（docs/architecture/03）。装卸位预留不进增量：
/// DockView 不含 reserved_for（docs/game-design/08 字段表），预留期仅一个
/// tick，下一 tick 全量镜像即一致；接单权威以 take 结果码为准。
#[derive(Serialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MirrorDelta {
    Take(TakeDelta),
    Cancel(CancelDelta),
    Destroy(DestroyDelta),
    /// 借款 / 还款共用：只动金币与欠款两根线。
    Funds(FundsDelta),
    Buy(BuyDelta),
}

#[derive(Serialize, Debug, Clone)]
pub struct TakeDelta {
    pub gold_milli: String,
    pub remove_listing: Id,
    pub add_my_order: OrderView,
}

#[derive(Serialize, Debug, Clone)]
pub struct CancelDelta {
    pub gold_milli: String,
    pub debt_milli: String,
    pub remove_my_order: Id,
    pub remove_vehicle: Option<Id>,
    pub update_dock: Option<DockPatch>,
}

#[derive(Serialize, Debug, Clone)]
pub struct DockPatch {
    pub id: Id,
    pub docked_vehicle: Option<Id>,
}

#[derive(Serialize, Debug, Clone)]
pub struct DestroyDelta {
    /// 对象类别：robot / shelf / charger / dock / box。
    pub object: String,
    pub id: Id,
    pub gold_milli: String,
    /// 释放的静态障碍格（blocked 数组补丁，保同 tick 可见）。
    pub unblock: Vec<(i32, i32)>,
    /// 嵌套视图受影响（销毁被携带 / 在架货物）→ 增量不猜测，宿主整体重建。
    pub rebuild: bool,
}

/// borrow / repay 的资金增量。
#[derive(Serialize, Debug, Clone)]
pub struct FundsDelta {
    pub gold_milli: String,
    pub debt_milli: String,
}

/// buy 的增量：object 指明入列目标，同名可选字段携带新对象全量视图
/// （回放端原样 push，不猜测字段；构造侧与全量镜像同源，无模板漂移）。
#[derive(Serialize, Debug, Clone)]
pub struct BuyDelta {
    /// 对象类别：robot / shelf / charger / dock。
    pub object: String,
    pub gold_milli: String,
    /// 新增静态障碍格（blocked 数组补丁；机器人无）。
    pub block: Vec<(i32, i32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub robot: Option<RobotView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shelf: Option<ShelfView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charger: Option<ChargerView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dock: Option<DockView>,
}

impl MirrorDelta {
    pub fn from_take(world_after: &World, eff: &TakeEffect) -> MirrorDelta {
        MirrorDelta::Take(TakeDelta {
            gold_milli: money(world_after.gold_milli),
            remove_listing: eff.order_id,
            add_my_order: order_view(&eff.order),
        })
    }

    pub fn from_cancel(world_after: &World, eff: &CancelEffect) -> MirrorDelta {
        MirrorDelta::Cancel(CancelDelta {
            gold_milli: money(world_after.gold_milli),
            debt_milli: money(world_after.debt_milli),
            remove_my_order: eff.order_id,
            remove_vehicle: eff.vehicle_id,
            update_dock: eff.dock_id.map(|id| DockPatch {
                id,
                docked_vehicle: world_after.docks.get(&id).and_then(|d| d.docked_vehicle),
            }),
        })
    }

    pub fn from_destroy(world_after: &World, eff: &DestroyEffect) -> MirrorDelta {
        let object = match eff.kind {
            DestroyedKind::Robot => "robot",
            DestroyedKind::Shelf => "shelf",
            DestroyedKind::Charger => "charger",
            DestroyedKind::Dock => "dock",
            DestroyedKind::GroundBox => "box",
        };
        MirrorDelta::Destroy(DestroyDelta {
            object: object.to_string(),
            id: eff.target_id,
            gold_milli: money(world_after.gold_milli),
            unblock: eff.freed_cells.iter().map(|p| (p.x, p.y)).collect(),
            // 无地面格的货物 = 被携带 / 在架，影响嵌套视图（robot.carry /
            // shelf.boxes），增量无法补丁，置 rebuild 走整体重建。
            rebuild: matches!(eff.kind, DestroyedKind::GroundBox) && eff.freed_cells.is_empty(),
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("增量序列化不失败")
    }

    pub fn from_borrow(world_after: &World, _eff: &BorrowEffect) -> MirrorDelta {
        MirrorDelta::Funds(FundsDelta {
            gold_milli: money(world_after.gold_milli),
            debt_milli: money(world_after.debt_milli),
        })
    }

    pub fn from_repay(world_after: &World, _eff: &RepayEffect) -> MirrorDelta {
        MirrorDelta::Funds(FundsDelta {
            gold_milli: money(world_after.gold_milli),
            debt_milli: money(world_after.debt_milli),
        })
    }

    pub fn from_buy(world_after: &World, eff: &BuyEffect) -> MirrorDelta {
        let object = match eff.kind {
            BoughtKind::Robot => "robot",
            BoughtKind::Shelf => "shelf",
            BoughtKind::Charger => "charger",
            BoughtKind::Dock => "dock",
        }
        .to_string();
        // 构造后的世界直接取全量视图：与下一次全量镜像同源，回放端原样
        // 入列即一致。
        let (robot, shelf, charger, dock) = match eff.kind {
            BoughtKind::Robot => (
                Some(robot_view_of(world_after, &world_after.robots[&eff.id])),
                None,
                None,
                None,
            ),
            BoughtKind::Shelf => (
                None,
                Some(shelf_view_of(world_after, &world_after.shelves[&eff.id])),
                None,
                None,
            ),
            BoughtKind::Charger => (
                None,
                None,
                Some(charger_view_of(&world_after.chargers[&eff.id])),
                None,
            ),
            BoughtKind::Dock => (
                None,
                None,
                None,
                Some(dock_view_of(&world_after.docks[&eff.id])),
            ),
        };
        MirrorDelta::Buy(BuyDelta {
            object,
            gold_milli: money(world_after.gold_milli),
            block: eff.blocked_cells.iter().map(|p| (p.x, p.y)).collect(),
            robot,
            shelf,
            charger,
            dock,
        })
    }
}

/// 世界修订号：以 tick 与管理操作计数合成（镜像内容纯函数仍成立）。
#[derive(Debug, Clone, Default)]
pub struct WorldRevision {
    pub tick: u64,
    pub mgmt_count: u64,
}

impl WorldRevision {
    pub fn get(&self) -> u64 {
        self.tick.wrapping_mul(1_000_003) ^ self.mgmt_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztw_model::{OrderSide, Position};

    #[test]
    fn mirror_shape() {
        let mut w = World::new_empty(6, 5, 123_000);
        w.add_robot(Position::new(1, 1));
        w.add_shelf(Position::new(3, 3));
        w.add_dock(Position::new(0, 4), (1, 0));
        w.add_listing(OrderSide::Sell, "battery", 2, 5_000);
        let m = MirrorView::from_world(&w, 1);
        let json = m.to_json();
        assert!(json.contains("\"gold_milli\":\"123000\""));
        assert!(json.contains("\"unit_price_milli\":\"5000\""));
        assert_eq!(m.robots.len(), 1);
        assert_eq!(m.sell_orders.len(), 1);
        assert_eq!(m.blocked.len(), 3); // 货架 + 装卸位两格
    }

    /// 增量 JSON 形状契约：宿主 applyDelta 逐字段回放（bootstrap.js），
    /// 序列化属性与字段名的任何改动都必须同步此处。
    #[test]
    fn delta_shapes() {
        // take：标签 + 挂单移除 + 已接订单回填。
        let mut w = World::new_empty(6, 5, 123_000);
        w.add_dock(Position::new(0, 4), (1, 0));
        let o = w.add_listing(OrderSide::Sell, "battery", 2, 5_000);
        let (code, eff) = w.manage_take(o);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_take(&w, &eff.unwrap())).unwrap();
        assert_eq!(j["kind"], "take");
        assert_eq!(j["gold_milli"], "113000"); // 字符串防浮点
        assert_eq!(j["remove_listing"], o);
        assert_eq!(j["add_my_order"]["id"], o);
        assert_eq!(j["add_my_order"]["dock"], *w.docks.keys().next().unwrap());

        // cancel（未到场）：remove_vehicle / update_dock 为 null。
        let (code, eff) = w.manage_cancel(o);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_cancel(&w, &eff.unwrap())).unwrap();
        assert_eq!(j["kind"], "cancel");
        assert_eq!(j["gold_milli"], "122000"); // 123 - 10 + 退 9（手续费 1）
        assert_eq!(j["debt_milli"], "0");
        assert_eq!(j["remove_my_order"], o);
        assert!(j["remove_vehicle"].is_null());
        // 未到场取消：车辆为 null，但预留位的补丁对象仍在（清 docked 语义）。
        assert_eq!(j["update_dock"]["id"], *w.docks.keys().next().unwrap());
        assert!(j["update_dock"]["docked_vehicle"].is_null());

        // cancel（到场后）：车辆移除与装卸位清空补丁。
        let mut w2 = World::new_empty(6, 5, 123_000);
        let dock = w2.add_dock(Position::new(0, 4), (1, 0));
        let o2 = w2.add_listing(OrderSide::Sell, "battery", 1, 5_000);
        w2.manage_take(o2);
        w2.end_tick();
        w2.boundary_events();
        let vid = w2.docks[&dock].docked_vehicle.unwrap();
        let (code, eff) = w2.manage_cancel(o2);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_cancel(&w2, &eff.unwrap())).unwrap();
        assert_eq!(j["remove_vehicle"], vid);
        assert_eq!(j["update_dock"]["id"], dock);
        assert!(j["update_dock"]["docked_vehicle"].is_null());

        // destroy：对象类别、blocked 补丁（元组序列化为 [x,y] 数组）、退款。
        let mut w3 = World::new_empty(6, 5, 123_000);
        let shelf = w3.add_shelf(Position::new(3, 3));
        let (code, eff) = w3.manage_destroy(shelf);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_destroy(&w3, &eff.unwrap())).unwrap();
        assert_eq!(j["kind"], "destroy");
        assert_eq!(j["object"], "shelf");
        assert_eq!(j["id"], shelf);
        assert_eq!(j["gold_milli"], "210500"); // 123 + 87.5（设备价 175 的一半）
        assert_eq!(j["unblock"], serde_json::json!([[3, 3]]));
        assert_eq!(j["rebuild"], false);

        // destroy 装卸位：锚点与第二格同时解除占用。
        let mut w5 = World::new_empty(6, 5, 123_000);
        let d5 = w5.add_dock(Position::new(0, 4), (1, 0));
        let (code, eff) = w5.manage_destroy(d5);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_destroy(&w5, &eff.unwrap())).unwrap();
        assert_eq!(j["object"], "dock");
        assert_eq!(j["unblock"], serde_json::json!([[0, 4], [1, 4]]));
        assert_eq!(j["rebuild"], false);

        // destroy 被携带货物：无地面格 → rebuild 置位走整体重建。
        let mut w4 = World::new_empty(6, 5, 123_000);
        let r = w4.add_robot(Position::new(1, 1));
        let bx = w4.next_id;
        w4.next_id += 1;
        w4.ground_boxes.insert(
            bx,
            ztw_model::GroundBox {
                id: bx,
                goods_type: "water".into(),
                pos: Position::new(1, 1),
                holder: Some(r),
            },
        );
        w4.robots.get_mut(&r).unwrap().carry = Some(bx);
        let (code, eff) = w4.manage_destroy(bx);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_destroy(&w4, &eff.unwrap())).unwrap();
        assert_eq!(j["object"], "box");
        assert_eq!(j["rebuild"], true);
        assert_eq!(j["unblock"], serde_json::json!([]));

        // funds（borrow / repay 共用）：只动金币与欠款两根线。
        let mut w6 = World::new_empty(6, 5, 123_000);
        let (code, eff) = w6.manage_borrow(50_000);
        assert_eq!(code, "OK");
        let eff = eff.unwrap();
        let j = serde_json::to_value(MirrorDelta::from_borrow(&w6, &eff)).unwrap();
        assert_eq!(j["kind"], "funds");
        assert_eq!(j["gold_milli"], "173000");
        assert_eq!(j["debt_milli"], "50000");
        let (code, eff) = w6.manage_repay(20_000);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_repay(&w6, &eff.unwrap())).unwrap();
        assert_eq!(j["gold_milli"], "153000");
        assert_eq!(j["debt_milli"], "30000");

        // buy：对象类别、全量视图入列、blocked 补丁；dock 朝向随视图携带。
        let mut w7 = World::new_empty(8, 6, 1_000_000);
        for y in 0..6 {
            w7.add_wall(Position::new(0, y));
            w7.add_wall(Position::new(7, y));
        }
        for x in 0..8 {
            w7.add_wall(Position::new(x, 0));
            w7.add_wall(Position::new(x, 5));
        }
        let (code, eff) = w7.manage_buy("shelf", 3, 3);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_buy(&w7, &eff.unwrap())).unwrap();
        assert_eq!(j["kind"], "buy");
        assert_eq!(j["object"], "shelf");
        assert_eq!(j["gold_milli"], "825000"); // 1000 − 175
        assert_eq!(j["block"], serde_json::json!([[3, 3]]));
        assert_eq!(j["shelf"]["x"], 3);
        assert_eq!(j["shelf"]["capacity"], 4);
        assert_eq!(j["shelf"]["boxes"], serde_json::json!([]));
        assert!(j["robot"].is_null()); // 未购对象字段整体缺省

        // buy dock：锚点开墙、两格 blocked、ext 指向库内。
        let (code, eff) = w7.manage_buy("dock", 0, 3);
        assert_eq!(code, "OK");
        let eff = eff.unwrap();
        let j = serde_json::to_value(MirrorDelta::from_buy(&w7, &eff)).unwrap();
        assert_eq!(j["object"], "dock");
        assert_eq!(j["gold_milli"], "325000"); // 825 − 500
        assert_eq!(j["block"], serde_json::json!([[0, 3], [1, 3]]));
        assert_eq!(j["dock"]["ext"], serde_json::json!([1, 0]));
        assert_eq!(j["dock"]["docked_vehicle"], serde_json::Value::Null);
        // 增量回放与全量镜像同源：直接对比视图序列化。
        let full = MirrorView::from_world(&w7, 1);
        assert_eq!(
            j["dock"],
            serde_json::to_value(full.docks.iter().find(|d| d.id == eff.id).unwrap()).unwrap()
        );
    }
}
