//! A1 能力收窄验收（docs/architecture/08「能力收窄」行）：白名单外
//! import、文件打开等返回可读错误；白名单内模块及传递依赖可用。
//! 仅验证误用防护，不宣称恶意代码安全（docs 03 定位）。

mod common;

use common::demo_world;
use ztw_api::harness::OutcomeKind;

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).expect("fixture 存在")
}

#[test]
fn denied_imports_give_readable_errors() {
    let mut s = common::session(demo_world());
    let init = s.load_code(&fixture("narrow_import.py"));
    assert!(init.ok, "初始化失败：{:?}", init.fault);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    let log: Vec<&str> = s.logs.iter().map(|(_, l)| l.as_str()).collect();
    for name in [
        "os",
        "itertools",
        "re",
        "random",
        "time",
        "sys",
        "subprocess",
        "socket",
    ] {
        assert!(
            log.contains(&format!("import {name}: DENIED").as_str()),
            "{name} 应被拒绝；实际日志：{log:?}"
        );
    }
    assert!(
        log.contains(&"open: DENIED"),
        "open 应被拒绝；实际日志：{log:?}"
    );
}

#[test]
fn whitelisted_modules_work_with_transitive_deps() {
    let mut s = common::session(demo_world());
    let init = s.load_code(&fixture("whitelist_allowed.py"));
    assert!(init.ok, "初始化失败（含 import 期）：{:?}", init.fault);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    let snap = s.memory_snapshot();
    let pairs = match &snap {
        ztw_model::MemValue::Map(p) => p,
        _ => panic!("期望映射"),
    };
    let get = |k: &str| -> f64 {
        match &pairs.iter().find(|(key, _)| key == k).unwrap().1 {
            ztw_model::MemValue::Num(n) => *n,
            v => panic!("{k} 期望数值，得到 {v:?}"),
        }
    };
    assert_eq!(get("heap_head"), 1.0);
    assert_eq!(get("facts"), 720.0);
    assert_eq!(get("bisect"), 1.0);
    assert_eq!(get("reduce"), 45.0);
    // namedtuple 写入 memory：转 [x, y] 列表（tuple → 列表，docs 06）。
    assert_eq!(
        &pairs.iter().find(|(key, _)| key == "pt").unwrap().1,
        &ztw_model::MemValue::List(vec![
            ztw_model::MemValue::Num(3.0),
            ztw_model::MemValue::Num(4.0),
        ])
    );
}

#[test]
fn source_error_stack_shows_player_frames() {
    // 脚本级错误带 <player> 帧定位（docs 03 故障分级：展示错误类型、
    // 源码位置与堆栈）。
    let mut s = common::session(demo_world());
    let init =
        s.load_code("def boom():\n    raise ValueError('kaputt')\n\ndef loop():\n    boom()\n");
    assert!(init.ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(ztw_api::harness::FaultClass::Script)
    );
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "ValueError");
    assert!(rec.message.contains("kaputt"), "{rec:?}");
    assert!(rec.stack.contains("<player>"), "堆栈应含玩家帧：{rec:?}");
    assert!(rec.stack.contains("line 2"), "定位到 raise 行：{rec:?}");
    assert!(
        !rec.stack.contains("<bootstrap>"),
        "绑定层帧应被过滤：{rec:?}"
    );
}
