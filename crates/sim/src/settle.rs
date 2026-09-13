//! 阶段 4：统一结算（受理 → 只读求解 → 原子提交；docs/architecture/02）。
//! 求解按 docs/game-design/03「结算资源与执行顺序」固化为五个阶段：
//! ① 管理终态重校验 → ② move/drop 联合落点候选与移动依赖 → ③ 转交目标
//! 移动判定 → ④ 剩余动作按机器人 id 升序原子占用资源 → ⑤ 车辆离场。

use std::collections::{BTreeMap, BTreeSet};

use ztw_model::{Id, MilliGold, OrderSide, Position, VehicleKind, codes};

use crate::intent::{Intent, LastResult, TargetRef};
use crate::world::World;
use crate::{
    CHARGE_PER_TICK, ENERGY_COST_DROP, ENERGY_COST_GIVE, ENERGY_COST_PICK, ENERGY_COST_TAKE,
};

/// 结算计划：只读求解的产物、原子提交的输入（docs/architecture/02）。
#[derive(Debug, Clone, Default)]
pub struct SettlementPlan {
    pub results: BTreeMap<Id, LastResult>,
    /// 成功移动：机器人 → 目标格。
    pub moves: BTreeMap<Id, Position>,
    /// 电量变化（成功充电为正、成功动作耗电为负）。
    pub energy: BTreeMap<Id, i32>,
    /// 货物转移：(箱, 新容器, 新位置)；每箱每 tick 至多一条。
    pub box_moves: Vec<(Id, Option<Id>, Position)>,
    /// 车辆离场（含订单完成与收款、装卸位释放所需信息）。
    pub departures: Vec<Departure>,
}

/// 一辆车的离场：离场 → 订单完成与收款 → 装卸位释放（次序固化）。
#[derive(Debug, Clone)]
pub struct Departure {
    pub vehicle_id: Id,
    pub order_id: Id,
    pub port_id: Id,
    /// 买单（玩家卖出）装满离场的收款；卖单离场无收款。
    pub revenue_milli: MilliGold,
}

/// 重校验后的存活动作分拣（阶段间的传递载体）。
#[derive(Default)]
struct Candidates {
    moves: Vec<MoveCand>,
    drops: Vec<DropCand>,
    /// 资源裁决阶段的动作（drop 落点胜出者并入）。
    res_acts: Vec<Res>,
}

#[derive(Clone, Copy)]
struct MoveCand {
    robot: Id,
    from: Position,
    to: Position,
}

#[derive(Clone, Copy)]
struct DropCand {
    robot: Id,
    cell: Position,
    box_id: Id,
}

enum Res {
    Charge {
        robot: Id,
        charger: Id,
    },
    Take {
        robot: Id,
        target: TargetRef,
        box_id: Id,
    },
    Give {
        robot: Id,
        target: TargetRef,
        box_id: Id,
    },
    Pick {
        robot: Id,
        cell: Position,
        box_id: Id,
    },
    Drop {
        robot: Id,
        cell: Position,
        box_id: Id,
    },
}

impl Res {
    fn robot(&self) -> Id {
        match self {
            Res::Charge { robot, .. }
            | Res::Take { robot, .. }
            | Res::Give { robot, .. }
            | Res::Pick { robot, .. }
            | Res::Drop { robot, .. } => *robot,
        }
    }
}

impl World {
    /// 结算本 tick 已受理意图。求解是「最终管理状态 + 意图集合 → 计划」的
    /// 纯函数：任意受理顺序得到同一计划（排列测试覆盖）。
    pub fn settle(&mut self) -> BTreeMap<Id, LastResult> {
        let plan = self.solve();
        self.commit(plan)
    }

    /// 只读求解，产出完整结算计划。电量在受理时按快照校验且结算前不变，
    /// 无需重查（「不预支同 tick 充入电量」由此成立）。
    fn solve(&self) -> SettlementPlan {
        let mut plan = SettlementPlan::default();
        let mut cands = Candidates::default();
        self.revalidate(&mut plan, &mut cands);
        let move_ok = self.resolve_landing(&mut plan, &mut cands);
        let vehicle_fill = self.allocate_resources(&mut plan, &mut cands, &move_ok);
        self.collect_departures(&mut plan, &vehicle_fill);
        plan
    }

    /// ① 管理终态重校验：主体 / 目标被删或静态条件失效的意图在此出局，
    /// 存活者按类型分拣。
    fn revalidate(&self, plan: &mut SettlementPlan, cands: &mut Candidates) {
        for intent in &self.intents {
            let robot_id = intent.robot_id();
            let Some(robot) = self.robots.get(&robot_id) else {
                // 主体被管理操作删除：结果保留在诊断记录（镜像不展示）。
                let (action, arg) = intent_label(intent);
                record(plan, robot_id, action, arg, codes::TARGET_GONE);
                continue;
            };
            match intent {
                Intent::Move { dx, dy, .. } => {
                    let to = robot.pos.step(*dx, *dy);
                    let code = if to.x < 0 || to.y < 0 || to.x >= self.map_w || to.y >= self.map_h {
                        codes::OUT_OF_BOUNDS
                    } else if !self.statically_passable(to) {
                        codes::CELL_BLOCKED
                    } else {
                        cands.moves.push(MoveCand {
                            robot: robot_id,
                            from: robot.pos,
                            to,
                        });
                        codes::OK
                    };
                    if code != codes::OK {
                        let (action, arg) = intent_label(intent);
                        record(plan, robot_id, action, arg, code);
                    }
                }
                Intent::Charge { charger_id, .. } => {
                    if !self.chargers.contains_key(charger_id) {
                        let (action, arg) = intent_label(intent);
                        record(plan, robot_id, action, arg, codes::TARGET_GONE);
                    } else {
                        cands.res_acts.push(Res::Charge {
                            robot: robot_id,
                            charger: *charger_id,
                        });
                    }
                }
                Intent::Take { target, box_id, .. } => {
                    // 先判目标存活再查持有，两段判定与 give 分支一致：
                    // target_holds 对已删目标直接索引，不依赖「销毁必空、
                    // cancel 必恢复」的跨操作不变量。
                    let alive = self.target_alive(*target);
                    let holds = alive
                        && self.ground_boxes.contains_key(box_id)
                        && self.target_holds(*target, *box_id);
                    if holds {
                        cands.res_acts.push(Res::Take {
                            robot: robot_id,
                            target: *target,
                            box_id: *box_id,
                        });
                    } else {
                        let code = if !alive {
                            codes::TARGET_GONE
                        } else {
                            codes::BOX_NOT_FOUND
                        };
                        let (action, arg) = intent_label(intent);
                        record(plan, robot_id, action, arg, code);
                    }
                }
                Intent::Give { target, box_id, .. } => {
                    let carrying =
                        robot.carry == Some(*box_id) && self.ground_boxes.contains_key(box_id);
                    if carrying && self.target_alive(*target) {
                        cands.res_acts.push(Res::Give {
                            robot: robot_id,
                            target: *target,
                            box_id: *box_id,
                        });
                    } else {
                        let code = if !carrying {
                            codes::BOX_NOT_FOUND
                        } else {
                            codes::TARGET_GONE
                        };
                        let (action, arg) = intent_label(intent);
                        record(plan, robot_id, action, arg, code);
                    }
                }
                Intent::Pick { x, y, box_id, .. } => {
                    let still = self
                        .ground_boxes
                        .get(box_id)
                        .is_some_and(|b| b.holder.is_none() && b.pos == Position::new(*x, *y));
                    if still {
                        cands.res_acts.push(Res::Pick {
                            robot: robot_id,
                            cell: Position::new(*x, *y),
                            box_id: *box_id,
                        });
                    } else {
                        let (action, arg) = intent_label(intent);
                        record(plan, robot_id, action, arg, codes::BOX_NOT_FOUND);
                    }
                }
                Intent::Drop { x, y, box_id, .. } => {
                    let cell = Position::new(*x, *y);
                    let carrying =
                        robot.carry == Some(*box_id) && self.ground_boxes.contains_key(box_id);
                    let code = if !carrying {
                        codes::BOX_NOT_FOUND
                    } else if cell.x < 0
                        || cell.y < 0
                        || cell.x >= self.map_w
                        || cell.y >= self.map_h
                    {
                        codes::OUT_OF_BOUNDS
                    } else if self.ground_box_at(cell).is_some() {
                        codes::CELL_OCCUPIED
                    } else if !self.statically_passable(cell) {
                        codes::CELL_BLOCKED
                    } else {
                        cands.drops.push(DropCand {
                            robot: robot_id,
                            cell,
                            box_id: *box_id,
                        });
                        codes::OK
                    };
                    if code != codes::OK {
                        let (action, arg) = intent_label(intent);
                        record(plan, robot_id, action, arg, code);
                    }
                }
            }
        }
    }

    /// ② move 与 drop 的联合落点候选：同格争抢，机器人 id 小者胜，
    /// 候选失败不递补（败者 CELL_CONTESTED）。随后求解移动依赖并裁定
    /// drop 落点，返回各移动候选的成败表（供阶段 ③ 判 TARGET_MOVED 与
    /// drop 依赖使用）。
    fn resolve_landing(
        &self,
        plan: &mut SettlementPlan,
        cands: &mut Candidates,
    ) -> BTreeMap<Id, bool> {
        #[derive(Clone, Copy)]
        enum Landing {
            M(MoveCand),
            D(DropCand),
        }
        let mut by_cell: BTreeMap<Position, Vec<Landing>> = BTreeMap::new();
        for m in std::mem::take(&mut cands.moves) {
            by_cell.entry(m.to).or_default().push(Landing::M(m));
        }
        for d in std::mem::take(&mut cands.drops) {
            by_cell.entry(d.cell).or_default().push(Landing::D(d));
        }
        let mut move_winners: Vec<MoveCand> = Vec::new();
        let mut drop_winners: Vec<DropCand> = Vec::new();
        for (_cell, mut cands_) in by_cell {
            cands_.sort_by_key(|l| match l {
                Landing::M(m) => m.robot,
                Landing::D(d) => d.robot,
            });
            match cands_.remove(0) {
                Landing::M(m) => move_winners.push(m),
                Landing::D(d) => drop_winners.push(d),
            }
            for loser in cands_ {
                match loser {
                    Landing::M(m) => record(
                        plan,
                        m.robot,
                        "move",
                        dir_name(m.to.x - m.from.x, m.to.y - m.from.y).to_string(),
                        codes::CELL_CONTESTED,
                    ),
                    Landing::D(d) => record(
                        plan,
                        d.robot,
                        "drop",
                        format!("{},{}", d.cell.x, d.cell.y),
                        codes::CELL_CONTESTED,
                    ),
                }
            }
        }
        move_winners.sort_by_key(|m| m.robot);
        drop_winners.sort_by_key(|d| d.robot);

        // 移动依赖求解（只读推演）：成功 = 目标格 tick 开始无机器人，或占位
        // 者的候选移动成功离开。迭代传播失败直到不动点（候选失败不递补）。
        let mut move_ok: BTreeMap<Id, bool> =
            move_winners.iter().map(|m| (m.robot, true)).collect();
        // 二元交换先判负（长度 2 的依赖环 → CHAIN_BLOCKED；长度 ≥3 的环允许），
        // 再进入不动点迭代——交换失败沿链统一传播，正确性不依赖落点竞争
        // 排他性的非局部论证。不动点初值全 true 时互相依赖不会自行传播，
        // 故必须在迭代前显式判负。
        for i in 0..move_winners.len() {
            for j in (i + 1)..move_winners.len() {
                let (a, b) = (&move_winners[i], &move_winners[j]);
                if a.from == b.to && b.from == a.to {
                    move_ok.insert(a.robot, false);
                    move_ok.insert(b.robot, false);
                }
            }
        }
        loop {
            let mut changed = false;
            for m in &move_winners {
                if !move_ok[&m.robot] {
                    continue;
                }
                let blocked = match self.robot_at(m.to) {
                    None => false,
                    Some(occ) => occ != m.robot && !move_ok.get(&occ).copied().unwrap_or(false),
                };
                if blocked {
                    move_ok.insert(m.robot, false);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for m in &move_winners {
            let arg = dir_name(m.to.x - m.from.x, m.to.y - m.from.y).to_string();
            if move_ok[&m.robot] {
                plan.moves.insert(m.robot, m.to);
                record(plan, m.robot, "move", arg, codes::OK);
            } else {
                record(plan, m.robot, "move", arg, codes::CHAIN_BLOCKED);
            }
        }
        // drop 依赖：落点被机器人占用时，须其成功离开；否则 CELL_OCCUPIED。
        // 胜出的 drop 并入资源裁决（其箱转移仍可能被更低 id 的主动取走截胡）。
        for d in &drop_winners {
            let ok = match self.robot_at(d.cell) {
                None => true,
                Some(occ) => move_ok.get(&occ).copied().unwrap_or(false),
            };
            if ok {
                cands.res_acts.push(Res::Drop {
                    robot: d.robot,
                    cell: d.cell,
                    box_id: d.box_id,
                });
            } else {
                record(
                    plan,
                    d.robot,
                    "drop",
                    format!("{},{}", d.cell.x, d.cell.y),
                    codes::CELL_OCCUPIED,
                );
            }
        }
        move_ok
    }

    /// ③ + ④ 剩余动作按机器人 id 升序原子占用资源。草稿状态 = 结算前
    /// 世界 + 本 tick 已裁决转移；失败动作无部分效果；同一箱每 tick 至多
    /// 转移一次，不允许在不同动作间接力。返回草稿车辆装载量（阶段 ⑤ 用）。
    fn allocate_resources(
        &self,
        plan: &mut SettlementPlan,
        cands: &mut Candidates,
        move_ok: &BTreeMap<Id, bool>,
    ) -> BTreeMap<Id, usize> {
        let mut res_acts = std::mem::take(&mut cands.res_acts);
        res_acts.sort_by_key(|a| a.robot());
        let mut box_holder: BTreeMap<Id, Option<Id>> = self
            .ground_boxes
            .iter()
            .map(|(id, b)| (*id, b.holder))
            .collect();
        let mut carry: BTreeMap<Id, Option<Id>> =
            self.robots.iter().map(|(id, r)| (*id, r.carry)).collect();
        let mut shelf_fill: BTreeMap<Id, usize> = self
            .shelves
            .iter()
            .map(|(id, s)| (*id, s.box_ids.len()))
            .collect();
        let mut vehicle_fill: BTreeMap<Id, usize> = self
            .vehicles
            .iter()
            .map(|(id, v)| (*id, v.box_ids.len()))
            .collect();
        let mut transferred: BTreeSet<Id> = BTreeSet::new();
        let mut charger_busy: BTreeSet<Id> = BTreeSet::new();

        for act in res_acts {
            match act {
                Res::Charge { robot, charger } => {
                    if !charger_busy.insert(charger) {
                        // 一桩每 tick 至多一台；同 tick 竞争由机器人 id 裁决。
                        record(
                            plan,
                            robot,
                            "charge",
                            charger.to_string(),
                            codes::CHARGER_BUSY,
                        );
                        continue;
                    }
                    let r = &self.robots[&robot];
                    let gain = CHARGE_PER_TICK.min(r.energy_max.saturating_sub(r.energy));
                    *plan.energy.entry(robot).or_insert(0) += gain as i32;
                    record(plan, robot, "charge", charger.to_string(), codes::OK);
                }
                Res::Take {
                    robot,
                    target,
                    box_id,
                } => {
                    // 转交目标机器人本 tick 成功移动 → 失败（先判移动后判资源）。
                    if let TargetRef::Robot(t) = target
                        && move_ok.get(&t).copied().unwrap_or(false)
                    {
                        record(
                            plan,
                            robot,
                            "take",
                            target.id().to_string(),
                            codes::TARGET_MOVED,
                        );
                        continue;
                    }
                    let contested = transferred.contains(&box_id)
                        || box_holder.get(&box_id).copied().flatten() != Some(target.id())
                        || carry[&robot].is_some(); // 被动接收已占单箱容量
                    if contested {
                        record(
                            plan,
                            robot,
                            "take",
                            target.id().to_string(),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, Some(robot));
                    carry.insert(robot, Some(box_id));
                    transferred.insert(box_id);
                    // 取出释放容器装载（同 tick 后续 give 可用该空位，按 id 序裁决）。
                    match target {
                        TargetRef::Shelf(shelf_id) => {
                            let f = shelf_fill.get_mut(&shelf_id).expect("已索引");
                            *f = f.saturating_sub(1);
                        }
                        TargetRef::Vehicle(vehicle_id) => {
                            let f = vehicle_fill.get_mut(&vehicle_id).expect("已索引");
                            *f = f.saturating_sub(1);
                        }
                        TargetRef::Robot(_) => {}
                    }
                    let pos = self.robots[&robot].pos;
                    plan.box_moves.push((box_id, Some(robot), pos));
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_TAKE as i32;
                    record(plan, robot, "take", target.id().to_string(), codes::OK);
                }
                Res::Give {
                    robot,
                    target,
                    box_id,
                } => {
                    if let TargetRef::Robot(t) = target
                        && move_ok.get(&t).copied().unwrap_or(false)
                    {
                        record(
                            plan,
                            robot,
                            "give",
                            target.id().to_string(),
                            codes::TARGET_MOVED,
                        );
                        continue;
                    }
                    if transferred.contains(&box_id) || carry[&robot] != Some(box_id) {
                        record(
                            plan,
                            robot,
                            "give",
                            target.id().to_string(),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    let placed = match target {
                        TargetRef::Shelf(shelf_id) => {
                            if shelf_fill[&shelf_id] < self.shelves[&shelf_id].capacity {
                                *shelf_fill.get_mut(&shelf_id).expect("已索引") += 1;
                                plan.box_moves.push((
                                    box_id,
                                    Some(shelf_id),
                                    self.shelves[&shelf_id].pos,
                                ));
                                true
                            } else {
                                false
                            }
                        }
                        TargetRef::Vehicle(vehicle_id) => {
                            if vehicle_fill[&vehicle_id] < self.vehicle_capacity(vehicle_id) {
                                *vehicle_fill.get_mut(&vehicle_id).expect("已索引") += 1;
                                plan.box_moves.push((
                                    box_id,
                                    Some(vehicle_id),
                                    self.vehicles[&vehicle_id].interact_pos,
                                ));
                                true
                            } else {
                                false
                            }
                        }
                        TargetRef::Robot(target_robot) => {
                            if carry[&target_robot].is_none() {
                                carry.insert(target_robot, Some(box_id));
                                plan.box_moves.push((
                                    box_id,
                                    Some(target_robot),
                                    self.robots[&target_robot].pos,
                                ));
                                true
                            } else {
                                false
                            }
                        }
                    };
                    if !placed {
                        record(
                            plan,
                            robot,
                            "give",
                            target.id().to_string(),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, Some(target.id()));
                    carry.insert(robot, None);
                    transferred.insert(box_id);
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_GIVE as i32;
                    record(plan, robot, "give", target.id().to_string(), codes::OK);
                }
                Res::Pick {
                    robot,
                    cell,
                    box_id,
                } => {
                    let contested = transferred.contains(&box_id)
                        || box_holder.get(&box_id).copied().flatten().is_some()
                        || carry[&robot].is_some();
                    if contested {
                        record(
                            plan,
                            robot,
                            "pick",
                            format!("{},{}", cell.x, cell.y),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, Some(robot));
                    carry.insert(robot, Some(box_id));
                    transferred.insert(box_id);
                    let pos = self.robots[&robot].pos;
                    plan.box_moves.push((box_id, Some(robot), pos));
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_PICK as i32;
                    record(
                        plan,
                        robot,
                        "pick",
                        format!("{},{}", cell.x, cell.y),
                        codes::OK,
                    );
                }
                Res::Drop {
                    robot,
                    cell,
                    box_id,
                } => {
                    if transferred.contains(&box_id) || carry[&robot] != Some(box_id) {
                        record(
                            plan,
                            robot,
                            "drop",
                            format!("{},{}", cell.x, cell.y),
                            codes::TARGET_CONTESTED,
                        );
                        continue;
                    }
                    box_holder.insert(box_id, None);
                    carry.insert(robot, None);
                    transferred.insert(box_id);
                    plan.box_moves.push((box_id, None, cell));
                    *plan.energy.entry(robot).or_insert(0) -= ENERGY_COST_DROP as i32;
                    record(
                        plan,
                        robot,
                        "drop",
                        format!("{},{}", cell.x, cell.y),
                        codes::OK,
                    );
                }
            }
        }
        vehicle_fill
    }

    /// ⑤ 车辆离场（按草稿装载量判定；车辆 id 升序）：入库车卸空、出库车
    /// 装满即离场。离场 → 订单完成与收款 → 装卸位释放的次序在 commit 固化。
    fn collect_departures(&self, plan: &mut SettlementPlan, vehicle_fill: &BTreeMap<Id, usize>) {
        for (vid, v) in &self.vehicles {
            let Some(order) = self.my_orders.get(&v.order_id) else {
                continue;
            };
            let fill = vehicle_fill.get(vid).copied().unwrap_or(v.box_ids.len());
            let depart = match v.kind {
                VehicleKind::In => fill == 0,
                VehicleKind::Out => fill as u32 >= order.qty,
            };
            if !depart {
                continue;
            }
            let revenue_milli = match order.side {
                // 买单（玩家卖出）装满离场时收款；卖单款项已付。
                OrderSide::Buy => (order.qty as i64).saturating_mul(order.unit_price_milli),
                OrderSide::Sell => 0,
            };
            plan.departures.push(Departure {
                vehicle_id: *vid,
                order_id: v.order_id,
                port_id: order.port.expect("在场车辆必有关联装卸口"),
                revenue_milli,
            });
        }
    }

    /// 第 3 段：原子提交。全部效果一次性应用，次序：移动 → 电量 → 货物
    /// 转移 → 车辆离场（离场 → 订单完成与收款 → 装卸位释放）。
    fn commit(&mut self, mut plan: SettlementPlan) -> BTreeMap<Id, LastResult> {
        for (robot_id, to) in &plan.moves {
            if let Some(r) = self.robots.get_mut(robot_id) {
                r.pos = *to;
            }
        }
        for (robot_id, delta) in &plan.energy {
            if let Some(r) = self.robots.get_mut(robot_id) {
                let e = r.energy as i64 + *delta as i64;
                r.energy = e.clamp(0, r.energy_max as i64) as u32;
            }
        }
        for (box_id, new_holder, new_pos) in &plan.box_moves {
            let Some(b) = self.ground_boxes.get_mut(box_id) else {
                continue;
            };
            // 清旧容器引用（id 跨类型唯一，命中恰一处）。
            if let Some(old) = b.holder {
                if let Some(r) = self.robots.get_mut(&old) {
                    if r.carry == Some(*box_id) {
                        r.carry = None;
                    }
                } else if let Some(s) = self.shelves.get_mut(&old) {
                    s.box_ids.retain(|x| x != box_id);
                } else if let Some(v) = self.vehicles.get_mut(&old) {
                    v.box_ids.retain(|x| x != box_id);
                }
            }
            // 立新容器引用。
            if let Some(h) = new_holder {
                if let Some(r) = self.robots.get_mut(h) {
                    r.carry = Some(*box_id);
                } else if let Some(s) = self.shelves.get_mut(h) {
                    s.box_ids.push(*box_id);
                } else if let Some(v) = self.vehicles.get_mut(h) {
                    v.box_ids.push(*box_id);
                }
            }
            b.holder = *new_holder;
            b.pos = *new_pos;
        }
        for dep in std::mem::take(&mut plan.departures) {
            // 车辆离场：连同车上货物退出世界。
            if let Some(v) = self.vehicles.remove(&dep.vehicle_id) {
                for b in &v.box_ids {
                    self.ground_boxes.remove(b);
                }
                // 订单完成与收款。
                self.my_orders.remove(&dep.order_id);
                self.gold_milli = self.gold_milli.saturating_add(dep.revenue_milli);
                // 装卸位释放。
                if let Some(p) = self.ports.get_mut(&dep.port_id)
                    && p.docked_vehicle == Some(v.id)
                {
                    p.docked_vehicle = None;
                }
            }
        }
        self.intents.clear();
        self.last_results = plan.results.clone();
        plan.results
    }
}

fn record(
    plan: &mut SettlementPlan,
    robot: Id,
    action: &'static str,
    arg: String,
    code: &'static str,
) {
    plan.results.insert(
        robot,
        LastResult {
            action,
            arg,
            code: code.to_string(),
        },
    );
}

fn intent_label(i: &Intent) -> (&'static str, String) {
    match i {
        Intent::Move { dx, dy, .. } => ("move", dir_name(*dx, *dy).to_string()),
        Intent::Charge { charger_id, .. } => ("charge", charger_id.to_string()),
        Intent::Take { target, .. } => ("take", target.id().to_string()),
        Intent::Give { target, .. } => ("give", target.id().to_string()),
        Intent::Pick { x, y, .. } => ("pick", format!("{x},{y}")),
        Intent::Drop { x, y, .. } => ("drop", format!("{x},{y}")),
    }
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
