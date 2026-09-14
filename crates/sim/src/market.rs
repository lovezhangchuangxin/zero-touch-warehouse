//! 管理操作（即时生效；docs/game-design/08）：market.take / market.cancel /
//! manage.destroy / manage.borrow / manage.repay / manage.buy，以及供 api 层
//! 构造镜像增量的效果摘要。

use ztw_model::{Id, MilliGold, Order, OrderSide, Position, VehicleKind, codes};

use crate::world::{PendingArrival, World};
use crate::{
    CANCEL_FEE_DENOMINATOR, CANCEL_FEE_NUMERATOR, CREDIT_LIMIT_MILLI, DESTROY_REFUND_DENOMINATOR,
    DESTROY_REFUND_NUMERATOR, PRICE_CHARGER, PRICE_DOCK, PRICE_ROBOT, PRICE_SHELF,
};

/// take 的镜像同步增量素材（api 层据此构造宿主回放增量）。
#[derive(Debug, Clone)]
pub struct TakeEffect {
    pub order_id: Id,
    pub dock_id: Id,
    pub order: Order,
}

/// cancel 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct CancelEffect {
    pub order_id: Id,
    pub vehicle_id: Option<Id>,
    pub dock_id: Option<Id>,
    /// 卖单（买入）取消的退款；买单为 0。
    pub refund_milli: MilliGold,
    pub fee_milli: MilliGold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyedKind {
    Robot,
    Shelf,
    Charger,
    Dock,
    GroundBox,
}

/// borrow 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct BorrowEffect {
    pub amount_milli: MilliGold,
}

/// repay 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct RepayEffect {
    /// 实际归还额（以欠款为上限钳定后）。
    pub amount_milli: MilliGold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoughtKind {
    Robot,
    Shelf,
    Charger,
    Dock,
}

/// buy 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct BuyEffect {
    pub kind: BoughtKind,
    pub id: Id,
    pub pos: Position,
    /// 装卸位朝向（唯一内向法向推导）；其余对象为 None。
    pub ext: Option<(i32, i32)>,
    pub price_milli: MilliGold,
    /// 新增静态障碍格（装卸位两格；机器人无）。
    pub blocked_cells: Vec<Position>,
}

/// destroy 的镜像同步增量素材。
#[derive(Debug, Clone)]
pub struct DestroyEffect {
    pub kind: DestroyedKind,
    pub target_id: Id,
    pub refund_milli: MilliGold,
    /// 释放的静态障碍格（镜像 blocked 增量用；装卸位为两格，被携带 /
    /// 在架货物无地面格）。
    pub freed_cells: Vec<Position>,
}

impl World {
    /// 接单：校验并预留空闲装卸位；卖单（玩家买入）即时扣款；车辆下一 tick
    /// 边界到场。空闲位由装卸位分配流随机占用（docs/game-design/04）。
    pub fn manage_take(&mut self, order_id: Id) -> (&'static str, Option<TakeEffect>) {
        let Some(order) = self.listings.get(&order_id) else {
            return (codes::ORDER_GONE, None);
        };
        let side = order.side;
        let qty = order.qty;
        let unit_price = order.unit_price_milli;
        let mut free_docks: Vec<Id> = self
            .docks
            .values()
            .filter(|d| d.docked_vehicle.is_none() && d.reserved_for.is_none())
            .map(|d| d.id)
            .collect();
        free_docks.sort_unstable(); // 遍历序规则：按 id 升序后抽样
        if free_docks.is_empty() {
            return (codes::NO_FREE_DOCK, None);
        }
        let idx = self.rng_dock.below(free_docks.len());
        let dock_id = free_docks[idx];
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
        self.docks.get_mut(&dock_id).expect("存在").reserved_for = Some(order_id);
        let mut taken = order;
        taken.dock = Some(dock_id);
        self.my_orders.insert(order_id, taken.clone());
        self.arrivals.push(PendingArrival {
            order_id,
            arrive_tick: self.tick + 1,
        });
        let effect = TakeEffect {
            order_id,
            dock_id,
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
        let dock_id = order.dock;
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
        if let Some(did) = dock_id
            && let Some(d) = self.docks.get_mut(&did)
        {
            if d.docked_vehicle == vehicle_id {
                d.docked_vehicle = None;
            }
            if d.reserved_for == Some(order_id) {
                d.reserved_for = None;
            }
        }
        self.my_orders.remove(&order_id);
        (
            codes::OK,
            Some(CancelEffect {
                order_id,
                vehicle_id,
                dock_id,
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
            return self.finish_destroy(DestroyedKind::Robot, target_id, vec![pos], PRICE_ROBOT);
        }
        if let Some(s) = self.shelves.get(&target_id) {
            if !s.box_ids.is_empty() {
                return (codes::NOT_EMPTY, None);
            }
            let pos = s.pos;
            self.shelves.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Shelf, target_id, vec![pos], PRICE_SHELF);
        }
        if let Some(c) = self.chargers.get(&target_id) {
            let pos = c.pos;
            self.chargers.remove(&target_id);
            return self.finish_destroy(
                DestroyedKind::Charger,
                target_id,
                vec![pos],
                PRICE_CHARGER,
            );
        }
        if let Some(d) = self.docks.get(&target_id) {
            if d.docked_vehicle.is_some() || d.reserved_for.is_some() {
                return (codes::HAS_VEHICLE, None);
            }
            let cells = vec![d.pos, d.pos.step(d.ext.0, d.ext.1)];
            self.docks.remove(&target_id);
            return self.finish_destroy(DestroyedKind::Dock, target_id, cells, PRICE_DOCK);
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
            let freed = if holder.is_none() {
                vec![pos]
            } else {
                Vec::new()
            };
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
        freed_cells: Vec<Position>,
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
                freed_cells,
            }),
        )
    }

    /// 借款即时到账，受信用额度限制（docs/game-design/06：借贷是零金币
    /// 零库存时重启经营的保底；额度须高于购置一台机器人的解围成本）。
    pub fn manage_borrow(
        &mut self,
        amount_milli: MilliGold,
    ) -> (&'static str, Option<BorrowEffect>) {
        if amount_milli <= 0 {
            return (codes::INVALID_ARGUMENT, None);
        }
        // 溢出视同超额：任何超出额度的组合都归 CREDIT_EXCEEDED。
        match self
            .debt_milli
            .checked_add(amount_milli)
            .filter(|d| *d <= CREDIT_LIMIT_MILLI)
        {
            Some(_) => {}
            None => return (codes::CREDIT_EXCEEDED, None),
        }
        self.gold_milli += amount_milli;
        self.debt_milli += amount_milli;
        (codes::OK, Some(BorrowEffect { amount_milli }))
    }

    /// 归还部分或全部欠款：金额以欠款为上限，金币不足返回 NO_FUNDS、
    /// 不自动借贷（docs/game-design/08 管理操作）。
    pub fn manage_repay(&mut self, amount_milli: MilliGold) -> (&'static str, Option<RepayEffect>) {
        if amount_milli <= 0 {
            return (codes::INVALID_ARGUMENT, None);
        }
        let amount = amount_milli.min(self.debt_milli);
        if amount > self.gold_milli {
            return (codes::NO_FUNDS, None);
        }
        self.gold_milli -= amount;
        self.debt_milli -= amount;
        (
            codes::OK,
            Some(RepayEffect {
                amount_milli: amount,
            }),
        )
    }

    /// 商店购买：即时扣费、即时建成生效（docs/game-design/06）。校验次序
    /// 沿用 take 先例（放置类校验在前、资金在后）：越界 → 地面货物 →
    /// 静态障碍 → 金币。装卸位锚点必须是边界墙格，ext 取唯一内向法向，
    /// 角格（两个内向法向）与非墙格拒绝 NOT_ON_WALL。
    pub fn manage_buy(&mut self, kind: &str, x: i32, y: i32) -> (&'static str, Option<BuyEffect>) {
        let price = match kind {
            "robot" => PRICE_ROBOT,
            "shelf" => PRICE_SHELF,
            "charger" => PRICE_CHARGER,
            "dock" => PRICE_DOCK,
            _ => return (codes::INVALID_ARGUMENT, None),
        };
        if x < 0 || y < 0 || x >= self.map_w || y >= self.map_h {
            return (codes::OUT_OF_BOUNDS, None);
        }
        let pos = Position::new(x, y);
        // 装卸位：锚点开在边界墙上（缺口格），朝向自边界唯一推导。
        let ext = if kind == "dock" {
            let Some(inward) = self.inward_normal(x, y) else {
                return (codes::NOT_ON_WALL, None);
            };
            if !self.walls.contains(&pos) {
                return (codes::NOT_ON_WALL, None);
            }
            let second = pos.step(inward.0, inward.1);
            if second.x < 0 || second.y < 0 || second.x >= self.map_w || second.y >= self.map_h {
                return (codes::OUT_OF_BOUNDS, None);
            }
            if self.ground_box_at(second).is_some() {
                return (codes::CELL_OCCUPIED, None);
            }
            if !self.statically_passable(second) {
                return (codes::CELL_BLOCKED, None);
            }
            Some(inward)
        } else {
            if self.ground_box_at(pos).is_some() {
                return (codes::CELL_OCCUPIED, None);
            }
            if !self.statically_passable(pos) {
                return (codes::CELL_BLOCKED, None);
            }
            None
        };
        if price > self.gold_milli {
            return (codes::NO_FUNDS, None);
        }
        self.gold_milli -= price;
        let (id, blocked_cells) = match kind {
            // 构造器复用（含 id 分配与容器初始化）；先扣款后建成对调用方
            // 原子——构造器无失败路径。
            "robot" => (self.add_robot(pos), Vec::new()),
            "shelf" => (self.add_shelf(pos), vec![pos]),
            "charger" => (self.add_charger(pos), vec![pos]),
            _ => {
                let (ex, ey) = ext.expect("装卸位已推导朝向");
                self.remove_wall(pos);
                (self.add_dock(pos, (ex, ey)), vec![pos, pos.step(ex, ey)])
            }
        };
        let bought = match kind {
            "robot" => BoughtKind::Robot,
            "shelf" => BoughtKind::Shelf,
            "charger" => BoughtKind::Charger,
            _ => BoughtKind::Dock,
        };
        (
            codes::OK,
            Some(BuyEffect {
                kind: bought,
                id,
                pos,
                ext,
                price_milli: price,
                blocked_cells,
            }),
        )
    }

    /// 边界墙格的唯一内向法向：北 (0,1)、南 (0,-1)、西 (1,0)、东 (-1,0)；
    /// 角格两个内向方向歧义、非边界格无墙可贴，均返回 None。
    fn inward_normal(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        let on_ns = (y == 0) as u8 + (y == self.map_h - 1) as u8;
        let on_we = (x == 0) as u8 + (x == self.map_w - 1) as u8;
        if on_ns + on_we != 1 {
            return None; // 角格（两条边界）或内部格（零条）
        }
        if y == 0 {
            Some((0, 1))
        } else if y == self.map_h - 1 {
            Some((0, -1))
        } else if x == 0 {
            Some((1, 0))
        } else {
            Some((-1, 0))
        }
    }
}
