//! M4 寻路贯通 JS 宿主：find_path 三分支与 move_to 复合步经绑定层往返，
//! `_move` 缓存由主进程单源管理（服务端写、玩家读、玩家写拒绝），
//! 跨 tick 推进与热重载后的缓存保留在 Session 闭环内验证。

mod common;

use common::{demo_world, fixture};
use ztw_api::harness::{OutcomeKind, Session, SessionConfig};

fn run_moveto_fixture() -> Session {
    let mut s = Session::new(SessionConfig::new(common::host_bin()), demo_world());
    assert!(
        s.load_code(&fixture("m4_moveto.js")).ok,
        "初始化失败：{:?}",
        s.fault
    );
    for _ in 0..7 {
        assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
    }
    s
}

/// 全程日志锚点：三分支、码路径、缓存推进与对象目的地。
#[test]
fn moveto_fixture_walks_anchors() {
    let s = run_moveto_fixture();
    let logs: Vec<&str> = s.logs.iter().map(|(_, l)| l.as_str()).collect();
    for anchor in [
        "p1:5,2,1,6,1", // 最短路径：不含 start、直线五步
        "p2:len0",      // 已在到达范围 → []（truthy 陷阱的证据点）
        "p3:null",      // 装卸位锚点（障碍）range 0 → 不可达
        "p5:len2",      // 受控序列坐标（JS 代理形态，审查轮 P0 回归锚点）
        "m1:ARRIVED",   // 到达判定优先于行动占用检查
        "m2:OK",
        "m3:ALREADY_ACTED",
        "wm:RESERVED_KEY", // 保留键玩家写拒绝
        "w1:OK@2,1",
        "w2:OK@3,1",
        "w3:OK@4,1",
        "w4:OK@5,1",
        "w5:ARRIVED@6,1", // range 0：到达即停，不占行动
        "mc:6,1|0|5",     // 缓存玩家可读：w1 写入的完整轨迹（w2–w4 命中不重写）
        "np:NO_PATH",     // 越界目标（不占行动）
        "md:OK",          // 对象目的地（装卸位视图）
    ] {
        assert!(logs.contains(&anchor), "缺少锚点 {anchor}：{logs:?}");
    }
}

/// 终态：机器人已沿缓存轨迹走到 (6,1)，np 不占行动后 md 再受领一步。
#[test]
fn moveto_final_state_consistent() {
    let s = run_moveto_fixture();
    let pos = s.world.robots[&1].pos;
    assert_ne!(
        pos,
        ztw_model::Position::new(6, 1),
        "md:OK 后应离开到达格再走一步"
    );
}

/// 热重载保留缓存（memory 语义）：重载后的程序读到旧目标缓存；换目标
/// 调用 move_to 即按新目的地重寻（缓存自愈）。
#[test]
fn moveto_cache_survives_hot_reload_and_replans() {
    let mut s = Session::new(SessionConfig::new(common::host_bin()), demo_world());
    assert!(
        s.load_code(
            r#"
export function loop() {
  var r = Game.robots()[0];
  if (Game.tick === 0) Game.log("a:" + r.move_to([5, 1], { range: 0 }));
}
"#
        )
        .ok,
        "初始化失败：{:?}",
        s.fault
    );
    assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
    // 热重载（同一机器人）：t1 读旧缓存（跨 load_code 存活），t2 换目标重寻。
    assert!(
        s.load_code(
            r#"
export function loop() {
  var r = Game.robots()[0];
  if (Game.tick === 1) {
    var mv = JSON.parse(r.memory["_move"]);
    Game.log("c1:" + mv.goal[0] + "," + mv.goal[1] + "," + mv.range + "," + mv.path.length);
  } else if (Game.tick === 2) {
    r.move_to([6, 1], { range: 0 });
    var mv = JSON.parse(r.memory["_move"]);
    Game.log("c2:" + mv.goal[0] + "," + mv.goal[1] + "," + mv.range + "," + mv.path.length);
  }
}
"#
        )
        .ok,
        "热重载初始化失败：{:?}",
        s.fault
    );
    for _ in 0..2 {
        assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
    }
    let logs: Vec<&str> = s.logs.iter().map(|(_, l)| l.as_str()).collect();
    // c1：重载后仍读到重载前的缓存（goal (5,1)，从 (1,1) 出发的完整轨迹
    // 5 格——跨 load_code 存活的证据）；c2：换目标重寻（goal (6,1)，从
    // 当前 (2,1) 出发 4 步 + 起点共 5 格）。
    assert!(logs.contains(&"c1:5,1,0,5"), "{logs:?}");
    assert!(logs.contains(&"c2:6,1,0,5"), "{logs:?}");
}

/// 初始化阶段：find_path 放行（查询），move_to 拒 INIT_PHASE（动作）。
#[test]
fn init_phase_allows_find_path_rejects_move_to() {
    let mut s = Session::new(SessionConfig::new(common::host_bin()), demo_world());
    let init = s
        .load_code(
            r#"
const p = Game.find_path([1, 1], [6, 1]);
Game.log("init:" + p.length + "," + Game.robots()[0].move_to([6, 1]));
export function loop() {}
"#,
        )
        .ok;
    assert!(init, "初始化应成功（拒绝是返回码，不是异常）");
    assert_eq!(s.tick().kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert!(
        s.logs.iter().any(|(_, l)| l == "init:5,INIT_PHASE"),
        "{:?}",
        s.logs
    );
}
