//! 受控 memory 验收（docs/architecture/08 验证表「memory 原子性」的 A0 子集；
//! 值模型全量矩阵在 A1）。

mod common;

use common::host_bin;
use common::{demo_world, fixture, session};
use ztw_api::harness::OutcomeKind;
use ztw_api::harness::{Session, SessionConfig};
use ztw_api::memory::MemoryLimits;
use ztw_model::MemValue;

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
fn deep_copy_trap_and_controlled_ops() {
    let mut s = session(demo_world());
    assert!(s.load_code(&fixture("memory_ops_demo.js")).ok);
    for _ in 0..3 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok);
    }
    let snap = s.memory_snapshot();
    // 陷阱：items.push(1) 未提交；受控 push(2) 提交；loop 每 tick 追加。
    assert_eq!(
        get_map(&snap, "items"),
        &MemValue::List(vec![num(2.0), num(0.0), num(1.0), num(2.0)])
    );
    assert_eq!(get_map(&snap, "count"), &num(4.0));
    assert_eq!(get_map(&snap, "scalar"), &num(42.0));
    // 嵌套句柄写。
    assert_eq!(
        get_map(&snap, "nested"),
        &MemValue::Map(vec![(
            "a".into(),
            MemValue::Map(vec![
                ("b".into(), MemValue::List(vec![num(1.0), num(2.0)])),
                ("c".into(), num(3.0))
            ])
        )])
    );
    // r.memory 自动创建且共享同一路径。
    let robots = get_map(&snap, "robots");
    let r1 = get_map(robots, "1");
    assert_eq!(get_map(r1, "visited"), &num(3.0));
    // keys() 有序枚举（插入序）。
    let last_log = s.logs.back().unwrap().1.clone();
    assert!(last_log.starts_with("keys "), "日志：{last_log}");
}

#[test]
fn illegal_values_rejected_tree_unchanged() {
    let code = r#"
export function loop() {
  Game.memory["seed"] = 1;
  const before = JSON.stringify(Game.memory.to_dict());
  const tries = [];
  try { Game.memory["f"] = function () {}; } catch (e) { tries.push(e.code); }
  try { Game.memory["n"] = NaN; } catch (e) { tries.push(e.code); }
  try { Game.memory["i"] = 1e16; } catch (e) { tries.push(e.code); }
  try { const o = {}; o.self = o; Game.memory["o"] = o; } catch (e) { tries.push(e.code); }
  try { const a = [1, , 3]; Game.memory["a"] = a; } catch (e) { tries.push(e.code); }
  Game.log("codes", tries.join(","), "unchanged", before === JSON.stringify(Game.memory.to_dict()));
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s.logs.back().unwrap().1.clone();
    assert!(
        line.contains("INVALID_VALUE"),
        "非法值应返回 INVALID_VALUE：{line}"
    );
    assert!(line.ends_with("unchanged true"), "原树不得改变：{line}");
    // 主进程侧同样只剩 seed。
    assert_eq!(
        s.memory_snapshot(),
        MemValue::Map(vec![("seed".into(), num(1.0))])
    );
}

#[test]
fn over_limit_rejects_atomically() {
    // 缩小的 memory 限额：写超限被原子拒绝。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.memory = MemoryLimits {
        max_nodes: 64,
        max_bytes: 2048,
        max_depth: 6,
        // 字符串上限放宽，确保触发的是字节预算（MEMORY_LIMIT）而非字符串长度。
        max_string: 400 * 1024,
    };
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("big_memory_write.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s.logs.back().unwrap().1.clone();
    assert!(line.contains("MEMORY_LIMIT"), "超限应拒绝：{line}");
    // 原树不变（初始化写入的 seed 保留，big 未出现）。
    assert_eq!(
        s.memory_snapshot(),
        MemValue::Map(vec![("seed".into(), num(1.0))])
    );
    // 限额内写入仍可用（此前成功调用不受失败影响）。
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
}

#[test]
fn init_branch_commit_or_discard() {
    // 多次修改整体提交：初始化全部成功 → 全部可见。
    let ok_code = r#"
Game.memory["a"] = 1;
Game.memory["b"] = [1, 2, 3];
Game.memory["c"] = { k: "v" };
export function loop() {}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(ok_code).ok);
    let snap = s.memory_snapshot();
    assert_eq!(get_map(&snap, "a"), &num(1.0));
    assert_eq!(
        get_map(&snap, "b"),
        &MemValue::List(vec![num(1.0), num(2.0), num(3.0)])
    );
    assert_eq!(
        get_map(&snap, "c"),
        &MemValue::Map(vec![("k".into(), MemValue::Str("v".into()))])
    );

    // 整体放弃：初始化前半成功、后半抛错 → 全部丢弃。
    let fail_code = r#"
Game.memory["x1"] = 1;
Game.memory["x2"] = 2;
throw new Error("half done");
"#;
    let mut s2 = session(demo_world());
    let init = s2.load_code(fail_code);
    assert!(!init.ok);
    assert_eq!(s2.memory_snapshot(), MemValue::Map(vec![]));
    // 世界不变：tick 未推进。
    assert_eq!(s2.world.tick, 0);
}

#[test]
fn reserved_keys_rejected() {
    let code = r#"
export function loop() {
  const tries = [];
  try { Game.memory["robots"] = 1; } catch (e) { tries.push(e.code); }
  try { delete Game.memory["robots"]; } catch (e) { tries.push(e.code); }
  try { Game.robots()[0].memory["_move"] = 1; } catch (e) { tries.push(e.code); }
  Game.log("codes", tries.join(","));
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s.logs.back().unwrap().1.clone();
    assert_eq!(
        line, "codes RESERVED_KEY,RESERVED_KEY,RESERVED_KEY",
        "保留键必须拒绝：{line}"
    );
    // r.memory 仍然自动创建了 robots/1。
    let snap = s.memory_snapshot();
    assert!(get_map(&snap, "robots") != &MemValue::Null);
}

#[test]
fn stale_handle_after_replacement() {
    let code = r#"
export function loop() {
  Game.memory["m"] = { k: 1 };
  const h = Game.memory["m"];          // 句柄
  Game.memory["m"] = 2;                 // 整体替换 → 旧句柄失效
  let code = "none";
  try { h["k"] = 9; } catch (e) { code = e.code; }
  Game.log("stale", code, "new", Game.memory["m"]);
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s.logs.back().unwrap().1.clone();
    assert!(
        line.contains("STALE_MEMORY_REFERENCE"),
        "失效句柄必须报错：{line}"
    );
    assert!(line.ends_with("new 2"), "新值不受旧句柄影响：{line}");
    assert_eq!(get_map(&s.memory_snapshot(), "m"), &num(2.0));
}

#[test]
fn stale_handle_cached_read_after_replacement() {
    // 槽位缓存评审回归：容器读命中缓存（含孙辈）不得绕过「失效句柄
    // 访问报 STALE」——先用嵌套读热缓存，再替换祖先，各深度旧句柄的
    // 读与写都必须报错（本地毒化树兜住，不依赖读穿透服务端）。
    let code = r#"
export function loop() {
  Game.memory["m"] = { a: { b: { c: 1 } } };
  const h = Game.memory["m"]["a"];       // a 句柄(热缓存)
  const deep = Game.memory["m"]["a"]["b"]; // b 句柄(缓存在 a 的槽位)
  Game.memory["m"] = 2;                  // 替换祖先 → 整棵子树死亡
  let r1 = "none", r2 = "none", r3 = "none";
  try { h["b"]; } catch (e) { r1 = e.code; }
  try { deep["c"]; } catch (e) { r2 = e.code; }
  try { deep["c"] = 9; } catch (e) { r3 = e.code; }
  Game.log("stale-read", r1, r2, r3, "new", Game.memory["m"]);
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s.logs.back().unwrap().1.clone();
    assert!(
        line.starts_with(
            "stale-read STALE_MEMORY_REFERENCE STALE_MEMORY_REFERENCE STALE_MEMORY_REFERENCE"
        ),
        "各深度旧句柄读写均须报 STALE：{line}"
    );
    assert!(line.ends_with("new 2"), "新值不受旧句柄影响：{line}");
}

#[test]
fn list_remove_survives_slot_cache() {
    // 槽位缓存评审回归（P0 同源路径）：remove 平移下标后，缓存下标必须
    // 整体失效——读到的应是平移后的新值，而不是旧包装器。
    let code = r#"
export function loop() {
  Game.memory["l"] = [10, 20, 30];
  const l = Game.memory["l"];
  const warm = l[1];                      // 热缓存下标 1(标量,不入槽)
  Game.memory["nest"] = { inner: [1] };
  const inner = Game.memory["nest"]["inner"]; // 容器句柄入槽
  l.remove(0);
  Game.log("shift", l[0], l[1], l.length, warm);
  inner.push(2);
  Game.log("inner", Game.memory["nest"]["inner"].length);
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    let logs: Vec<&str> = s.logs.iter().map(|(_, l)| l.as_str()).collect();
    assert!(
        logs.contains(&"shift 20 30 2 20"),
        "remove 后下标平移：{logs:?}"
    );
    assert!(
        logs.contains(&"inner 2"),
        "remove 不误伤其它容器句柄：{logs:?}"
    );
}

#[test]
fn memory_survives_hot_reload_and_host_restart() {
    // memory 跨热重载保留；跨宿主重启保留（重新建立句柄后仍可访问）。
    let mut s = session(demo_world());
    let first = r#"
Game.memory["persist"] = "v1";
Game.memory["counter"] = 0;
export function loop() { Game.memory["counter"] = Game.memory["counter"] + 1; }
"#;
    assert!(s.load_code(first).ok);
    s.tick();
    s.tick();
    assert_eq!(get_map(&s.memory_snapshot(), "counter"), &num(2.0));

    // 热重载：普通全局变量重置，memory 保留。
    let second = r#"
export function loop() { Game.memory["counter"] = Game.memory["counter"] + 10; }
"#;
    assert!(s.load_code(second).ok, "热重载初始化失败");
    s.tick();
    assert_eq!(get_map(&s.memory_snapshot(), "counter"), &num(12.0));
    assert_eq!(
        get_map(&s.memory_snapshot(), "persist"),
        &MemValue::Str("v1".into())
    );

    // 宿主重启：杀进程后 restart_host 重新初始化，memory 仍在。
    s.kill_host_now();
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(ztw_api::harness::FaultClass::HostTerminated("killed"))
    );
    let init = s.restart_host();
    assert!(init.ok, "重启后初始化失败：{:?}", init.fault);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_eq!(get_map(&s.memory_snapshot(), "counter"), &num(22.0));
}

#[test]
fn failed_hot_reload_preserves_existing_memory() {
    // docs/architecture/06「热重载」：编译或初始化失败后保留原世界与
    // memory。从非空树出发的热重载失败是修复对账的关键场景。
    let mut s = session(demo_world());
    let first = r#"
Game.memory["stage"] = "A";
Game.memory["log"] = [1, 2];
export function loop() { Game.memory["stage"] = "A"; }
"#;
    assert!(s.load_code(first).ok);
    s.tick();
    let before = s.memory_snapshot();

    // 热重载失败：新代码写了几笔后抛错。
    let bad = r#"
Game.memory["stage"] = "B";
Game.memory["extra"] = 1;
throw new Error("reload failed");
"#;
    let init = s.load_code(bad);
    assert!(!init.ok);
    assert_eq!(init.fault.unwrap().code, "Error");
    // 原 memory 分毫未动（A 阶段的提交与 tick 内更新都在）。
    assert_eq!(s.memory_snapshot(), before);
    // 会话可用：再次热重载成功后继续。
    let good = r#"
export function loop() { Game.memory["stage"] = "C"; }
"#;
    assert!(s.load_code(good).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_eq!(
        get_map(&s.memory_snapshot(), "stage"),
        &MemValue::Str("C".into())
    );
}

#[test]
fn init_phase_rejects_mutations_but_allows_memory() {
    // 初始化阶段：动作与管理操作返回 INIT_PHASE；memory 与查询可用。
    let code = r#"
const codes = [];
codes.push(Game.robots()[0].move(Game.EAST));      // INIT_PHASE
codes.push(Game.market.take(Game.market.sell_orders()[0].id)); // INIT_PHASE
Game.memory["inited"] = Game.tick;                 // 允许
const seen = Game.my_orders().length;              // 查询允许
export function loop() {
  Game.log("codes", codes.join(","), "orders", seen);
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let line = s.logs.back().unwrap().1.clone();
    assert_eq!(line, "codes INIT_PHASE,INIT_PHASE orders 0");
    assert_eq!(
        get_map(&s.memory_snapshot(), "inited"),
        &num(0.0),
        "init 时 Game.tick 为 0"
    );
    // 世界不受 init 期动作影响。
    assert_eq!(s.world.robots[&1].pos, ztw_model::Position::new(1, 1));
    assert_eq!(s.world.listings.len(), 2);
}
