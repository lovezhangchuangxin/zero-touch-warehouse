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
use ztw_sim::{CancelEffect, DestroyEffect, DestroyedKind, TakeEffect, World};

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
    /// 静态障碍格（墙 / 货架 / 充电桩 / 装卸口 / 地面货物）。
    pub blocked: Vec<(i32, i32)>,
    pub robots: Vec<RobotView>,
    pub shelves: Vec<ShelfView>,
    pub chargers: Vec<ChargerView>,
    pub ports: Vec<PortView>,
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

#[derive(Serialize, Debug, Clone)]
pub struct PortView {
    pub id: Id,
    pub x: i32,
    pub y: i32,
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
    pub port: Option<Id>,
}

fn order_view(o: &Order) -> OrderView {
    OrderView {
        id: o.id,
        side: o.side.as_str().to_string(),
        goods_type: o.goods_type.clone(),
        qty: o.qty,
        unit_price_milli: money(o.unit_price_milli),
        vehicle: o.vehicle,
        port: o.port,
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
                .map(|r| RobotView {
                    id: r.id,
                    x: r.pos.x,
                    y: r.pos.y,
                    energy: r.energy,
                    energy_max: r.energy_max,
                    carry: r.carry.map(box_view),
                    last_result: world.last_results.get(&r.id).map(|lr| LastResultView {
                        action: lr.action.to_string(),
                        arg: lr.arg.clone(),
                        code: lr.code.clone(),
                    }),
                })
                .collect(),
            shelves: world
                .shelves
                .values()
                .map(|s| ShelfView {
                    id: s.id,
                    x: s.pos.x,
                    y: s.pos.y,
                    boxes: s.box_ids.iter().map(|id| box_view(*id)).collect(),
                    capacity: s.capacity,
                })
                .collect(),
            chargers: world
                .chargers
                .values()
                .map(|c| ChargerView {
                    id: c.id,
                    x: c.pos.x,
                    y: c.pos.y,
                })
                .collect(),
            ports: world
                .ports
                .values()
                .map(|p| PortView {
                    id: p.id,
                    x: p.pos.x,
                    y: p.pos.y,
                    docked_vehicle: p.docked_vehicle,
                })
                .collect(),
            vehicles: world
                .vehicles
                .values()
                .map(|v| VehicleView {
                    id: v.id,
                    kind: v.kind.as_str().to_string(),
                    goods_type: v.goods_type.clone(),
                    x: v.interact_pos.x,
                    y: v.interact_pos.y,
                    order_id: v.order_id,
                    boxes: v.box_ids.iter().map(|id| box_view(*id)).collect(),
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
/// 回放逻辑一一对应（docs/architecture/03）。装卸口预留不进增量：
/// PortView 不含 reserved_for（docs/game-design/08 字段表），预留期仅一个
/// tick，下一 tick 全量镜像即一致；接单权威以 take 结果码为准。
#[derive(Serialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MirrorDelta {
    Take(TakeDelta),
    Cancel(CancelDelta),
    Destroy(DestroyDelta),
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
    pub update_port: Option<PortPatch>,
}

#[derive(Serialize, Debug, Clone)]
pub struct PortPatch {
    pub id: Id,
    pub docked_vehicle: Option<Id>,
}

#[derive(Serialize, Debug, Clone)]
pub struct DestroyDelta {
    /// 对象类别：robot / shelf / charger / port / box。
    pub object: String,
    pub id: Id,
    pub gold_milli: String,
    /// 释放的静态障碍格（blocked 数组补丁，保同 tick 可见）。
    pub unblock: Vec<(i32, i32)>,
    /// 嵌套视图受影响（销毁被携带 / 在架货物）→ 增量不猜测，宿主整体重建。
    pub rebuild: bool,
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
            update_port: eff.port_id.map(|id| PortPatch {
                id,
                docked_vehicle: world_after.ports.get(&id).and_then(|p| p.docked_vehicle),
            }),
        })
    }

    pub fn from_destroy(world_after: &World, eff: &DestroyEffect) -> MirrorDelta {
        let object = match eff.kind {
            DestroyedKind::Robot => "robot",
            DestroyedKind::Shelf => "shelf",
            DestroyedKind::Charger => "charger",
            DestroyedKind::Port => "port",
            DestroyedKind::GroundBox => "box",
        };
        MirrorDelta::Destroy(DestroyDelta {
            object: object.to_string(),
            id: eff.target_id,
            gold_milli: money(world_after.gold_milli),
            unblock: eff.freed_cell.map(|p| vec![(p.x, p.y)]).unwrap_or_default(),
            // 无地面格的货物 = 被携带 / 在架，影响嵌套视图（robot.carry /
            // shelf.boxes），增量无法补丁，置 rebuild 走整体重建。
            rebuild: matches!(eff.kind, DestroyedKind::GroundBox) && eff.freed_cell.is_none(),
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("增量序列化不失败")
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
        w.add_port(Position::new(0, 4));
        w.add_listing(OrderSide::Sell, "battery", 2, 5_000);
        let m = MirrorView::from_world(&w, 1);
        let json = m.to_json();
        assert!(json.contains("\"gold_milli\":\"123000\""));
        assert!(json.contains("\"unit_price_milli\":\"5000\""));
        assert_eq!(m.robots.len(), 1);
        assert_eq!(m.sell_orders.len(), 1);
        assert_eq!(m.blocked.len(), 2); // 货架 + 装卸口
    }

    /// 增量 JSON 形状契约：宿主 applyDelta 逐字段回放（bootstrap.js），
    /// 序列化属性与字段名的任何改动都必须同步此处。
    #[test]
    fn delta_shapes() {
        // take：标签 + 挂单移除 + 已接订单回填。
        let mut w = World::new_empty(6, 5, 123_000);
        w.add_port(Position::new(0, 4));
        let o = w.add_listing(OrderSide::Sell, "battery", 2, 5_000);
        let (code, eff) = w.manage_take(o);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_take(&w, &eff.unwrap())).unwrap();
        assert_eq!(j["kind"], "take");
        assert_eq!(j["gold_milli"], "113000"); // 字符串防浮点
        assert_eq!(j["remove_listing"], o);
        assert_eq!(j["add_my_order"]["id"], o);
        assert_eq!(j["add_my_order"]["port"], *w.ports.keys().next().unwrap());

        // cancel（未到场）：remove_vehicle / update_port 为 null。
        let (code, eff) = w.manage_cancel(o);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_cancel(&w, &eff.unwrap())).unwrap();
        assert_eq!(j["kind"], "cancel");
        assert_eq!(j["gold_milli"], "122000"); // 123 - 10 + 退 9（手续费 1）
        assert_eq!(j["debt_milli"], "0");
        assert_eq!(j["remove_my_order"], o);
        assert!(j["remove_vehicle"].is_null());
        // 未到场取消：车辆为 null，但预留口的补丁对象仍在（清 docked 语义）。
        assert_eq!(j["update_port"]["id"], *w.ports.keys().next().unwrap());
        assert!(j["update_port"]["docked_vehicle"].is_null());

        // cancel（到场后）：车辆移除与装卸口清空补丁。
        let mut w2 = World::new_empty(6, 5, 123_000);
        let port = w2.add_port(Position::new(0, 4));
        let o2 = w2.add_listing(OrderSide::Sell, "battery", 1, 5_000);
        w2.manage_take(o2);
        w2.end_tick();
        w2.boundary_events();
        let vid = w2.ports[&port].docked_vehicle.unwrap();
        let (code, eff) = w2.manage_cancel(o2);
        assert_eq!(code, "OK");
        let j = serde_json::to_value(MirrorDelta::from_cancel(&w2, &eff.unwrap())).unwrap();
        assert_eq!(j["remove_vehicle"], vid);
        assert_eq!(j["update_port"]["id"], port);
        assert!(j["update_port"]["docked_vehicle"].is_null());

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
    }
}
