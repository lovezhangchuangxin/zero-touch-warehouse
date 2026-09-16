//! C1 存档信封与 memory 重建验收（docs/architecture/06 §文件格式与
//! 落盘、§写入安全）。无宿主：SaveData 组装是纯函数，世界与 memory
//! 直接构造。

use serde_json::{Value, json};
use ztw_api::harness::PlayerProgram;
use ztw_api::memory::{MemoryLimits, MemoryTree};
use ztw_api::save::{SAVE_FORMAT_VERSION, decode, encode};
use ztw_model::{MemValue, Position};
use ztw_sim::World;

fn num(v: f64) -> MemValue {
    MemValue::Num(v)
}

fn content_tree() -> MemoryTree {
    let mut t = MemoryTree::new(MemoryLimits::default());
    t.map_set(0, 0, "score", &num(42.0)).unwrap();
    t.map_set(0, 0, "items", &MemValue::List(vec![num(1.0), num(2.0)]))
        .unwrap();
    let rm = t.robot_memory(0, 3).unwrap();
    t.map_set(0, rm, "note", &MemValue::Str("岗位 A".into()))
        .unwrap();
    // 追加修订：让 revision 非零可断言恢复。
    let _ = rm;
    t
}

fn sample_world() -> World {
    let mut w = World::new_empty(12, 12, 1_000_000)
        .with_seed(20260913)
        .with_market();
    w.add_robot(Position::new(1, 1));
    w.add_shelf(Position::new(2, 3));
    w.add_dock(Position::new(0, 4), (1, 0));
    for _ in 0..30 {
        w.boundary_events();
        w.settle();
        w.end_tick();
    }
    w
}

/// memory 重建往返：内容逐字节相等、修订号恢复、保留键语义（RESERVED_KEY
/// 与 r.memory 同节点）照旧成立。
#[test]
fn memory_restore_roundtrip() {
    let mut t = content_tree();
    let rev = t.revision();
    assert!(rev > 0, "布置须产生修订");
    let snap = t.snapshot();
    let mut restored =
        MemoryTree::from_snapshot(snap, rev, MemoryLimits::default()).expect("合法快照重建");
    assert_eq!(restored.revision(), rev, "修订号按存档恢复");
    assert_eq!(restored.snapshot(), t.snapshot(), "重建后整树内容相等");
    // 保留键：robots 整体替换仍被拒；robots/<id> 经 r.memory 命中同一节点。
    assert_eq!(
        restored
            .map_set(0, 0, "robots", &num(1.0))
            .unwrap_err()
            .code,
        "RESERVED_KEY"
    );
    let rm = restored.robot_memory(0, 3).unwrap();
    assert_eq!(
        restored.map_get(0, rm, "note").unwrap(),
        ztw_api::memory::ReadResult::Scalar(MemValue::Str("岗位 A".into()))
    );
    // 重建后继续写：修订号从恢复点续增。
    restored.map_set(0, 0, "k", &num(9.0)).unwrap();
    assert_eq!(restored.revision(), rev + 1);
}

/// 读档限额复核与写入路径同标准：超限存档整体拒绝（不产出半棵树）。
#[test]
fn memory_restore_enforces_limits() {
    let mut t = MemoryTree::new(MemoryLimits::default());
    t.map_set(
        0,
        0,
        "l",
        &MemValue::List((0..16).map(|i| num(i as f64)).collect()),
    )
    .unwrap();
    let snap = t.snapshot();
    let tiny = MemoryLimits {
        max_nodes: 4,
        max_bytes: 256,
        max_depth: 16,
        max_string: 8 * 1024,
    };
    let err = MemoryTree::from_snapshot(snap, 0, tiny).unwrap_err();
    assert_eq!(err.code, "MEMORY_LIMIT", "超限存档须按 MEMORY_LIMIT 拒绝");
}

/// 坏档形态逐一拒绝：非映射根 / robots 非映射 / robots 条目非映射 /
/// robots 键非数字或非规范 id / 根或嵌套映射重复键。
#[test]
fn memory_restore_rejects_corrupt_forms() {
    let lim = MemoryLimits::default();
    let cases: Vec<(MemValue, &str)> = vec![
        (num(1.0), "根非映射"),
        (
            MemValue::Map(vec![("robots".into(), num(1.0))]),
            "robots 非映射",
        ),
        (
            MemValue::Map(vec![(
                "robots".into(),
                MemValue::Map(vec![("7".into(), num(5.0))]),
            )]),
            "robots 条目非映射",
        ),
        (
            MemValue::Map(vec![(
                "robots".into(),
                MemValue::Map(vec![("seven".into(), MemValue::Map(vec![]))]),
            )]),
            "robots 键非数字",
        ),
        (
            MemValue::Map(vec![(
                "robots".into(),
                MemValue::Map(vec![("007".into(), MemValue::Map(vec![]))]),
            )]),
            "robots 键非规范十进制（007 ≠ 7 的寻址形态）",
        ),
        (
            MemValue::Map(vec![("a".into(), num(1.0)), ("a".into(), num(2.0))]),
            "根下重复键",
        ),
        (
            MemValue::Map(vec![(
                "m".into(),
                MemValue::Map(vec![("k".into(), num(1.0)), ("k".into(), num(2.0))]),
            )]),
            "嵌套映射重复键",
        ),
        (
            MemValue::Map(vec![(
                "robots".into(),
                MemValue::Map(vec![(
                    "7".into(),
                    MemValue::Map(vec![(
                        "_move".into(),
                        MemValue::Map(vec![("goal".into(), num(1.0))]),
                    )]),
                )]),
            )]),
            "_move 容器形（服务端恒写标量，容器即手改档——恢复会破坏宿主句柄缓存不变量）",
        ),
    ];
    for (root, why) in cases {
        let err = MemoryTree::from_snapshot(root, 0, lim.clone()).unwrap_err();
        assert_eq!(err.code, "INVALID_VALUE", "{why} 须按坏档拒绝：{err:?}");
    }
}

/// `_move`（move_to 路径缓存）自 M4 起由服务端写入线上树（JSON 字符串
/// 标量），存档恢复原样接受；标量形状异常（非字符串）自愈为缓存 miss，
/// 不构成坏档。
#[test]
fn memory_restore_accepts_move_cache() {
    let canonical = MemValue::Str(r#"{"goal":[3,4],"range":1,"path":[[1,4],[2,4]]}"#.to_string());
    // 规范 JSON 字符串与非字符串标量（形状异常、首次 move_to 自愈）都
    // 原样恢复。
    for (v, why) in [
        (canonical.clone(), "规范缓存形状"),
        (num(1.0), "非字符串标量"),
    ] {
        let want = serde_json::to_value(&v).unwrap();
        let root = MemValue::Map(vec![(
            "robots".into(),
            MemValue::Map(vec![("7".into(), MemValue::Map(vec![("_move".into(), v)]))]),
        )]);
        let mut tree = MemoryTree::from_snapshot(root, 0, MemoryLimits::default())
            .unwrap_or_else(|e| panic!("{why} 的 _move 存档应接受：{e:?}"));
        let got = tree.server_move_cache_get(7).expect("_move 应恢复");
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            want,
            "{why} 应原样恢复"
        );
    }
}

/// 信封往返：encode → decode → 世界恢复 state_hash 对账、memory 与
/// 草稿段无损。
#[test]
fn savefile_roundtrip() {
    let world = sample_world();
    let mut mem = content_tree();
    let rev = mem.revision();
    let mem_root = mem.snapshot();
    let program = PlayerProgram::new(
        [("main.js".to_string(), "function loop() {}".to_string())]
            .into_iter()
            .collect(),
        "main.js",
    );
    let drafts = json!({"js": [{"name": "main.js", "code": "// 草稿"}], "entry": "main.js"});
    let data = ztw_api::save::SaveData::capture(
        &world,
        mem_root,
        rev,
        &program,
        "js",
        "m3-trade",
        Some("第一档".into()),
        Some(drafts.clone()),
        1_760_000_000_000,
    );
    let text = encode(&data);
    // 文件本体经纯 Rust 路径：u64 指纹以 JSON 数字无损存取。
    assert!(text.contains(&format!("\"state_hash\":{}", world.state_hash())));
    let decoded = decode(&text).expect("自家编码须可解码");
    let restored_world = decoded.restore_world().expect("指纹对账");
    assert_eq!(restored_world.state_hash(), world.state_hash());
    assert_eq!(decoded.memory.revision, rev);
    assert_eq!(decoded.memory.root, mem.snapshot());
    assert_eq!(decoded.drafts, Some(drafts));
    assert_eq!(decoded.program.files["main.js"], "function loop() {}");
    assert_eq!(decoded.scenario_id, "m3-trade");
}

/// 版本门禁：三道硬门（格式 / 规则 / 协议）任一不匹配即拒读，错误点名
/// 期望与实际。
#[test]
fn version_gates_reject_incompatible_saves() {
    let world = sample_world();
    let data = ztw_api::save::SaveData::capture(
        &world,
        MemValue::Map(vec![]),
        0,
        &PlayerProgram::single_js("function loop() {}"),
        "js",
        "b2-one",
        None,
        None,
        0,
    );
    let base: Value = serde_json::from_str(&encode(&data)).expect("信封是合法 JSON");
    let expect_rules = ztw_sim::RULES_VERSION;
    let expect_protocol = ztw_api::protocol::PROTOCOL_VERSION;
    for (key, val, needle) in [
        ("format_version", json!(SAVE_FORMAT_VERSION + 1), "格式版本"),
        ("rules_version", json!("ancient"), "规则版本"),
        ("protocol_version", json!(expect_protocol + 1), "协议版本"),
    ] {
        let mut v = base.clone();
        v[key] = val;
        let err = decode(&v.to_string()).expect_err("版本不匹配须拒绝");
        assert!(err.contains(needle), "{key} 拒读须点名：{err}");
    }
    assert_eq!(expect_rules, "m3", "规则版本锚（里程碑推进时同步递增）");
}

/// 校验和门禁：save 段被篡改后重写文件（校验和保持旧值）须拒读；截断
/// 与非本游戏文件同样明确报错。
#[test]
fn checksum_and_shape_gates() {
    let world = sample_world();
    let data = ztw_api::save::SaveData::capture(
        &world,
        MemValue::Map(vec![]),
        0,
        &PlayerProgram::single_js("function loop() {}"),
        "js",
        "b2-one",
        None,
        None,
        0,
    );
    let text = encode(&data);
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["save"]["tick"] = json!(u64::MAX);
    let err = decode(&v.to_string()).expect_err("篡改载荷须拒读");
    assert!(err.contains("校验和"), "须按校验和失败报错：{err}");
    let truncated = &text[..text.len() / 2];
    assert!(decode(truncated).is_err(), "截断文件须拒绝");
    assert!(decode("{}").is_err(), "空 JSON 须按 magic 不符拒绝");
}

/// 指纹对账双保险：信封结构合法但 state_hash 字段与内容不符（镜像转换
/// 丢字段的运行时等价物）时，restore_world 拒绝。
#[test]
fn restore_world_rejects_fingerprint_mismatch() {
    let world = sample_world();
    let mut data = ztw_api::save::SaveData::capture(
        &world,
        MemValue::Map(vec![]),
        0,
        &PlayerProgram::single_js("function loop() {}"),
        "js",
        "b2-one",
        None,
        None,
        0,
    );
    data.state_hash ^= 1;
    let err = decode(&encode(&data))
        .expect("信封自身合法")
        .restore_world()
        .expect_err("指纹不符须拒绝");
    assert!(err.contains("指纹"), "错误须说明对账失败：{err}");
}
