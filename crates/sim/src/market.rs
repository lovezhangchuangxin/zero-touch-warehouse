//! 管理操作（即时生效；docs/game-design/08）：market.take / market.cancel /
//! manage.destroy，以及供 api 层构造镜像增量的效果摘要。

use ztw_model::{Id, MilliGold, Order, OrderSide, Position, VehicleKind, codes};

use crate::world::{PendingArrival, World};
use crate::{
    CANCEL_FEE_DENOMINATOR, CANCEL_FEE_NUMERATOR, DESTROY_REFUND_DENOMINATOR,
    DESTROY_REFUND_NUMERATOR, PRICE_CHARGER, PRICE_PORT, PRICE_ROBOT, PRICE_SHELF,
};

/// take 的镜像同步增量素材（api 层据此构造宿主回放增量）。
#[derive(Debug, Clone)]
pub struct TakeEffect {
    pub order_id: Id,
    pub port_id: Id,
    pub order: Order,
}

/// cancel 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct CancelEffect {
    pub order_id: Id,
    pub vehicle_id: Option<Id>,
    pub port_id: Option<Id>,
    /// 卖单（买入）取消的退款；买单为 0。
    pub refund_milli: MilliGold,
    pub fee_milli: MilliGold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyedKind {
    Robot,
    Shelf,
    Charger,
    Port,
    GroundBox,
}

/// destroy 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct DestroyEffect {
    pub kind: DestroyedKind,
    pub target_id: Id,
    pub refund_milli: MilliGold,
    /// 释放的静态障碍格（镜像 blocked 增量用）；被携带 / 在架货物无地面格。
    pub freed_cell: Option<Position>,
}

impl World {
    /// 接单：校验并预留空闲装卸位；卖单（玩家买入）即时扣款；车辆下一 tick
    /// 边界到场。空闲口由装卸位分配流随机占用（docs/game-design/04）。
    pub fn manage_take(&mut self, order_id: Id) -> (&'static str, Option<TakeEffect>) {
        let Some(order) = self.listings.get(&order_id) else {
            return (codes::ORDER_GONE, None);
        };
        let side = order.side;
        let qty = order.qty;
        let unit_price = order.unit_price_milli;
        let mut free_ports: Vec<Id> = self
            .ports
            .values()
            .filter(|p| p.docked_vehicle.is_none() && p.reserved_for.is_none())
            .map(|p| p.id)
            .collect();
        free_ports.sort_unstable(); // 遍历序规则：按 id 升序后抽样
        if free_ports.is_empty() {
            return (codes::NO_FREE_PORT, None);
        }
        let idx = self.rng_port.below(free_ports.len());
        let port_id = free_ports[idx];
        // 卖单（玩家买入）需即时扣款：成本只算一次，校验与扣款同源。
        let cost = match side {
            OrderSide::Sell => (qty as i64).checked_mul(unit_price),
            OrderSide::Buy => Some(0),
        };
        let cost = match cost {
            Some(c) if c <= self.gold_milli || side == OrderSide::Buy => c,
            _ => return (codes::NO_FUNDS, None),
        };
        let order = self.listings.remove(&order_id).expect("已校验存在");
        if side == OrderSide::Sell {
            self.gold_milli -= cost;
        }
        self.ports.get_mut(&port_id).expect("存在").reserved_for = Some(order_id);
        let mut taken = order;
        taken.port = Some(port_id);
        self.my_orders.insert(order_id, taken.clone());
        self.arrivals.push(PendingArrival {
            order_id,
            arrive_tick: self.tick + 1,
        });
        let effect = TakeEffect {
            order_id,
            port_id,
            order: taken,
        };
        (codes::OK, Some(effect))
    }

    /// 取消订单：车辆须恢复到场初态（入库车满、出库车空；同类型同数量即可，
    /// 未到场即初态）。卖单（买入）退款扣手续费；买单（卖出）手续费扣金币、
    /// 不足自动计欠款。取消的订单不回市场（docs/game-design/04）。
    pub fn manage_cancel(&mut self, order_id: Id) -> (&'static str, Option<CancelEffect>) {
        let Some(order) = self.my_orders.get(&order_id) else {
            return (codes::ORDER_GONE, None);
        };
        let side = order.side;
        let qty = order.qty;
        let amount = (qty as i64).saturating_mul(order.unit_price_milli);
        let vehicle_id = order.vehicle;
        let port_id = order.port;
        let fee = amount / CANCEL_FEE_DENOMINATOR * CANCEL_FEE_NUMERATOR;
        // 车辆到场后校验可恢复性；未到场（arrival 待定）即初态。
        if let Some(vid) = vehicle_id {
            let Some(v) = self.vehicles.get(&vid) else {
                return (codes::ORDER_GONE, None); // 理论不可达：在场车辆必在
            };
            let restorable = match v.kind {
                VehicleKind::In => v.box_ids.len() as u32 == qty,
                VehicleKind::Out => v.box_ids.is_empty(),
            };
            if !restorable {
                return (codes::GOODS_MOVED, None);
            }
        }
        let (refund, gold_pay, debt_add) = match side {
            OrderSide::Sell => ((amount - fee).max(0), 0, 0), // 退款下限 0
            OrderSide::Buy => {
                let pay = fee.min(self.gold_milli.max(0));
                (0, pay, fee - pay) // 手续费扣金币、不足计欠款
            }
        };
        self.gold_milli += refund - gold_pay;
        self.debt_milli += debt_add;
        // 车辆与其上货物退回对方；未到场则撤销到场事件。
        if let Some(vid) = vehicle_id {
            if let Some(v) = self.vehicles.remove(&vid) {
                for b in &v.box_ids {
                    self.ground_boxes.remove(b);
                }
            }
        } else {
            self.arrivals.retain(|a| a.order_id != order_id);
        }
        if let Some(pid) = port_id
            && let Some(p) = self.ports.get_mut(&pid)
        {
            if p.docked_vehicle == vehicle_id {
                p.docked_vehicle = None;
            }
            if p.reserved_for == Some(order_id) {
                p.reserved_for = None;
            }
        }
        self.my_orders.remove(&order_id);
        (
            codes::OK,
            Some(CancelEffect {
                order_id,
                vehicle_id,
                port_id,
                refund_milli: refund,
                fee_milli: fee,
            }),
        )
    }

    /// 最小销毁管理操作（docs/architecture/08 原型 B 回归第 5 条所需）。
    /// 退款为设备价一半（临时锚点）；车辆不可直接销毁（cancel 是唯一路径）；
    /// 买入货物卸离车辆前不可销毁（docs/game-design/04）。
    pub fn manage_destroy(&mut self, target_id: Id) -> (&'static str, Option<DestroyEffect>) {
        if let Some(r) = self.robots.get(&target_id) {
            if r.carry.is_some() {
                return (codes::NOT_EMPTY, None);
            }
            let pos = r.pos;
            self.robots.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Robot, target_id, Some(pos), PRICE_ROBOT);
        }
        if let Some(s) = self.shelves.get(&target_id) {
            if !s.box_ids.is_empty() {
                return (codes::NOT_EMPTY, None);
            }
            let pos = s.pos;
            self.shelves.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Shelf, target_id, Some(pos), PRICE_SHELF);
        }
        if let Some(c) = self.chargers.get(&target_id) {
            let pos = c.pos;
            self.chargers.remove(&target_id);
            return self.finish_destroy(
                DestroyedKind::Charger,
                target_id,
                Some(pos),
                PRICE_CHARGER,
            );
        }
        if let Some(p) = self.ports.get(&target_id) {
            if p.docked_vehicle.is_some() || p.reserved_for.is_some() {
                return (codes::HAS_VEHICLE, None);
            }
            let pos = p.pos;
            self.ports.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Port, target_id, Some(pos), PRICE_PORT);
        }
        if let Some(b) = self.ground_boxes.get(&target_id) {
            let holder = b.holder;
            let pos = b.pos;
            // 仅入库（购入）车上的货物不可销毁（docs/game-design/04「买入的
            // 货物在卸离车辆前不可销毁」）；出库车上的箱是玩家自有履约货，
            // 销毁后仍可补同类型箱完成订单，允许止损。
            if holder.is_some_and(|h| {
                self.vehicles
                    .get(&h)
                    .is_some_and(|v| v.kind == VehicleKind::In)
            }) {
                return (codes::ON_VEHICLE, None);
            }
            self.ground_boxes.remove(&target_id);
            let freed = if holder.is_none() { Some(pos) } else { None };
            if let Some(h) = holder {
                if let Some(r) = self.robots.get_mut(&h) {
                    if r.carry == Some(target_id) {
                        r.carry = None;
                    }
                } else if let Some(s) = self.shelves.get_mut(&h) {
                    s.box_ids.retain(|b| b != &target_id);
                } else if let Some(v) = self.vehicles.get_mut(&h) {
                    v.box_ids.retain(|b| b != &target_id);
                }
            }
            // 货物无设备价，销毁止损无退款。
            return self.finish_destroy(DestroyedKind::GroundBox, target_id, freed, 0);
        }
        if self.vehicles.contains_key(&target_id) {
            return (codes::INVALID_TARGET, None);
        }
        (codes::NO_SUCH_OBJECT, None)
    }

    fn finish_destroy(
        &mut self,
        kind: DestroyedKind,
        target_id: Id,
        freed_cell: Option<Position>,
        price: MilliGold,
    ) -> (&'static str, Option<DestroyEffect>) {
        let refund = price / DESTROY_REFUND_DENOMINATOR * DESTROY_REFUND_NUMERATOR;
        self.gold_milli += refund;
        (
            codes::OK,
            Some(DestroyEffect {
                kind,
                target_id,
                refund_milli: refund,
                freed_cell,
            }),
        )
    }
}
