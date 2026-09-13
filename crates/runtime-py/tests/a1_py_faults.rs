//! A1 Python 宿主资源限制与故障分级验收（docs/architecture/08 验证表：
//! 无限循环/吞中断、无限分配、故障分类防伪造）。与 a0_faults 的 JS 侧
//! 对照，差异点全部来自文档规定的语言语义差异：
//! - KeyboardInterrupt **可被捕获** → 运行时内中断是脚本级错误；
//!   反复吞掉由主进程权威看门狗（第三层）终止兜底。
//! - MemoryError 归**脚本级**（docs 03 故障分级）：解释器与命名空间保留，
//!   热重载重建命名空间后恢复——与 JS OOM 销毁环境不同。

mod common;

use std::time::{Duration, Instant};

use common::{demo_world, host_bin};
use ztw_api::harness::{FaultClass, OutcomeKind, Session, SessionConfig};

#[test]
fn infinite_loop_interrupted_within_budget() {
    let mut s = common::session(demo_world());
    assert!(
        s.load_code("def loop():\n    while True:\n        pass\n")
            .ok
    );
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    // 第二层（宿主内注入）生效：KeyboardInterrupt 在字节码边界抬起，
    // 未被捕获 → 脚本级 INTERRUPTED，远早于主进程宽限上限。
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "INTERRUPTED", "应报告运行时内中断：{rec:?}");
    assert!(dt < Duration::from_secs(3), "中断应发生在预算附近：{dt:?}");
    assert_eq!(s.world.tick, 1, "世界照常结算推进");
    assert!(s.host_alive(), "脚本级错误不杀宿主");
    // 恢复后再次超限，同样中断。
    assert!(s.resume_after_script_error());
    assert_eq!(s.tick().kind, OutcomeKind::Fault(FaultClass::Script));
    assert_eq!(s.fault.as_ref().unwrap().code, "INTERRUPTED");
}

#[test]
fn swallowed_interrupt_terminated_by_main_watchdog() {
    // KeyboardInterrupt 可被捕获（与 JS 中断不可捕获相反）：持续吞掉时
    // 只有第三层（主进程权威看门狗）能收场——诊断文案按语言区分的依据。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.tick_budget_base_ms = 100;
    cfg.grace_ms = 300;
    let mut s = Session::new(cfg, demo_world());
    assert!(
        s.load_code(
            "def loop():\n    while True:\n        try:\n            while True:\n                pass\n        except BaseException:\n            pass\n"
        )
        .ok
    );
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("watchdog")),
        "吞中断必须由主进程看门狗终止：{:?}",
        s.fault
    );
    assert!(
        dt >= Duration::from_millis(350) && dt < Duration::from_secs(5),
        "终止延迟 {dt:?}"
    );
    assert_eq!(s.world.tick, 1, "世界照常结算推进");
    assert!(!s.host_alive());
    // 宿主重启（解释器重建）后恢复。
    assert!(s.restart_host().ok, "重启失败：{:?}", s.restart_host().ok);
}

#[test]
fn unbounded_allocation_raises_memory_error_script_level() {
    // 配额分配器覆盖三域：超额分配返回 NULL → 玩家代码收 MemoryError，
    // 归脚本级（宿主存活、解释器与命名空间保留）。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.heap_limit = 64 * 1024 * 1024; // 压低配额缩短测试时长
    let mut s = Session::new(cfg, demo_world());
    assert!(
        s.load_code(
            "data = []\n\ndef loop():\n    while True:\n        data.append(b'x' * 65536)\n"
        )
        .ok
    );
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::Script),
        "{:?}",
        s.fault
    );
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "MEMORY_LIMIT", "{rec:?}");
    assert!(rec.message.contains("配额分配器"), "{rec:?}");
    // 关键语言差异：MemoryError 是脚本级——宿主存活。
    assert!(s.host_alive(), "Python 内存超限不销毁宿主（与 JS 不同）");
    assert_eq!(s.world.tick, 1, "世界照常结算推进");
    // 命名空间仍持有装满的列表：热重载（重建命名空间）后恢复。
    assert!(s.load_code("def loop():\n    pass\n").ok, "热重载失败");
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
}

#[test]
fn forged_interrupt_and_memory_error_classification() {
    // 玩家可手动 raise，但触发不了看门狗注入 / 分配器拒绝——互证标志
    // 缺失时按普通脚本错误归类（与 JS 侧伪造防护理由一致）。
    let mut s = common::session(demo_world());
    assert!(s.load_code("def loop():\n    raise KeyboardInterrupt\n").ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(
        rec.code, "KeyboardInterrupt",
        "伪造中断不得归类 INTERRUPTED：{rec:?}"
    );
    assert!(!rec.message.contains("超预算"), "{rec:?}");

    let mut s2 = common::session(demo_world());
    assert!(s2.load_code("def loop():\n    raise MemoryError\n").ok);
    let out = s2.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    let rec = s2.fault.as_ref().unwrap();
    assert_eq!(
        rec.code, "MemoryError",
        "伪造 OOM 不得归类 MEMORY_LIMIT：{rec:?}"
    );
    assert!(!rec.message.contains("配额分配器"), "{rec:?}");
}
