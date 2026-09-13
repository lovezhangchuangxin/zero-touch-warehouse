//! 世界状态：结构与构造器、静态查询、交互目标解析（受理与结算共用）、
//! tick 边界事件、阶段 5 收尾（end_tick）与确定性摘要（state_hash）。
//! 动作受理见 `accept`，管理操作见 `market`，统一结算见 `settle`。

use std::collections::{BTreeMap, BTreeSet};

use ztw_model::{
    Charger, Dock, GroundBox, Id, MilliGold, Order, OrderSide, Position, Robot, Shelf, Vehicle,
    VehicleKind, codes,
};

use crate::intent::{Intent, LastResult, TargetRef};
use crate::rng::Xoshiro256;

/// take 排定的车辆到场事件（tick 边界、loop() 之前生效）。
#[derive(Debug, Clone)]
pub(crate) struct PendingArrival {
    pub(crate) order_id: Id,
    pub(crate) arrive_tick: u64,
}

#[derive(Debug, Clone)]
pub struct World {
    pub tick: u64,
    pub map_w: i32,
    pub map_h: i32,
    /// 静态障碍之外的墙（测试用）；货架 / 充电桩 / 装卸位 / 地面货物另行计入。
    walls: BTreeSet<Position>,
    /// 世界种子：一切子 PRNG 流的派生根。
    pub seed: u64,
    /// 装卸位分配流（docs/architecture/02：市场、装卸位分配、场景独立分流）。
    pub(crate) rng_dock: Xoshiro256,
    pub next_id: Id,
    pub gold_milli: MilliGold,
    pub debt_milli: MilliGold,
    pub robots: BTreeMap<Id, Robot>,
    pub shelves: BTreeMap<Id, Shelf>,
    pub chargers: BTreeMap<Id, Charger>,
    pub docks: BTreeMap<Id, Dock>,
    pub vehicles: BTreeMap<Id, Vehicle>,
    pub ground_boxes: BTreeMap<Id, GroundBox>,
    /// 市场挂单（固定挂单；刷新与价格波动属里程碑 3）。
    pub listings: BTreeMap<Id, Order>,
    /// 已接未完成订单。
    pub my_orders: BTreeMap<Id, Order>,
    pub(crate) arrivals: Vec<PendingArrival>,
    /// 本 tick 已受理意图。
    pub(crate) intents: Vec<Intent>,
    /// 上一 tick 结算结果，按机器人 id。
    pub last_results: BTreeMap<Id, LastResult>,
}

impl World {
    pub fn new_empty(map_w: i32, map_h: i32, gold_milli: MilliGold) -> World {
        World {
            tick: 0,
            map_w,
            map_h,
            walls: BTreeSet::new(),
            seed: 0,
            rng_dock: Xoshiro256::derive(0, "dock"),
            next_id: 1,
            gold_milli,
            debt_milli: 0,
            robots: BTreeMap::new(),
            shelves: BTreeMap::new(),
            chargers: BTreeMap::new(),
            docks: BTreeMap::new(),
            vehicles: BTreeMap::new(),
            ground_boxes: BTreeMap::new(),
            listings: BTreeMap::new(),
            my_orders: BTreeMap::new(),
            arrivals: Vec::new(),
            intents: Vec::new(),
            last_results: BTreeMap::new(),
        }
    }

    /// 设定世界种子（须在世界构造前调用，派生流随之重建）。
    pub fn with_seed(mut self, seed: u64) -> World {
        self.seed = seed;
        self.rng_dock = Xoshiro256::derive(seed, "dock");
        self
    }

    fn alloc_id(&mut self) -> Id {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_wall(&mut self, p: Position) {
        self.walls.insert(p);
    }

    pub fn add_robot(&mut self, pos: Position) -> Id {
        let id = self.alloc_id();
        self.robots.insert(
            id,
            Robot {
                id,
                pos,
                energy: 100,
                energy_max: 100,
                carry: None,
            },
        );
        id
    }

    pub fn add_shelf(&mut self, pos: Position) -> Id {
        self.add_shelf_sized(pos, 4)
    }

    /// 指定容量的货架（测试与场景构造用）。
    pub fn add_shelf_sized(&mut self, pos: Position, capacity: usize) -> Id {
        let id = self.alloc_id();
        self.shelves.insert(
            id,
            Shelf {
                id,
                pos,
                box_ids: Vec::new(),
                capacity,
            },
        );
        id
    }

    pub fn add_charger(&mut self, pos: Position) -> Id {
        let id = self.alloc_id();
        self.chargers.insert(id, Charger { id, pos });
        id
    }

    /// 添加装卸位：锚点为靠墙缺口格，`ext` 指向库内第二格（单位偏移）。
    /// 两格均计入静态障碍；第二格须在界内且不与既有静态设施重叠（debug 断言，
    /// 场景构造负责满足）。
    pub fn add_dock(&mut self, pos: Position, ext: (i32, i32)) -> Id {
        debug_assert!(
            ext.0.abs() + ext.1.abs() == 1,
            "装卸位 ext 必须为单位偏移向量"
        );
        let second = pos.step(ext.0, ext.1);
        debug_assert!(
            second.x >= 0 && second.y >= 0 && second.x < self.map_w && second.y < self.map_h,
            "装卸位第二格 {second:?} 越界"
        );
        let occupied = |p: Position| {
            self.walls.contains(&p)
                || self.shelves.values().any(|s| s.pos == p)
                || self.chargers.values().any(|c| c.pos == p)
                || self
                    .docks
                    .values()
                    .any(|d| d.pos == p || d.pos.step(d.ext.0, d.ext.1) == p)
        };
        debug_assert!(!occupied(pos), "装卸位锚点 {pos:?} 与既有设施重叠");
        debug_assert!(!occupied(second), "装卸位第二格 {second:?} 与既有设施重叠");
        let id = self.alloc_id();
        self.docks.insert(
            id,
            Dock {
                id,
                pos,
                ext,
                docked_vehicle: None,
                reserved_for: None,
            },
        );
        id
    }

    /// 直接放置地面货物（测试与场景构造用；每格至多一箱由调用方保证）。
    pub fn add_ground_box(&mut self, goods_type: &str, pos: Position) -> Id {
        let id = self.alloc_id();
        self.ground_boxes.insert(
            id,
            GroundBox {
                id,
                goods_type: goods_type.to_string(),
                pos,
                holder: None,
            },
        );
        id
    }

    pub fn add_listing(
        &mut self,
        side: OrderSide,
        goods_type: &str,
        qty: u32,
        unit_price_milli: MilliGold,
    ) -> Id {
        let id = self.alloc_id();
        self.listings.insert(
            id,
            Order {
                id,
                side,
                goods_type: goods_type.to_string(),
                qty,
                unit_price_milli,
                vehicle: None,
                dock: None,
            },
        );
        id
    }

    // -----------------------------------------------------------------------
    // 静态查询
    // -----------------------------------------------------------------------

    /// 静态可通行判定：界内且非墙、非货架、非充电桩、非装卸位、非地面货物。
    /// 装卸位两格（锚点 + 库内第二格）均计入障碍，停靠货车不再另行占格；
    /// 机器人不是静态障碍。
    pub fn statically_passable(&self, p: Position) -> bool {
        if p.x < 0 || p.y < 0 || p.x >= self.map_w || p.y >= self.map_h {
            return false;
        }
        if self.walls.contains(&p) {
            return false;
        }
        if self.shelves.values().any(|s| s.pos == p) {
            return false;
        }
        if self.chargers.values().any(|c| c.pos == p) {
            return false;
        }
        if self
            .docks
            .values()
            .any(|d| d.pos == p || d.pos.step(d.ext.0, d.ext.1) == p)
        {
            return false;
        }
        if self
            .ground_boxes
            .values()
            .any(|b| b.holder.is_none() && b.pos == p)
        {
            return false;
        }
        true
    }

    /// tick 开始时占位该格的机器人（若有）。
    pub(crate) fn robot_at(&self, p: Position) -> Option<Id> {
        self.robots.values().find(|r| r.pos == p).map(|r| r.id)
    }

    /// 该地面格上的无主货物（每格至多一箱由受理与结算维护）。
    pub(crate) fn ground_box_at(&self, p: Position) -> Option<Id> {
        self.ground_boxes
            .values()
            .find(|b| b.holder.is_none() && b.pos == p)
            .map(|b| b.id)
    }

    // -----------------------------------------------------------------------
    // 交互目标解析（受理与结算共用）
    // -----------------------------------------------------------------------

    /// 受理前公共检查：机器人存在且本 tick 未行动。
    pub(crate) fn actor(&self, robot_id: Id) -> Result<&Robot, &'static str> {
        let Some(robot) = self.robots.get(&robot_id) else {
            return Err(codes::NO_SUCH_OBJECT);
        };
        if self.intents.iter().any(|i| i.robot_id() == robot_id) {
            return Err(codes::ALREADY_ACTED);
        }
        Ok(robot)
    }

    /// 解析交互目标：货架 / 车辆 / 机器人之外（含不存在）报错。
    pub(crate) fn resolve_target(&self, target_id: Id) -> Result<TargetRef, &'static str> {
        if self.shelves.contains_key(&target_id) {
            Ok(TargetRef::Shelf(target_id))
        } else if self.vehicles.contains_key(&target_id) {
            Ok(TargetRef::Vehicle(target_id))
        } else if self.robots.contains_key(&target_id) {
            Ok(TargetRef::Robot(target_id))
        } else if self.chargers.contains_key(&target_id)
            || self.docks.contains_key(&target_id)
            || self.ground_boxes.contains_key(&target_id)
        {
            Err(codes::INVALID_TARGET)
        } else {
            Err(codes::NO_SUCH_OBJECT)
        }
    }

    /// 交互格：车辆为 interact_pos，其余为所在格（docs/game-design/08）。
    pub(crate) fn target_pos(&self, t: TargetRef) -> Position {
        match t {
            TargetRef::Shelf(id) => self.shelves[&id].pos,
            TargetRef::Vehicle(id) => self.vehicles[&id].interact_pos,
            TargetRef::Robot(id) => self.robots[&id].pos,
        }
    }

    pub(crate) fn target_alive(&self, t: TargetRef) -> bool {
        match t {
            TargetRef::Shelf(id) => self.shelves.contains_key(&id),
            TargetRef::Vehicle(id) => self.vehicles.contains_key(&id),
            TargetRef::Robot(id) => self.robots.contains_key(&id),
        }
    }

    /// 目标当前是否持有该货物。
    pub(crate) fn target_holds(&self, t: TargetRef, box_id: Id) -> bool {
        match t {
            TargetRef::Shelf(id) => self.shelves[&id].box_ids.contains(&box_id),
            TargetRef::Vehicle(id) => self.vehicles[&id].box_ids.contains(&box_id),
            TargetRef::Robot(id) => self.robots[&id].carry == Some(box_id),
        }
    }

    /// 出库车的装载容量即订单数量（到场空车，装满即离场）。
    pub(crate) fn vehicle_capacity(&self, vehicle_id: Id) -> usize {
        let v = &self.vehicles[&vehicle_id];
        self.my_orders
            .get(&v.order_id)
            .map(|o| o.qty as usize)
            .unwrap_or(v.box_ids.len())
    }

    // -----------------------------------------------------------------------
    // 阶段 1：tick 边界事件（车辆到场；市场刷新属里程碑 3）
    // -----------------------------------------------------------------------

    pub fn boundary_events(&mut self) {
        // 到期判定含迟到补发（<=）：宿主跳过某 tick 的边界处理后，排定
        // 事件在下次调用时补上车，不因迟到而丢失（否则装卸位永久悬挂）。
        let mut due: Vec<Id> = self
            .arrivals
            .iter()
            .filter(|a| a.arrive_tick <= self.tick)
            .map(|a| a.order_id)
            .collect();
        due.sort_unstable(); // 同边界多辆车：按订单 id 升序到场（遍历序规则）
        self.arrivals.retain(|a| a.arrive_tick > self.tick);
        for order_id in due {
            self.spawn_vehicle(order_id);
        }
    }

    fn spawn_vehicle(&mut self, order_id: Id) {
        let Some(order) = self.my_orders.get(&order_id) else {
            return;
        };
        let kind = match order.side {
            OrderSide::Sell => VehicleKind::In,
            OrderSide::Buy => VehicleKind::Out,
        };
        let dock_id = order.dock.expect("已接订单必有预留装卸位");
        let interact_pos = {
            let d = self.docks.get(&dock_id).expect("装卸位存在");
            d.pos.step(d.ext.0, d.ext.1)
        };
        let goods = order.goods_type.clone();
        let qty = order.qty;
        let vehicle_id = self.alloc_id();
        let mut box_ids = Vec::new();
        if kind == VehicleKind::In {
            // 卖单车辆带货到场（固定流程：按订单数量生成货物）。
            for _ in 0..qty {
                let box_id = self.alloc_id();
                self.ground_boxes.insert(
                    box_id,
                    GroundBox {
                        id: box_id,
                        goods_type: goods.clone(),
                        pos: interact_pos,
                        holder: Some(vehicle_id),
                    },
                );
                box_ids.push(box_id);
            }
        }
        self.vehicles.insert(
            vehicle_id,
            Vehicle {
                id: vehicle_id,
                kind,
                goods_type: goods,
                interact_pos,
                order_id,
                box_ids,
            },
        );
        if let Some(dock) = self.docks.get_mut(&dock_id) {
            dock.docked_vehicle = Some(vehicle_id);
            dock.reserved_for = None;
        }
        if let Some(order) = self.my_orders.get_mut(&order_id) {
            order.vehicle = Some(vehicle_id);
        }
    }

    // -----------------------------------------------------------------------
    // 阶段 5 收尾与确定性摘要
    // -----------------------------------------------------------------------

    /// 结算后收尾（阶段 5 最小版）：tick +1。计息 / 渲染快照不在 B1。
    /// 正常时序为 settle → end_tick；若跳过结算直接推进，防御性丢弃未结算
    /// 意图，防止陈旧意图在后续 tick 照常执行（公开 API 脚枪加固）。
    pub fn end_tick(&mut self) {
        self.intents.clear();
        self.tick += 1;
    }

    /// 确定性回归用的世界摘要（排列测试对比）。覆盖全部影响后续行为的状态。
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mix = |v: u64, h: &mut u64| {
            *h ^= v;
            *h = h.wrapping_mul(0x100000001b3);
        };
        let mix_bytes = |s: &str, h: &mut u64| {
            for b in s.as_bytes() {
                mix(*b as u64, h);
            }
        };
        mix(self.tick, &mut h);
        mix(self.gold_milli as u64, &mut h);
        mix(self.debt_milli as u64, &mut h);
        mix(self.next_id, &mut h);
        mix(self.seed, &mut h);
        // 地图维度影响出界判定，是行为字段（契约：tick 边界调用）。
        mix(self.map_w as u64, &mut h);
        mix(self.map_h as u64, &mut h);
        for w in self.rng_dock.state_words() {
            mix(w, &mut h);
        }
        for p in &self.walls {
            mix(p.x as u64, &mut h);
            mix(p.y as u64, &mut h);
        }
        for (id, r) in &self.robots {
            mix(*id, &mut h);
            mix(r.pos.x as u64, &mut h);
            mix(r.pos.y as u64, &mut h);
            mix(r.energy as u64, &mut h);
            mix(r.energy_max as u64, &mut h);
            mix(r.carry.unwrap_or(0), &mut h);
        }
        for (id, res) in &self.last_results {
            mix(*id, &mut h);
            mix_bytes(&res.code, &mut h);
            mix_bytes(res.action, &mut h);
            mix_bytes(&res.arg, &mut h);
        }
        for (id, s) in &self.shelves {
            mix(*id, &mut h);
            mix(s.pos.x as u64, &mut h);
            mix(s.pos.y as u64, &mut h);
            mix(s.capacity as u64, &mut h);
            for b in &s.box_ids {
                mix(*b, &mut h);
            }
        }
        for (id, c) in &self.chargers {
            mix(*id, &mut h);
            mix(c.pos.x as u64, &mut h);
            mix(c.pos.y as u64, &mut h);
        }
        for (id, d) in &self.docks {
            mix(*id, &mut h);
            mix(d.pos.x as u64, &mut h);
            mix(d.pos.y as u64, &mut h);
            mix(d.ext.0 as u64, &mut h);
            mix(d.ext.1 as u64, &mut h);
            mix(d.docked_vehicle.unwrap_or(0), &mut h);
            mix(d.reserved_for.unwrap_or(0), &mut h);
        }
        for (id, v) in &self.vehicles {
            mix(*id, &mut h);
            mix(if v.kind.as_str() == "in" { 1 } else { 0 }, &mut h);
            mix(v.interact_pos.x as u64, &mut h);
            mix(v.interact_pos.y as u64, &mut h);
            mix(v.order_id, &mut h);
            mix_bytes(&v.goods_type, &mut h);
            for b in &v.box_ids {
                mix(*b, &mut h);
            }
        }
        for (id, b) in &self.ground_boxes {
            mix(*id, &mut h);
            mix(b.pos.x as u64, &mut h);
            mix(b.pos.y as u64, &mut h);
            mix(b.holder.unwrap_or(0), &mut h);
            mix_bytes(&b.goods_type, &mut h);
        }
        let mix_order = |o: &Order, h: &mut u64| {
            mix(o.id, h);
            mix(if o.side.as_str() == "buy" { 1 } else { 0 }, h);
            mix(o.qty as u64, h);
            mix(o.unit_price_milli as u64, h);
            mix(o.vehicle.unwrap_or(0), h);
            mix(o.dock.unwrap_or(0), h);
            mix_bytes(&o.goods_type, h);
        };
        for o in self.listings.values() {
            mix_order(o, &mut h);
        }
        for o in self.my_orders.values() {
            mix_order(o, &mut h);
        }
        for a in &self.arrivals {
            mix(a.order_id, &mut h);
            mix(a.arrive_tick, &mut h);
        }
        h
    }
}
