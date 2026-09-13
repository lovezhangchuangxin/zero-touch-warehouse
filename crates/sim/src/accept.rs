//! 动作受理（静态检查）：基于调用时世界状态 = tick 开始快照 + 已生效管理
//! 操作（docs/game-design/03：只判静态条件，同 tick 冲突一律留给结算）。

use ztw_model::{Id, Position, codes};

use crate::intent::{Intent, TargetRef};
use crate::world::World;
use crate::{ENERGY_COST_DROP, ENERGY_COST_GIVE, ENERGY_COST_PICK, ENERGY_COST_TAKE};

impl World {
    /// move 受理：只判定调用时可知的静态条件。目标格争抢、链受阻等
    /// 依赖同 tick 其他意图的冲突一律留给结算。
    pub fn accept_move(&mut self, robot_id: Id, dx: i32, dy: i32) -> &'static str {
        if !(dx == 0 || dy == 0) || dx.abs() > 1 || dy.abs() > 1 || (dx == 0 && dy == 0) {
            return codes::INVALID_ARGUMENT;
        }
        let pos = match self.actor(robot_id) {
            Ok(r) => r.pos,
            Err(code) => return code,
        };
        let target = pos.step(dx, dy);
        if target.x < 0 || target.y < 0 || target.x >= self.map_w || target.y >= self.map_h {
            return codes::OUT_OF_BOUNDS;
        }
        if !self.statically_passable(target) {
            return codes::CELL_BLOCKED;
        }
        self.intents.push(Intent::Move { robot_id, dx, dy });
        codes::OK
    }

    /// charge 受理：无参动作——受理时按相邻 id 最小桩固定目标，之后不换桩
    /// （docs/game-design/03 电力与充电）。同 tick 竞争留给结算（CHARGER_BUSY）。
    pub fn accept_charge(&mut self, robot_id: Id) -> &'static str {
        let pos = match self.actor(robot_id) {
            Ok(r) => r.pos,
            Err(code) => return code,
        };
        let Some(charger_id) = self
            .chargers
            .values()
            .filter(|c| c.pos.adjacent(&pos))
            .map(|c| c.id)
            .min()
        else {
            return codes::NOT_ADJACENT;
        };
        self.intents.push(Intent::Charge {
            robot_id,
            charger_id,
        });
        codes::OK
    }

    /// take 受理：从相邻容器取得指定货物；自身须空载。
    pub fn accept_take(&mut self, robot_id: Id, target_id: Id, box_id: Id) -> &'static str {
        let (pos, energy, loaded) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry.is_some()),
            Err(code) => return code,
        };
        if loaded {
            return codes::LOADED;
        }
        let target = match self.resolve_target(target_id) {
            Ok(t) => t,
            Err(code) => return code,
        };
        if !self.target_pos(target).adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        if !self.target_holds(target, box_id) {
            return codes::BOX_NOT_FOUND;
        }
        if energy < ENERGY_COST_TAKE {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Take {
            robot_id,
            target,
            box_id,
        });
        codes::OK
    }

    /// give 受理：将携带物放入相邻容器（货架 / 车辆 / 空载机器人）。
    /// box_id 缺省为当前携带物（为多箱预留）。同 tick 容量争抢留给结算。
    pub fn accept_give(&mut self, robot_id: Id, target_id: Id, box_id: Option<Id>) -> &'static str {
        let (pos, energy, carry) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry),
            Err(code) => return code,
        };
        let Some(carried) = carry else {
            return codes::NOT_CARRYING;
        };
        let box_id = box_id.unwrap_or(carried);
        if carry != Some(box_id) {
            return codes::BOX_NOT_FOUND; // 指定货物不在自身携带
        }
        let target = match self.resolve_target(target_id) {
            Ok(t) => t,
            Err(code) => return code,
        };
        if !self.target_pos(target).adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        match target {
            TargetRef::Shelf(shelf_id) => {
                let s = &self.shelves[&shelf_id];
                if s.box_ids.len() >= s.capacity {
                    return codes::TARGET_FULL;
                }
            }
            TargetRef::Vehicle(vehicle_id) => {
                let v = &self.vehicles[&vehicle_id];
                if self.ground_boxes[&box_id].goods_type != v.goods_type {
                    return codes::WRONG_GOODS;
                }
                if v.box_ids.len() >= self.vehicle_capacity(vehicle_id) {
                    return codes::TARGET_FULL;
                }
            }
            TargetRef::Robot(target_robot) => {
                if self.robots[&target_robot].carry.is_some() {
                    return codes::TARGET_FULL; // 接收方非空载
                }
            }
        }
        if energy < ENERGY_COST_GIVE {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Give {
            robot_id,
            target,
            box_id,
        });
        codes::OK
    }

    /// pick 受理：拾起相邻地面格的货物；自身须空载。
    pub fn accept_pick(&mut self, robot_id: Id, x: i32, y: i32) -> &'static str {
        let (pos, energy, loaded) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry.is_some()),
            Err(code) => return code,
        };
        if loaded {
            return codes::LOADED;
        }
        let cell = Position::new(x, y);
        if cell.x < 0 || cell.y < 0 || cell.x >= self.map_w || cell.y >= self.map_h {
            return codes::OUT_OF_BOUNDS;
        }
        if !cell.adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        let Some(box_id) = self.ground_box_at(cell) else {
            return codes::BOX_NOT_FOUND;
        };
        if energy < ENERGY_COST_PICK {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Pick {
            robot_id,
            x,
            y,
            box_id,
        });
        codes::OK
    }

    /// drop 受理：将携带物放到相邻空地面格。目标格被机器人占用不在受理
    /// 判定：能否放下取决于其本 tick 是否成功离开（结算判 CELL_OCCUPIED）。
    pub fn accept_drop(
        &mut self,
        robot_id: Id,
        x: i32,
        y: i32,
        box_id: Option<Id>,
    ) -> &'static str {
        let (pos, energy, carry) = match self.actor(robot_id) {
            Ok(r) => (r.pos, r.energy, r.carry),
            Err(code) => return code,
        };
        let Some(carried) = carry else {
            return codes::NOT_CARRYING;
        };
        let box_id = box_id.unwrap_or(carried);
        if carry != Some(box_id) {
            return codes::BOX_NOT_FOUND;
        }
        let cell = Position::new(x, y);
        if cell.x < 0 || cell.y < 0 || cell.x >= self.map_w || cell.y >= self.map_h {
            return codes::OUT_OF_BOUNDS;
        }
        if !cell.adjacent(&pos) {
            return codes::NOT_ADJACENT;
        }
        // 地面货物占格优先报 CELL_OCCUPIED（docs/game-design/08 码表），
        // 其余静态障碍（墙 / 货架 / 桩 / 口）报 CELL_BLOCKED。
        if self.ground_box_at(cell).is_some() {
            return codes::CELL_OCCUPIED;
        }
        if !self.statically_passable(cell) {
            return codes::CELL_BLOCKED;
        }
        if energy < ENERGY_COST_DROP {
            return codes::NOT_ENOUGH_ENERGY;
        }
        self.intents.push(Intent::Drop {
            robot_id,
            x,
            y,
            box_id,
        });
        codes::OK
    }
}
