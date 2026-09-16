//! M4 寻路示例验收：demo_moveto.js（编辑器示例同一份文本，单一事实源）
//! 在 B2 单机场景跑巡逻——move_to 复合步跨 tick 推进与 ARRIVED 换段
//! 全程无脚本故障。（demo 的低电滞回是扩展位：巡逻只移动不耗电，
//! 该分支在本场景不触发，注释已如实说明。）

use ztw_api::harness::{OutcomeKind, Session, SessionConfig};
use ztw_desktop::{hostbin, scenario};

const MOVETO: &str = include_str!("../../../web/apps/game/src/scripts/demos/demo_moveto.js");

#[test]
fn moveto_demo_patrols_without_faults() {
    let bins = hostbin::resolve_host_bins();
    let mut s = Session::new(
        SessionConfig::new(bins.js),
        scenario::build(&scenario::B2_ONE),
    );
    assert!(s.load_code(MOVETO).ok, "示例加载应成功：{:?}", s.fault);
    let mut positions = std::collections::BTreeSet::new();
    let robot_id = *s.world.robots.keys().next().expect("场景应有机器人");
    for _ in 0..240 {
        let out = s.tick();
        assert_eq!(
            out.kind,
            OutcomeKind::Ok,
            "巡逻不得有脚本故障：{:?}",
            s.fault
        );
        positions.insert(s.world.robots[&robot_id].pos);
    }
    assert!(
        s.logs.iter().any(|(_, l)| l.starts_with("首段路径长：")),
        "开场的 find_path 查询应有日志：{:?}",
        s.logs
    );
    // 巡逻覆盖多段位置（move_to 持续推进而非原地打转：巡逻环远超 8 格），
    // 玩家键 leg 与服务端缓存 _move 都在 memory 中（存在性断言——leg 在
    // tick 0 就写入，不单独证明换段；换段证据由 positions 覆盖承担）。
    assert!(positions.len() >= 8, "机器人应走访多个格子：{positions:?}");
    let mem = serde_json::to_value(s.memory_snapshot()).unwrap();
    let mem_str = mem.to_string();
    assert!(
        mem_str.contains("leg"),
        "巡逻段号应已写入 memory：{mem_str}"
    );
    assert!(
        mem_str.contains("_move"),
        "move_to 缓存应在 memory 中：{mem_str}"
    );
}
