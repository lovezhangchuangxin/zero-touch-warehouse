//! A1 Python 宿主空壳冒烟：spawn → init → 多 tick 完成帧闭环与基础故障
//! 分类（docs/architecture/08 原型 A 行）。资源限制三层与中断/内存互证
//! 分类见 a1_py_faults。

mod common;

use common::demo_world;
use ztw_api::harness::{FaultClass, OutcomeKind};

#[test]
fn spawn_init_loop_roundtrip() {
    let mut s = common::session(demo_world());
    let init = s.load_code("def loop():\n    pass\n");
    assert!(init.ok, "初始化失败：{:?}", init.fault);
    for _ in 0..10 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    }
    assert_eq!(s.world.tick, 10);
}

#[test]
fn syntax_error_faults_init_and_world_unchanged() {
    let mut s = common::session(demo_world());
    let init = s.load_code("def loop(:\n    pass\n");
    assert!(!init.ok);
    let rec = init.fault.unwrap();
    assert_eq!(rec.class, FaultClass::Script, "{rec:?}");
    assert_eq!(rec.code, "SyntaxError", "{rec:?}");
    assert!(rec.message.contains("syntax"), "可读语法错误：{rec:?}");
    assert_eq!(s.world.tick, 0, "世界不受初始化失败影响");
    // 宿主存活：直接换源码重试成功。
    let init = s.load_code("def loop():\n    pass\n");
    assert!(init.ok, "重试失败：{:?}", init.fault);
    assert_eq!(s.tick().kind, OutcomeKind::Ok);
}

#[test]
fn entry_missing_is_script_fault() {
    let mut s = common::session(demo_world());
    let init = s.load_code("x = 1\n");
    assert!(!init.ok);
    assert_eq!(init.fault.as_ref().unwrap().code, "ENTRY_MISSING");
    // loop 被覆写为不可调用同样报入口缺失。
    let mut s2 = common::session(demo_world());
    assert!(s2.load_code("def loop():\n    pass\n").ok);
    let init = s2.load_code("loop = 5\n");
    assert!(!init.ok);
    assert_eq!(init.fault.as_ref().unwrap().code, "ENTRY_MISSING");
}

#[test]
fn uncaught_runtime_error_is_script_fault_each_tick() {
    let mut s = common::session(demo_world());
    assert!(
        s.load_code("def loop():\n    raise ValueError('boom')\n")
            .ok
    );
    for _ in 0..3 {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
        let rec = s.fault.as_ref().unwrap();
        assert_eq!(rec.code, "ValueError", "{rec:?}");
        assert!(rec.message.contains("boom"), "{rec:?}");
        assert!(s.resume_after_script_error());
    }
    // 热重载换好代码后恢复。
    assert!(s.load_code("def loop():\n    pass\n").ok);
    assert_eq!(s.tick().kind, OutcomeKind::Ok);
}
