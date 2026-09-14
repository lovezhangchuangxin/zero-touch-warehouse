//! A1 Python 绑定层功能闭环：demo_one.py 移动+记账、memory_ops 深拷贝
//! 陷阱与嵌套写、市场闭环（take → 增量回放 → cancel）。与 JS 侧
//! a0_step1 / a0_memory 的同款断言构成双语言对照锚点。

mod common;

use common::demo_world;
use ztw_api::harness::OutcomeKind;
use ztw_model::MemValue;

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

fn num(v: f64) -> MemValue {
    MemValue::Num(v)
}

fn get_map<'a>(v: &'a MemValue, key: &str) -> &'a MemValue {
    match v {
        MemValue::Map(pairs) => &pairs.iter().find(|(k, _)| k == key).unwrap().1,
        _ => panic!("期望映射"),
    }
}

#[test]
fn demo_one_py_roundtrip() {
    let mut s = common::session(demo_world());
    let init = s.load_code(&fixture("demo_one.py"));
    assert!(init.ok, "初始化失败：{:?}", init.fault);
    for _ in 0..10 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    }
    assert_eq!(s.world.tick, 10);
    let snap = s.memory_snapshot();
    assert_eq!(get_map(&snap, "n"), &num(10.0), "根计数每 tick +1");
    // 机器人记忆（robots → "<id>" → 记忆映射）：last == 最后一 tick（9）。
    let robot_mem = get_map(get_map(&snap, "robots"), "1");
    assert_eq!(get_map(robot_mem, "last"), &num(9.0));
}

#[test]
fn memory_ops_py_deep_copy_trap_and_nested_writes() {
    let mut s = common::session(demo_world());
    let init = s.load_code(&fixture("memory_ops_demo.py"));
    assert!(init.ok, "初始化失败：{:?}", init.fault);
    for _ in 0..3 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    }
    let snap = s.memory_snapshot();
    // 与 a0_memory 的 JS 断言完全一致（双语言同结果锚点）：
    // 陷阱：items.append(1) 未提交；受控 append(2) 提交；loop 每 tick 追加。
    assert_eq!(
        get_map(&snap, "items"),
        &MemValue::List(vec![num(2.0), num(0.0), num(1.0), num(2.0)])
    );
    assert_eq!(get_map(&snap, "count"), &num(4.0));
    assert_eq!(get_map(&snap, "scalar"), &num(42.0));
    assert_eq!(get_map(&snap, "note"), &MemValue::Str("hello".into()));
    assert_eq!(
        get_map(&snap, "nested"),
        &MemValue::Map(vec![(
            "a".into(),
            MemValue::Map(vec![
                ("b".into(), MemValue::List(vec![num(1.0), num(2.0)])),
                ("c".into(), num(3.0)),
            ]),
        )])
    );
    // 机器人记忆句柄：visited 每 tick +1（机器人 id=1）。
    let robot_mem = get_map(get_map(&snap, "robots"), "1");
    assert_eq!(get_map(robot_mem, "visited"), &num(3.0));
    // 每条 log 请求带 keys 摘要（受控 map 的键序列化）。
    assert!(s.logs.iter().all(|(_, l)| l.starts_with("keys ")));
}

#[test]
fn market_take_py_delta_replay_and_cancel() {
    let mut s = common::session(demo_world());
    let code = r#"
def loop():
    asks = Game.market.sell_orders()
    if len(asks) > 0 and len(Game.my_orders()) == 0:
        Game.market.take(asks[0].id)
    Game.memory["orders"] = len(Game.my_orders())
    Game.memory["gold"] = Game.gold
    if Game.tick == 2 and len(Game.my_orders()) > 0:
        Game.market.cancel(Game.my_orders()[0].id)
"#;
    let init = s.load_code(code);
    assert!(init.ok, "初始化失败：{:?}", init.fault);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    let snap = s.memory_snapshot();
    // 增量回放：take 的当 tick 即可见（无需整体重建）。
    assert_eq!(get_map(&snap, "orders"), &num(1.0));
    assert_eq!(get_map(&snap, "gold"), &num(190.0), "扣款 2×5.0");
    assert_eq!(s.world.listings.len(), 1);

    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert_eq!(s.world.my_orders.len(), 1);

    let out = s.tick(); // Game.tick == 2 → cancel
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert_eq!(s.world.my_orders.len(), 0, "取消后无持仓订单");
    // 取消语义：订单退场、扣手续费，不回挂单列表（挂单已被消费）。
    assert_eq!(s.world.listings.len(), 1);

    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    let snap = s.memory_snapshot();
    assert_eq!(
        get_map(&snap, "orders"),
        &num(0.0),
        "取消后增量回放移除订单"
    );
}
