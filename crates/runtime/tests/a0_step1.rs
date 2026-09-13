//! A0 第一步验收：loop + Game.tick + move + IPC 往返 + 查询镜像 的
//! 端到端示例（docs/architecture/08 原型 A0）。

mod common;

use common::{demo_world, fixture, session};
use ztw_api::harness::OutcomeKind;
use ztw_model::Position;

#[test]
fn patrol_moves_east_and_west() {
    let mut s = session(demo_world());
    let init = s.load_code(&fixture("demo_patrol.js"));
    assert!(init.ok, "初始化失败：{:?}", init.fault);

    // 6 个 tick：向东走 6 步（12 宽地图，从 x=1 到 x=7 不会折返）。
    for i in 0..6 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok, "tick {i} 失败：{:?}", s.fault);
    }
    assert_eq!(s.world.robots[&1].pos, Position::new(7, 1));
    assert_eq!(s.world.tick, 6);

    // 折返轨迹逐 tick 精确断言（夹具确定性）：
    // tick6 起 x=7；7→8、8→9、9→10（此刻 x>=w-2=10，仍向东走了一步）；
    // 之后 dir 翻转为 WEST：11→9、12→8、13→7、14→6。
    let expect = [8i32, 9, 10, 9, 8, 7, 6, 5];
    for (i, x) in expect.iter().enumerate() {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok, "tick {}", 7 + i);
        assert_eq!(s.world.robots[&1].pos.x, *x, "tick {}", 7 + i);
    }
    // move 的 IPC 往返：每 tick 恰一次变更型调用（无日志时）。
    let last = s.tick();
    assert_eq!(last.requests_served, 1, "纯巡逻 tick 应只有一次 move 往返");
}

#[test]
fn mirror_serves_queries_without_ipc() {
    // 只查询不动作的脚本：整个 tick 零 IPC（查询走本地镜像）。
    let code = r#"
function loop() {
  const rs = Game.robots();
  const vs = Game.vehicles();
  const s = Game.shelves().length + Game.chargers().length + Game.docks().length;
  const m = Game.map_size();
  Game.tick;
  Game.gold;
  Game.market.sell_orders();
  Game.market.buy_orders();
  Game.my_orders();
  Game.ground_boxes();
  Game.objects_at(1, 1);
  Game.get_object_by_id(rs[0].id);
  if (m[0] < 1 || rs.length < 1) Game.log("bad mirror");
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    for _ in 0..3 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok);
        assert_eq!(out.requests_served, 0, "查询必须走本地镜像，不产生 IPC");
    }
}

#[test]
fn take_order_and_vehicle_arrives_next_tick() {
    let mut s = session(demo_world());
    assert!(s.load_code(&fixture("demo_trade.js")).ok);

    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    // 卖单（玩家买入）：qty=2 × 5000 千分 = 10000 千分扣款。
    assert_eq!(s.world.gold_milli, 200_000 - 10_000);
    assert_eq!(s.world.my_orders.len(), 1);
    assert_eq!(s.world.listings.len(), 1); // 只剩买单

    // 下一 tick 边界：车辆到场、带货、占用装卸位（交互格 = 第二格）。
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_eq!(s.world.vehicles.len(), 1);
    let v = s.world.vehicles.values().next().unwrap();
    assert_eq!(v.kind.as_str(), "in");
    assert_eq!(v.box_ids.len(), 2);
    assert_eq!(v.interact_pos, Position::new(0, 5));
    let dock_id = s.world.docks.keys().next().unwrap();
    assert_eq!(s.world.docks[dock_id].docked_vehicle, Some(v.id));
    // 不重复接单。
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_eq!(s.world.gold_milli, 190_000);
    assert_eq!(s.world.my_orders.len(), 1);
}

#[test]
fn take_effect_visible_same_tick_via_delta_replay() {
    // 管理操作响应携带镜像增量，宿主回放后同 tick 查询可见。
    let mut s = session(demo_world());
    assert!(s.load_code(&fixture("take_and_query.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s
        .logs
        .iter()
        .rev()
        .find(|(_, l)| l.starts_with("take "))
        .map(|(_, l)| l.clone())
        .expect("有 take 日志");
    // before=0 after=1 listings=1（卖单已从镜像消失）gold=190
    assert!(
        line.contains("after 1"),
        "增量回放后 my_orders 应立即可见：{line}"
    );
    assert!(line.contains("listings 1"), "挂单应同步消失：{line}");
    assert!(line.contains("gold 190"), "gold 应经增量更新：{line}");
}

#[test]
fn last_result_visible_next_tick() {
    // 结算结果经镜像在下一 tick 可见；顶到墙角后受理失败日志可见。
    let code = r#"
function loop() {
  const r = Game.robots()[0];
  Game.log("res", r.id, r.pos.x, r.pos.y,
           r.last_result ? r.last_result.code + ":" + r.last_result.action + ":" + r.last_result.arg : "none");
  r.move(Game.EAST);
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    s.tick(); // tick0：last_result 为 none（上一 tick 无动作）
    assert!(s.logs.back().unwrap().1.contains("none"));
    s.tick(); // tick1：可见 tick0 的 move OK
    assert!(s.logs.back().unwrap().1.contains("OK:move:EAST"));
}
