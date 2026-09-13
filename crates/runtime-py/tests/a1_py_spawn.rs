//! A1 Python 宿主空壳冒烟：spawn → init → 多 tick 完成帧闭环、基础故障
//! 分类与主进程看门狗（docs/architecture/08 原型 A 行）。
//!
//! 本阶段（提交 2）无 Game 绑定与运行时内中断：无限循环由第三层
//! （主进程权威看门狗）终止是预期行为；运行时内中断随资源限制提交落地。

mod common;

use std::time::Instant;

use common::{demo_world, host_bin};
use ztw_api::harness::{FaultClass, OutcomeKind, Session, SessionConfig};

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

#[test]
fn infinite_loop_killed_by_main_watchdog() {
    // 空壳阶段无运行时内中断：唯一防线是主进程权威看门狗（预算+宽限后
    // SIGKILL）。此测试同时验证宿主重启路径对 Python 进程同样成立。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.tick_budget_base_ms = 100;
    cfg.grace_ms = 300;
    let mut s = Session::new(cfg, demo_world());
    assert!(
        s.load_code("def loop():\n    while True:\n        pass\n")
            .ok
    );
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("watchdog")),
        "{:?}",
        s.fault
    );
    assert!(
        dt >= std::time::Duration::from_millis(350) && dt < std::time::Duration::from_secs(5),
        "终止延迟 {dt:?}"
    );
    assert_eq!(s.world.tick, 1, "世界照常结算推进");
    assert!(!s.host_alive());
    assert!(s.restart_host().ok, "宿主重启失败");
}
