//! tick 状态机最小版、move 受理与统一结算（受理 → 只读求解 → 原子提交）、
//! 最小固定挂单市场。语义以 docs/game-design/03-simulation-and-actions.md 为准，
//! 结构以 docs/architecture/02-simulation-core.md「结算三段式」为准。
//!
//! A0 范围（docs/architecture/08）：机器人动作只实现 move；管理操作只实现
//! Game.market.take；车辆离场 / 收款按固定流程简化（A0 车辆到场后常驻，
//! 买入单扣款发生在 take 时；出库收款不在 A0）。

use std::collections::{BTreeMap, BTreeSet};

use ztw_model::{
    Charger, GroundBox, Id, MilliGold, Order, OrderSide, Port, Position, Robot, Shelf, Vehicle,
    VehicleKind, codes,
};

/// 已受理移动意图：每机器人每 tick 至多一条（docs/game-design/03 受理规则）。
#[derive(Debug, Clone, Copy)]
pub struct MoveIntent {
    pub robot_id: Id,
    pub dx: i32,
    pub dy: i32,
}

/// 上一 tick 结算结果（docs/game-design/03：成功为 OK；未受理动作时为 None）。
#[derive(Debug, Clone, PartialEq)]
pub struct LastResult {
    pub action: &'static str,
    /// 动作参数（move 为方向字符串）。
    pub arg: String,
    pub code: String,
}

#[derive(Debug, Clone)]
struct PendingArrival {
    order_id: Id,
    arrive_tick: u64,
}

#[derive(Debug, Clone)]
pub struct World {
    pub tick: u64,
    pub map_w: i32,
    pub map_h: i32,
    /// 静态障碍之外的墙（测试用）；货架 / 充电桩 / 装卸口 / 地面货物另行计入。
    walls: BTreeSet<Position>,
    pub next_id: Id,
    pub gold_milli: MilliGold,
    pub debt_milli: MilliGold,
    pub robots: BTreeMap<Id, Robot>,
    pub shelves: BTreeMap<Id, Shelf>,
    pub chargers: BTreeMap<Id, Charger>,
    pub ports: BTreeMap<Id, Port>,
    pub vehicles: BTreeMap<Id, Vehicle>,
    pub ground_boxes: BTreeMap<Id, GroundBox>,
    /// 市场挂单（固定挂单，A0 无刷新）。
    pub listings: BTreeMap<Id, Order>,
    /// 已接未完成订单。
    pub my_orders: BTreeMap<Id, Order>,
    /// take 排定的车辆到场事件（tick 边界、loop() 之前生效）。
    arrivals: Vec<PendingArrival>,
    /// 本 tick 已受理意图。
    intents: Vec<MoveIntent>,
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
            next_id: 1,
            gold_milli,
            debt_milli: 0,
            robots: BTreeMap::new(),
            shelves: BTreeMap::new(),
            chargers: BTreeMap::new(),
            ports: BTreeMap::new(),
            vehicles: BTreeMap::new(),
            ground_boxes: BTreeMap::new(),
            listings: BTreeMap::new(),
            my_orders: BTreeMap::new(),
            arrivals: Vec::new(),
            intents: Vec::new(),
            last_results: BTreeMap::new(),
        }
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
        let id = self.alloc_id();
        self.shelves.insert(
            id,
            Shelf {
                id,
                pos,
                box_ids: Vec::new(),
                capacity: 4,
            },
        );
        id
    }

    pub fn add_charger(&mut self, pos: Position) -> Id {
        let id = self.alloc_id();
        self.chargers.insert(id, Charger { id, pos });
        id
    }

    pub fn add_port(&mut self, pos: Position) -> Id {
        let id = self.alloc_id();
        self.ports.insert(
            id,
            Port {
                id,
                pos,
                docked_vehicle: None,
                reserved_for: None,
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
                port: None,
            },
        );
        id
    }

    /// 静态可通行判定：界内且非墙、非货架、非充电桩、非装卸口、非地面货物。
    /// 车辆停靠在装卸口格上（A0 简化），机器人不是静态障碍。
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
        if self.ports.values().any(|p0| p0.pos == p) {
            return false;
        }
        if self.ground_boxes.values().any(|b| b.pos == p) {
            return false;
        }
        true
    }

    /// tick 开始时占位该格的机器人（若有）。
    pub fn robot_at(&self, p: Position) -> Option<Id> {
        self.robots.values().find(|r| r.pos == p).map(|r| r.id)
    }

    // -----------------------------------------------------------------------
    // 阶段 1：tick 边界事件（车辆到场；市场刷新 A0 无）
    // -----------------------------------------------------------------------

    pub fn boundary_events(&mut self) {
        let mut due: Vec<Id> = self
            .arrivals
            .iter()
            .filter(|a| a.arrive_tick == self.tick)
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
        let port_id = order.port.expect("已接订单必有预留装卸口");
        let port_pos = self.ports.get(&port_id).expect("装卸口存在").pos;
        let goods = order.goods_type.clone();
        let qty = order.qty;
        let vehicle_id = self.alloc_id();
        let mut box_ids = Vec::new();
        if kind == VehicleKind::In {
            // 卖单车辆带货到场（A0 固定流程：按订单数量生成货物）。
            for _ in 0..qty {
                let box_id = self.alloc_id();
                self.ground_boxes.insert(
                    box_id,
                    GroundBox {
                        id: box_id,
                        goods_type: goods.clone(),
                        pos: port_pos,
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
                interact_pos: port_pos,
                order_id,
                box_ids,
            },
        );
        if let Some(port) = self.ports.get_mut(&port_id) {
            port.docked_vehicle = Some(vehicle_id);
            port.reserved_for = None;
        }
        if let Some(order) = self.my_orders.get_mut(&order_id) {
            order.vehicle = Some(vehicle_id);
        }
    }

    // -----------------------------------------------------------------------
    // 动作受理（静态检查，基于调用时世界状态；docs/game-design/03）
    // -----------------------------------------------------------------------

    /// move 受理：只判定调用时可知的静态条件。目标格争抢、链受阻等
    /// 依赖同 tick 其他意图的冲突一律留给结算。
    pub fn accept_move(&mut self, robot_id: Id, dx: i32, dy: i32) -> &'static str {
        if !(dx == 0 || dy == 0) || dx.abs() > 1 || dy.abs() > 1 || (dx == 0 && dy == 0) {
            return codes::INVALID_ARGUMENT;
        }
        let Some(robot) = self.robots.get(&robot_id) else {
            return codes::NO_SUCH_OBJECT;
        };
        if self.intents.iter().any(|i| i.robot_id == robot_id) {
            return codes::ALREADY_ACTED;
        }
        let target = robot.pos.step(dx, dy);
        if target.x < 0 || target.y < 0 || target.x >= self.map_w || target.y >= self.map_h {
            return codes::OUT_OF_BOUNDS;
        }
        if !self.statically_passable(target) {
            return codes::CELL_BLOCKED;
        }
        self.intents.push(MoveIntent { robot_id, dx, dy });
        codes::OK
    }

    // -----------------------------------------------------------------------
    // 管理操作：Game.market.take（即时生效；docs/game-design/08 管理操作）
    // -----------------------------------------------------------------------

    /// 接单：校验并预留空闲装卸位；卖单（玩家买入）即时扣款；车辆下一 tick
    /// 边界到场。返回结果码与镜像增量所需的影响摘要。
    pub fn manage_take(&mut self, order_id: Id) -> (&'static str, Option<TakeEffect>) {
        let Some(order) = self.listings.get(&order_id) else {
            return (codes::ORDER_GONE, None);
        };
        let side = order.side;
        let qty = order.qty;
        let unit_price = order.unit_price_milli;
        // 多装卸口时 A0 取 id 最小空位（文档为 PRNG 随机；简化记录在案）。
        let free_port = self
            .ports
            .values()
            .filter(|p| p.docked_vehicle.is_none() && p.reserved_for.is_none())
            .map(|p| p.id)
            .min();
        let Some(port_id) = free_port else {
            return (codes::NO_FREE_PORT, None);
        };
        // 卖单（玩家买入）需即时扣款：成本只算一次，校验与扣款同源。
        let cost = match side {
            OrderSide::Sell => (qty as i64).checked_mul(unit_price),
            OrderSide::Buy => Some(0),
        };
        let cost = match cost {
            Some(c) if c <= self.gold_milli || side == OrderSide::Buy => cost.unwrap_or(0),
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

    // -----------------------------------------------------------------------
    // 阶段 4：统一结算（受理 → 只读求解 → 原子提交）
    // -----------------------------------------------------------------------

    /// 结算本 tick 已受理意图。求解是「最终管理状态 + 意图集合 → 计划」的
    /// 纯函数：任意受理顺序得到同一计划（排列测试覆盖）。
    pub fn settle(&mut self) -> BTreeMap<Id, LastResult> {
        let plan = self.solve();
        self.commit(plan)
    }

    /// 只读求解，不修改世界。返回每条受理意图的结局。
    fn solve(&self) -> BTreeMap<Id, LastResult> {
        let mut results = BTreeMap::new();
        // 受理时已按机器人记录方向（结果与提交顺序无关）。
        let dirs: BTreeMap<Id, (i32, i32)> = self
            .intents
            .iter()
            .map(|i| (i.robot_id, (i.dx, i.dy)))
            .collect();
        let arg_of = |robot: Id| dir_name(dirs[&robot].0, dirs[&robot].1).to_string();
        // 第 1 段等价物：意图已在受理时收集（与提交顺序无关的集合）。
        // 重新校验：主体存在、目标在最终管理状态上仍可通行。
        #[derive(Clone, Copy)]
        struct Move {
            robot: Id,
            from: Position,
            to: Position,
        }
        let mut valid: Vec<Move> = Vec::new();
        for intent in &self.intents {
            let Some(robot) = self.robots.get(&intent.robot_id) else {
                results.insert(
                    intent.robot_id,
                    LastResult {
                        action: "move",
                        arg: arg_of(intent.robot_id),
                        code: codes::TARGET_GONE.to_string(),
                    },
                );
                continue;
            };
            let to = robot.pos.step(intent.dx, intent.dy);
            let code = if to.x < 0 || to.y < 0 || to.x >= self.map_w || to.y >= self.map_h {
                codes::OUT_OF_BOUNDS
            } else if !self.statically_passable(to) {
                codes::CELL_BLOCKED
            } else {
                codes::OK
            };
            if code == codes::OK {
                valid.push(Move {
                    robot: robot.id,
                    from: robot.pos,
                    to,
                });
            } else {
                results.insert(
                    intent.robot_id,
                    LastResult {
                        action: "move",
                        arg: arg_of(intent.robot_id),
                        code: code.to_string(),
                    },
                );
            }
        }

        // 第 2 段：落点候选（同格争抢，id 小者胜，不递补）。
        let mut by_target: BTreeMap<Position, Vec<Move>> = BTreeMap::new();
        for m in &valid {
            by_target.entry(m.to).or_default().push(*m);
        }
        let mut winners: Vec<Move> = Vec::new();
        for (_cell, mut cands) in by_target {
            cands.sort_by_key(|m| m.robot);
            let winner = cands.remove(0);
            for loser in cands {
                results.insert(
                    loser.robot,
                    LastResult {
                        action: "move",
                        arg: arg_of(loser.robot),
                        code: codes::CELL_CONTESTED.to_string(),
                    },
                );
            }
            winners.push(winner);
        }
        winners.sort_by_key(|m| m.robot);

        // 移动依赖求解（只读推演）：
        // 成功条件 = 目标格 tick 开始无机器人，或该机器人的（候选）移动成功离开。
        // 二元交换是长度 2 的依赖环 → 失败 CHAIN_BLOCKED；长度 ≥3 的环允许。
        // 迭代传播失败直到不动点（候选失败不递补）。
        let mut success: BTreeMap<Id, bool> = winners.iter().map(|m| (m.robot, true)).collect();
        loop {
            let mut changed = false;
            for m in &winners {
                if !success[&m.robot] {
                    continue;
                }
                let blocked = match self.robot_at(m.to) {
                    None => false,
                    Some(occ) => {
                        if occ == m.robot {
                            false // 不可能：相邻移动
                        } else {
                            !*success.get(&occ).unwrap_or(&false)
                        }
                    }
                };
                if blocked {
                    success.insert(m.robot, false);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // 二元交换：A↔B 互相依赖，迭代会把两者都置 false？
        // 不会——初值均为 true 时 A 依赖 B 成功、B 依赖 A 成功，不动点保持
        // true。因此显式检测长度恰为 2 的环（互相占据对方出发点）。
        for i in 0..winners.len() {
            for j in (i + 1)..winners.len() {
                let (a, b) = (&winners[i], &winners[j]);
                if a.from == b.to && b.from == a.to {
                    success.insert(a.robot, false);
                    success.insert(b.robot, false);
                }
            }
        }

        for m in &winners {
            let code = if success[&m.robot] {
                codes::OK
            } else {
                codes::CHAIN_BLOCKED
            };
            results.insert(
                m.robot,
                LastResult {
                    action: "move",
                    arg: arg_of(m.robot),
                    code: code.to_string(),
                },
            );
        }
        results
    }

    /// 第 3 段：原子提交。移动类 A0 无车辆离场 / 收款副作用。
    fn commit(&mut self, results: BTreeMap<Id, LastResult>) -> BTreeMap<Id, LastResult> {
        for (robot_id, res) in &results {
            if res.code == codes::OK {
                let intent = self
                    .intents
                    .iter()
                    .find(|i| i.robot_id == *robot_id)
                    .expect("结果必来自已受理意图");
                let (dx, dy) = (intent.dx, intent.dy);
                if let Some(robot) = self.robots.get_mut(robot_id) {
                    robot.pos = robot.pos.step(dx, dy);
                }
            }
        }
        self.intents.clear();
        self.last_results = results.clone();
        results
    }

    /// 结算后收尾（阶段 5 的 A0 最小版）：tick +1。计息 / 渲染快照不在 A0。
    pub fn end_tick(&mut self) {
        self.tick += 1;
    }

    /// 确定性回归用的世界摘要（排列测试对比）。
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mix = |v: u64, h: &mut u64| {
            *h ^= v;
            *h = h.wrapping_mul(0x100000001b3);
        };
        mix(self.tick, &mut h);
        mix(self.gold_milli as u64, &mut h);
        for (id, r) in &self.robots {
            mix(*id, &mut h);
            mix(r.pos.x as u64, &mut h);
            mix(r.pos.y as u64, &mut h);
            mix(r.energy as u64, &mut h);
        }
        for (id, res) in &self.last_results {
            mix(*id, &mut h);
            for b in res.code.bytes() {
                mix(b as u64, &mut h);
            }
            for b in res.action.as_bytes() {
                mix(*b as u64, &mut h);
            }
            for b in res.arg.as_bytes() {
                mix(*b as u64, &mut h);
            }
        }
        for id in self.my_orders.keys() {
            mix(*id, &mut h);
        }
        h
    }
}

/// take 的镜像同步增量素材（api 层据此构造宿主回放增量）。
#[derive(Debug, Clone)]
pub struct TakeEffect {
    pub order_id: Id,
    pub port_id: Id,
    pub order: Order,
}

fn dir_name(dx: i32, dy: i32) -> &'static str {
    match (dx, dy) {
        (0, -1) => "NORTH",
        (0, 1) => "SOUTH",
        (-1, 0) => "WEST",
        (1, 0) => "EAST",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_world() -> World {
        let mut w = World::new_empty(10, 10, 100_000);
        w.add_robot(Position::new(1, 1));
        w
    }

    #[test]
    fn accept_static_checks() {
        let mut w = demo_world(); // 机器人 1 在 (1,1)
        let edge = w.add_robot(Position::new(0, 0));
        assert_eq!(w.accept_move(1, 2, 0), codes::INVALID_ARGUMENT);
        assert_eq!(w.accept_move(99, 1, 0), codes::NO_SUCH_OBJECT);
        assert_eq!(w.accept_move(edge, 0, -1), codes::OUT_OF_BOUNDS);
        assert_eq!(w.accept_move(edge, -1, 0), codes::OUT_OF_BOUNDS);
        w.add_wall(Position::new(2, 1));
        assert_eq!(w.accept_move(1, 1, 0), codes::CELL_BLOCKED);
        // 受理失败不消耗行动机会：EAST 被墙挡后，改提 SOUTH 可受理通过。
        assert_eq!(w.accept_move(1, 0, 1), codes::OK);
        // 受理通过即占用本 tick 行动机会，重复提交不静默覆盖。
        assert_eq!(w.accept_move(1, 0, 1), codes::ALREADY_ACTED);
        assert_eq!(w.accept_move(1, 1, 0), codes::ALREADY_ACTED);
    }

    #[test]
    fn simple_move_settles() {
        let mut w = demo_world();
        assert_eq!(w.accept_move(1, 1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&1].code, codes::OK);
        assert_eq!(w.robots[&1].pos, Position::new(2, 1));
        w.end_tick();
        assert_eq!(w.tick, 1);
        // 无受理动作 → last_result 为 None 语义（该机器人无记录）。
        let res = w.settle();
        assert!(!res.contains_key(&1));
    }

    #[test]
    fn contested_cell_min_id_wins_no_promotion() {
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1)); // id 小
        let b = w.add_robot(Position::new(3, 1));
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&a].code, codes::OK);
        assert_eq!(res[&b].code, codes::CELL_CONTESTED);
        assert_eq!(w.robots[&a].pos, Position::new(2, 1));
        assert_eq!(w.robots[&b].pos, Position::new(3, 1));
    }

    #[test]
    fn swap_fails_ring_succeeds() {
        // 二元交换：不允许。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&b].code, codes::CHAIN_BLOCKED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        assert_eq!(w.robots[&b].pos, Position::new(2, 1));

        // 网格上最小的“圈”是四格方形轮转（≥3 个机器人允许）。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        let c = w.add_robot(Position::new(2, 2));
        let d = w.add_robot(Position::new(1, 2));
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, 0, 1), codes::OK);
        assert_eq!(w.accept_move(c, -1, 0), codes::OK);
        assert_eq!(w.accept_move(d, 0, -1), codes::OK);
        let res = w.settle();
        for id in [a, b, c, d] {
            assert_eq!(res[&id].code, codes::OK, "robot {id}");
        }
        assert_eq!(w.robots[&a].pos, Position::new(2, 1));
        assert_eq!(w.robots[&b].pos, Position::new(2, 2));
        assert_eq!(w.robots[&c].pos, Position::new(1, 2));
        assert_eq!(w.robots[&d].pos, Position::new(1, 1));
    }

    #[test]
    fn chain_blocked_propagates() {
        // c 静止占格，b 试图进 c 的格，a 跟在 b 后面：b、a 均 CHAIN_BLOCKED。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        let c = w.add_robot(Position::new(3, 1));
        assert_eq!(w.accept_move(b, 1, 0), codes::OK);
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&b].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        let _ = c;
    }

    #[test]
    fn follow_into_vacated_cell() {
        // 前方机器人成功离开 → 后续允许跟进（目标机器人本 tick 成功离开）。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(2, 1));
        assert_eq!(w.accept_move(b, 1, 0), codes::OK); // b → (3,1)
        assert_eq!(w.accept_move(a, 1, 0), codes::OK); // a → (2,1)（b 离开）
        let res = w.settle();
        assert_eq!(res[&b].code, codes::OK);
        assert_eq!(res[&a].code, codes::OK);
        assert_eq!(w.robots[&a].pos, Position::new(2, 1));
        assert_eq!(w.robots[&b].pos, Position::new(3, 1));
    }

    #[test]
    fn take_order_flow() {
        let mut w = World::new_empty(10, 10, 1_000_000);
        let port = w.add_port(Position::new(0, 5));
        let o1 = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
        let o2 = w.add_listing(OrderSide::Buy, "chip", 1, 7000);
        let (code, eff) = w.manage_take(o1);
        assert_eq!(code, codes::OK);
        let eff = eff.unwrap();
        assert_eq!(eff.port_id, port);
        assert_eq!(w.gold_milli, 1_000_000 - 2 * 5000);
        assert!(w.listings.contains_key(&o2) && !w.listings.contains_key(&o1));
        assert_eq!(w.my_orders[&o1].port, Some(port));
        // 车辆下一 tick 边界到场。
        w.end_tick();
        w.boundary_events();
        assert_eq!(w.vehicles.len(), 1);
        let v = w.vehicles.values().next().unwrap();
        assert_eq!(v.kind, VehicleKind::In);
        assert_eq!(v.box_ids.len(), 2);
        assert_eq!(w.ports[&port].docked_vehicle, Some(v.id));
        assert_eq!(w.my_orders[&o1].vehicle, Some(v.id));
    }

    #[test]
    fn take_rejects() {
        let mut w = World::new_empty(10, 10, 100);
        w.add_port(Position::new(0, 5));
        let o = w.add_listing(OrderSide::Sell, "battery", 2, 5000);
        assert_eq!(w.manage_take(o).0, codes::NO_FUNDS);
        let mut w2 = World::new_empty(10, 10, 1_000_000);
        let o2 = w2.add_listing(OrderSide::Sell, "battery", 1, 100);
        assert_eq!(w2.manage_take(o2).0, codes::NO_FREE_PORT);
        assert_eq!(w.manage_take(999).0, codes::ORDER_GONE);
    }

    #[test]
    fn no_promotion_when_winner_dependency_fails() {
        // c 静止占格；a、b 争 c 的格：a（id 小）取得候选但因 c 不离开而
        // CHAIN_BLOCKED；b 保持 CELL_CONTESTED——候选失败不递补。
        let mut w = World::new_empty(10, 10, 0);
        let a = w.add_robot(Position::new(1, 1));
        let b = w.add_robot(Position::new(3, 1));
        let c = w.add_robot(Position::new(2, 1)); // id 最大却占着目标格
        assert_eq!(w.accept_move(a, 1, 0), codes::OK);
        assert_eq!(w.accept_move(b, -1, 0), codes::OK);
        let res = w.settle();
        assert_eq!(res[&a].code, codes::CHAIN_BLOCKED);
        assert_eq!(res[&b].code, codes::CELL_CONTESTED);
        assert_eq!(w.robots[&a].pos, Position::new(1, 1));
        assert_eq!(w.robots[&b].pos, Position::new(3, 1));
        assert_eq!(w.robots[&c].pos, Position::new(2, 1));
    }

    /// 受理集合与结算结果与提交顺序无关：≤5 台全排列（docs/architecture/08
    /// 原型 B 回归的 move 子集，A0 先行覆盖）。
    #[test]
    fn settlement_permutation_invariant() {
        use ztw_model::EAST;
        // 布局：5 台机器人，两两争抢两个格、含一条链与一个静止占位者。
        let build = |w: &mut World| -> Vec<Id> {
            let mut ids = Vec::new();
            for (x, y) in [(1, 1), (3, 1), (5, 1), (2, 1), (4, 1)] {
                ids.push(w.add_robot(Position::new(x, y)));
            }
            ids
        };
        let dirs = [EAST, (0, 1), (-1, 0), EAST, (0, 1)];
        let accepts = |w: &mut World, ids: &Vec<Id>| -> Vec<&'static str> {
            ids.iter()
                .zip(dirs)
                .map(|(id, d)| w.accept_move(*id, d.0, d.1))
                .collect()
        };
        let baseline = {
            let mut w = World::new_empty(10, 10, 0);
            let ids = build(&mut w);
            let codes0 = accepts(&mut w, &ids);
            w.settle();
            (w.state_hash(), w.robots.clone(), codes0)
        };
        let mut perm = [0usize, 1, 2, 3, 4];
        loop {
            let mut w = World::new_empty(10, 10, 0);
            let ids = build(&mut w);
            let mut codes_p = vec![ztw_model::codes::OK; 5];
            for &i in &perm {
                let d = dirs[i];
                codes_p[i] = w.accept_move(ids[i], d.0, d.1);
            }
            w.settle();
            assert_eq!(w.state_hash(), baseline.0, "perm {perm:?}");
            assert_eq!(w.robots, baseline.1);
            assert_eq!(codes_p, baseline.2, "受理集合也必须一致：perm {perm:?}");
            if !next_perm(&mut perm) {
                break;
            }
        }
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
}
