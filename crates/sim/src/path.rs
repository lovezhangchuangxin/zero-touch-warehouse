//! find_path：静态障碍等代价 BFS。语义权威 docs/game-design/08「寻路」：
//! 可通行格复用 `statically_passable`（机器人不算障碍——寻路回答"物理
//! 可达"，动态避让由玩家代码负责）；到达集 = 与 goal 正交距离 ≤ range
//! 的可通行格；最短路径不唯一时按方向序（北、南、西、东）取首个发现，
//! 相同世界状态下结果确定。
//!
//! 纯查询：不消耗 PRNG、不写世界、不入 state_hash。move_to 的复合逻辑
//! （缓存校验与 `_move` 写入）在 crates/api 的 op 分发层单源实现
//! （docs/architecture/04），本模块只回答"物理可达"。

use std::collections::{BTreeMap, VecDeque};

use ztw_model::Position;

use crate::world::World;

/// 寻路结果（`BudgetExceeded` 与 `Unreachable` 分开报告：不可达是玩法
/// 语义，预算超限是调用成本防线，二者不得混淆——docs/game-design/08）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathOutcome {
    /// 最短路径：不含 start、含到达格，逐步正交相邻。
    Path(Vec<Position>),
    /// start 已在到达集内（无需移动）。
    AlreadyThere,
    /// 到达集不可达（界内不存在与 goal 正交距离 ≤ range 的可通行格）。
    Unreachable,
    /// 扩展次数达到 node_budget 仍未到达（防御性上界，正常地图不触发；
    /// 上限常量 PATH_NODE_BUDGET 见 lib.rs 数值锚点）。
    BudgetExceeded,
}

/// 扩展方向序（tie-break 冻结点：北、南、西、东——docs/game-design/08）。
const DIRECTIONS: [(i32, i32); 4] = [
    ztw_model::NORTH,
    ztw_model::SOUTH,
    ztw_model::WEST,
    ztw_model::EAST,
];

impl World {
    /// 静态障碍最短路径。start 不校验可通行性（玩家可能传障碍格，作为
    /// 源点照常扩展，只是自身永不属于到达集）；坐标可为任意 i32（含
    /// 极值——距离计算升 i64、邻步 checked_add，玩家输入不触发算术
    /// 溢出）；`node_budget` 为允许的出队扩展次数，测试注入小值覆盖
    /// 预算路径，线上取 `PATH_NODE_BUDGET`。
    pub fn find_path(
        &self,
        start: Position,
        goal: Position,
        range: i32,
        node_budget: u32,
    ) -> PathOutcome {
        // 到达集成员：与 goal 正交距离 ≤ range 的可通行格。i64 距离：
        // goal / start 是玩家可控的裸 i32，i32 减法在极值处回绕。
        let dist =
            |p: Position| (p.x as i64 - goal.x as i64).abs() + (p.y as i64 - goal.y as i64).abs();
        let in_arrival = |p: Position| dist(p) <= range as i64 && self.statically_passable(p);
        if in_arrival(start) {
            return PathOutcome::AlreadyThere;
        }
        // parent 兼做 visited（start 例外：不入 parent，回溯到 start 即停）。
        let mut parent: BTreeMap<Position, Position> = BTreeMap::new();
        let mut queue: VecDeque<Position> = VecDeque::new();
        queue.push_back(start);
        let mut expanded: u32 = 0;
        while let Some(cur) = queue.pop_front() {
            expanded += 1;
            if expanded > node_budget {
                return PathOutcome::BudgetExceeded;
            }
            for (dx, dy) in DIRECTIONS {
                // start 可为界外极值格：checked_add 防回绕，越界邻格由
                // statically_passable 的界检查过滤（语义不变）。
                let (Some(nx), Some(ny)) = (cur.x.checked_add(dx), cur.y.checked_add(dy)) else {
                    continue;
                };
                let next = Position::new(nx, ny);
                if next == start || !self.statically_passable(next) {
                    continue;
                }
                if parent.contains_key(&next) {
                    continue;
                }
                parent.insert(next, cur);
                if in_arrival(next) {
                    let mut path = vec![next];
                    let mut c = next;
                    while let Some(&p) = parent.get(&c) {
                        if p == start {
                            break;
                        }
                        path.push(p);
                        c = p;
                    }
                    path.reverse();
                    return PathOutcome::Path(path);
                }
                queue.push_back(next);
            }
        }
        PathOutcome::Unreachable
    }
}
