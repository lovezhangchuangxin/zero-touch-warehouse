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
use ztw_sim::{TakeEffect, World};

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

/// take 的镜像同步增量：宿主按 FIFO 回放到本地镜像。
/// take 的镜像同步增量。宿主按 FIFO 回放；字段必须与宿主回放逻辑一一对应
///（docs/architecture/03）。装卸口占用不由增量表达：PortView 不含
/// reserved_for（docs/game-design/08 字段表），预留期仅一个 tick，
/// 下一 tick 全量镜像即一致；接单权威以 take 结果码为准。
#[derive(Serialize, Debug, Clone)]
pub struct MirrorDelta {
    pub gold_milli: String,
    pub remove_listing: Id,
    pub add_my_order: OrderView,
}

impl MirrorDelta {
    pub fn from_take(world_after: &World, eff: &TakeEffect) -> MirrorDelta {
        MirrorDelta {
            gold_milli: money(world_after.gold_milli),
            remove_listing: eff.order_id,
            add_my_order: order_view(&eff.order),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("增量序列化不失败")
    }
}

/// 世界修订号：A0 以 tick 与 take 计数合成（镜像内容纯函数仍成立）。
#[derive(Debug, Clone, Default)]
pub struct WorldRevision {
    pub tick: u64,
    pub take_count: u64,
}

impl WorldRevision {
    pub fn get(&self) -> u64 {
        self.tick.wrapping_mul(1_000_003) ^ self.take_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztw_model::Position;

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
}
