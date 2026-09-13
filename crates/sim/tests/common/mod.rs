//! B1 回归套件公共设施：世界不变量断言器与直接构造状态的测试辅助。
//! 不变量对应 docs/architecture/08 原型 B 模拟回归第 7 条：
//! 每箱唯一归属、容器容量、地图占位、电量界内。

use ztw_model::{GroundBox, Id, Position};
use ztw_sim::{CHARGE_PER_TICK, ENERGY_COST_DROP, ENERGY_COST_PICK, World};

/// 结算后调用：任何测试路径都不得违反世界不变量。
#[allow(dead_code)]
pub fn check_invariants(w: &World) {
    use std::collections::BTreeSet;
    // 每箱唯一归属：holder 与恰好一个容器的引用互为镜像。
    let mut seen = BTreeSet::new();
    for (id, r) in &w.robots {
        if let Some(b) = r.carry {
            assert_eq!(
                w.ground_boxes.get(&b).and_then(|x| x.holder),
                Some(*id),
                "机器人 {id} 携带 {b}，但箱子归属不符"
            );
            assert!(seen.insert(b), "箱 {b} 被多重引用");
        }
        assert!(
            r.energy <= r.energy_max,
            "机器人 {id} 电量 {} 超上限 {}",
            r.energy,
            r.energy_max
        );
    }
    for (id, s) in &w.shelves {
        assert!(s.box_ids.len() <= s.capacity, "货架 {id} 超容量");
        for b in &s.box_ids {
            assert_eq!(
                w.ground_boxes.get(b).and_then(|x| x.holder),
                Some(*id),
                "货架 {id} 引用 {b}，但箱子归属不符"
            );
            assert!(seen.insert(*b), "箱 {b} 被多重引用");
        }
    }
    for (id, v) in &w.vehicles {
        let cap = w
            .my_orders
            .get(&v.order_id)
            .map(|o| o.qty as usize)
            .unwrap_or(v.box_ids.len());
        assert!(v.box_ids.len() <= cap, "车辆 {id} 超容量");
        for b in &v.box_ids {
            assert_eq!(
                w.ground_boxes.get(b).and_then(|x| x.holder),
                Some(*id),
                "车辆 {id} 引用 {b}，但箱子归属不符"
            );
            assert!(seen.insert(*b), "箱 {b} 被多重引用");
        }
    }
    for (id, b) in &w.ground_boxes {
        match b.holder {
            Some(_) => assert!(seen.contains(id), "箱 {id} 声称有容器，却无任何容器引用它"),
            None => assert!(seen.insert(*id), "地面箱 {id} 被多重引用"),
        }
    }
    // 地面占位：无主箱每格至多一箱；机器人每格至多一台。
    let mut cells = BTreeSet::new();
    for b in w.ground_boxes.values().filter(|b| b.holder.is_none()) {
        assert!(cells.insert(b.pos), "格 {:?} 有多箱无主货物", b.pos);
    }
    let mut robot_cells = BTreeSet::new();
    for r in w.robots.values() {
        assert!(robot_cells.insert(r.pos), "格 {:?} 有多台机器人", r.pos);
    }
}

/// 直接让机器人携带一箱货物（绕过 pick 流程的布置辅助）。
#[allow(dead_code)]
pub fn give_robot_a_box(w: &mut World, robot: Id, goods_type: &str) -> Id {
    let id = w.next_id;
    w.next_id += 1;
    let pos = w.robots[&robot].pos;
    w.ground_boxes.insert(
        id,
        GroundBox {
            id,
            goods_type: goods_type.to_string(),
            pos,
            holder: Some(robot),
        },
    );
    w.robots.get_mut(&robot).unwrap().carry = Some(id);
    id
}

/// 直接把一箱货物放进货架（布置辅助）。
#[allow(dead_code)]
pub fn put_box_on_shelf(w: &mut World, shelf: Id, goods_type: &str) -> Id {
    let id = w.next_id;
    w.next_id += 1;
    let pos = w.shelves[&shelf].pos;
    w.ground_boxes.insert(
        id,
        GroundBox {
            id,
            goods_type: goods_type.to_string(),
            pos,
            holder: Some(shelf),
        },
    );
    w.shelves.get_mut(&shelf).unwrap().box_ids.push(id);
    id
}

/// 电量刚好只够一次 pick+drop 的机器人（低电量场景布置）。
#[allow(dead_code)]
pub fn set_energy(w: &mut World, robot: Id, energy: u32) {
    w.robots.get_mut(&robot).unwrap().energy = energy;
}

/// 一次“拾起并放下”完整往返的电量成本。
#[allow(dead_code)]
pub const PICK_DROP_COST: u32 = ENERGY_COST_PICK + ENERGY_COST_DROP;

/// 与 CHARGE_PER_TICK 同源，供断言引用。
#[allow(dead_code)]
pub const CHARGE: u32 = CHARGE_PER_TICK;

/// 标准演示世界：10×10、1 机器人 (1,1)。
#[allow(dead_code)]
pub fn demo_world() -> World {
    let mut w = World::new_empty(10, 10, 1_000_000);
    w.add_robot(Position::new(1, 1));
    w
}
