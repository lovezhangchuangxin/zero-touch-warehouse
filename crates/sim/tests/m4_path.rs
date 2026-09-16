//! M4 find_path 语义（docs/game-design/08「寻路」节）：三分支返回、到达集
//! 与 range、机器人不算障碍、方向序 tie-break 确定性、节点预算防线。
//! 纯查询不动世界；仅用公开 API 构造。

use ztw_model::Position;
use ztw_sim::{PATH_NODE_BUDGET, PathOutcome, World};

fn p(x: i32, y: i32) -> Position {
    Position::new(x, y)
}

fn path(start: (i32, i32), goal: (i32, i32), range: i32) -> PathOutcome {
    let w = World::new_empty(8, 8, 0);
    w.find_path(
        p(start.0, start.1),
        p(goal.0, goal.1),
        range,
        PATH_NODE_BUDGET,
    )
}

#[test]
fn straight_line_shortest_path() {
    assert_eq!(
        path((0, 0), (4, 0), 0),
        PathOutcome::Path(vec![p(1, 0), p(2, 0), p(3, 0), p(4, 0)])
    );
}

#[test]
fn already_there_branches() {
    // start == goal 且可通行（range 0）。
    assert_eq!(path((3, 3), (3, 3), 0), PathOutcome::AlreadyThere);
    // 正交距离 ≤ range 即到达（move_to 默认 range 1 的形态）。
    assert_eq!(path((2, 2), (3, 2), 1), PathOutcome::AlreadyThere);
    // 对角格正交距离为 2：range 1 不到达，须移动一步进到达集。
    assert_eq!(path((2, 2), (3, 3), 1), PathOutcome::Path(vec![p(2, 3)]));
    assert_eq!(path((2, 2), (3, 3), 2), PathOutcome::AlreadyThere);
    // 距离超出 range 则仍须移动；range 到达集语义下终点为距 goal ≤ range
    // 的最近格（不必走到 goal 本身）。
    assert_eq!(path((2, 2), (4, 2), 1), PathOutcome::Path(vec![p(3, 2)]));
    // start 自身不可通行时即便距离为 0 也不算到达（到达集只含可通行格）。
    let mut w = World::new_empty(8, 8, 0);
    w.add_shelf(p(3, 3));
    assert_eq!(
        w.find_path(p(3, 3), p(3, 3), 0, PATH_NODE_BUDGET),
        PathOutcome::Unreachable
    );
}

#[test]
fn tie_break_is_north_south_west_east() {
    // (0,0)→(1,1) 两条等长路径；方向序北、南、西、东：南先入队、先扩展，
    // 首个到达路径必经 (0,1)。
    assert_eq!(
        path((0, 0), (1, 1), 0),
        PathOutcome::Path(vec![p(0, 1), p(1, 1)])
    );
}

#[test]
fn tie_break_north_before_south() {
    // 8×8 空图 start (0,1) goal (2,1)，封 (1,1)：北线 (0,0)(1,0)(2,0)(2,1)
    // 与南线 (0,2)(1,2)(2,2)(2,1) 等长——北先扩展，路径必经 (0,0)。
    let mut w = World::new_empty(8, 8, 0);
    w.add_wall(p(1, 1));
    assert_eq!(
        w.find_path(p(0, 1), p(2, 1), 0, PATH_NODE_BUDGET),
        PathOutcome::Path(vec![p(0, 0), p(1, 0), p(2, 0), p(2, 1)])
    );
}

#[test]
fn same_world_same_result_across_calls() {
    // 确定性冻结主张（08「寻路」）：同一世界状态下重复调用结果逐字相等。
    let mut w = World::new_empty(6, 6, 0);
    w.add_shelf(p(2, 2));
    w.add_ground_box("water", p(3, 4));
    let a = w.find_path(p(0, 0), p(5, 5), 1, PATH_NODE_BUDGET);
    let b = w.find_path(p(0, 0), p(5, 5), 1, PATH_NODE_BUDGET);
    assert_eq!(a, b);
}

#[test]
fn extreme_coordinates_do_not_overflow() {
    // 玩家可控的裸 i32 极值坐标：距离与邻步算术不得回绕（debug panic /
    // release 静默误判 AlreadyThere 的审查轮回归锚点）。
    let w = World::new_empty(8, 8, 0);
    assert_eq!(
        w.find_path(p(0, 0), p(i32::MIN, 0), 0, PATH_NODE_BUDGET),
        PathOutcome::Unreachable
    );
    assert_eq!(
        w.find_path(p(i32::MAX, 0), p(0, 0), 0, PATH_NODE_BUDGET),
        PathOutcome::Unreachable
    );
    // 极值 goal + 巨大 range：到达判定按 i64 距离，不会假阳性 AlreadyThere。
    assert_eq!(
        w.find_path(p(0, 0), p(i32::MIN, 0), i32::MAX, PATH_NODE_BUDGET),
        PathOutcome::Unreachable,
        "界内无可通行到达集格（range 再大也够不到界内）"
    );
}

#[test]
fn out_of_bounds_start_still_expands() {
    // start 界外（如贴界外一格）：作为源点照常向界内扩展。
    let w = World::new_empty(8, 8, 0);
    assert_eq!(
        w.find_path(p(8, 1), p(6, 1), 0, PATH_NODE_BUDGET),
        PathOutcome::Path(vec![p(7, 1), p(6, 1)])
    );
    assert_eq!(
        w.find_path(p(99, 99), p(0, 0), 0, PATH_NODE_BUDGET),
        PathOutcome::Unreachable
    );
}

#[test]
fn detour_around_walls() {
    let mut w = World::new_empty(5, 5, 0);
    for y in 0..4 {
        w.add_wall(p(2, y)); // x=2 竖墙，仅 y=4 可穿越
    }
    match w.find_path(p(0, 0), p(4, 0), 0, PATH_NODE_BUDGET) {
        PathOutcome::Path(steps) => {
            // 最短长度：下 4、过墙列（经 y=4）、上 4 → 12 步。
            assert_eq!(steps.len(), 12);
            let mut cur = p(0, 0);
            for s in &steps {
                assert!(cur.adjacent(s), "路径必须逐步正交相邻：{cur:?}→{s:?}");
                assert!(w.statically_passable(*s), "路径不得穿墙：{s:?}");
                cur = *s;
            }
            assert_eq!(cur, p(4, 0), "路径终点须为 goal");
        }
        other => panic!("应可绕行，得 {other:?}"),
    }
}

#[test]
fn goal_on_building_uses_arrival_set() {
    // goal 本身是货架格（不可通行）：range 1 时到达集为其四邻。
    let mut w = World::new_empty(8, 8, 0);
    w.add_shelf(p(4, 4));
    match w.find_path(p(0, 4), p(4, 4), 1, PATH_NODE_BUDGET) {
        PathOutcome::Path(steps) => {
            let end = steps.last().unwrap();
            assert_eq!((end.x - 4).abs() + (end.y - 4).abs(), 1);
            assert!(w.statically_passable(*end));
        }
        other => panic!("应可到达货架邻格，得 {other:?}"),
    }
    // range 0 时到达集为空（goal 不可通行）→ 不可达。
    assert_eq!(
        w.find_path(p(0, 4), p(4, 4), 0, PATH_NODE_BUDGET),
        PathOutcome::Unreachable
    );
}

#[test]
fn enclosed_goal_is_unreachable() {
    let mut w = World::new_empty(7, 7, 0);
    for d in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
        w.add_wall(p(3 + d.0, 3 + d.1));
    }
    assert_eq!(
        w.find_path(p(0, 0), p(3, 3), 0, PATH_NODE_BUDGET),
        PathOutcome::Unreachable
    );
}

#[test]
fn out_of_bounds_goal_is_unreachable() {
    assert_eq!(path((0, 0), (99, 99), 0), PathOutcome::Unreachable);
    // range 拉大也够不到界内可通行格（goal 远在界外）。
    assert_eq!(path((0, 0), (99, 99), 5), PathOutcome::Unreachable);
}

#[test]
fn ground_box_blocks_but_robot_does_not() {
    let mut w = World::new_empty(5, 3, 0);
    w.add_ground_box("water", p(1, 0)); // 直线上的地面货物：障碍
    w.add_robot(p(1, 1)); // 绕行格上的机器人：不是障碍
    match w.find_path(p(0, 0), p(2, 0), 0, PATH_NODE_BUDGET) {
        PathOutcome::Path(steps) => {
            // 绕 y=1：4 步，且穿过机器人所在格 (1,1)（机器人不是障碍）。
            assert_eq!(steps, vec![p(0, 1), p(1, 1), p(2, 1), p(2, 0)]);
        }
        other => panic!("应绕过地面货物，得 {other:?}"),
    }
}

#[test]
fn start_on_obstacle_still_expands() {
    // 玩家传障碍格作 start：不校验，作为源点照常扩展。
    let mut w = World::new_empty(8, 8, 0);
    w.add_shelf(p(0, 0));
    assert_eq!(
        w.find_path(p(0, 0), p(2, 0), 0, PATH_NODE_BUDGET),
        PathOutcome::Path(vec![p(1, 0), p(2, 0)])
    );
}

#[test]
fn negative_range_reaches_nothing() {
    // 负 range 到达集恒空（防线语义，API 层在受理前另有 INVALID_ARGUMENT）。
    assert_eq!(path((0, 0), (1, 0), -1), PathOutcome::Unreachable);
}

#[test]
fn budget_exhaustion_is_distinct_from_unreachable() {
    let w = World::new_empty(8, 8, 0);
    // 扩展序：start → 南格 (0,1)（方向序北南西东，北越界）→ 东格 (1,0)
    // 才把 (2,0) 收进到达——预算 1/2 均在到达前耗尽，3 恰好到达。
    assert_eq!(
        w.find_path(p(0, 0), p(2, 0), 0, 1),
        PathOutcome::BudgetExceeded
    );
    assert_eq!(
        w.find_path(p(0, 0), p(2, 0), 0, 2),
        PathOutcome::BudgetExceeded
    );
    assert_eq!(
        w.find_path(p(0, 0), p(2, 0), 0, 3),
        PathOutcome::Path(vec![p(1, 0), p(2, 0)])
    );
    // 同一世界、充足预算下结果与预算无关（预算只截断不改变路径）。
    assert_eq!(
        w.find_path(p(0, 0), p(2, 0), 0, PATH_NODE_BUDGET),
        PathOutcome::Path(vec![p(1, 0), p(2, 0)])
    );
}

#[test]
fn dock_footprint_blocks_both_cells() {
    // 装卸位两格（锚点 + ext 第二格）均为障碍（docs/game-design/02）。
    let mut w = World::new_empty(6, 6, 0);
    w.add_dock(p(2, 0), (0, 1)); // 锚点 (2,0)，第二格 (2,1)
    for y in 0..6 {
        if y != 0 && y != 1 {
            w.add_wall(p(2, y));
        }
    }
    // 本可通过 (2,0)/(2,1) 穿越的路径：两格被装卸位占据 → 不可达。
    assert_eq!(
        w.find_path(p(0, 0), p(4, 0), 0, PATH_NODE_BUDGET),
        PathOutcome::Unreachable
    );
}
