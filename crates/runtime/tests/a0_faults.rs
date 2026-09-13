//! 故障注入验收（docs/architecture/08 验证表 A0 行）：
//! 无限循环 / 吞中断 / 长运算、无限分配、初始化失败、接单与记账间崩溃。

mod common;

use std::time::{Duration, Instant};

use common::{demo_world, fixture, host_bin, session};
use ztw_api::harness::{FaultClass, OutcomeKind, SessionConfig};
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

// ---------------------------------------------------------------------------
// 验收 1：无限循环 / 吞中断 / 长运算
// ---------------------------------------------------------------------------

#[test]
fn infinite_loop_interrupts_and_state_survives() {
    let mut s = session(demo_world());
    assert!(s.load_code(&fixture("infinite_loop.js")).ok);
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    // 运行时内中断生效（quickjs-ng 中断不可捕获）：脚本级错误收场。
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "INTERRUPTED", "应报告运行时内中断：{rec:?}");
    assert!(
        dt < Duration::from_secs(3),
        "中断应在预算附近生效，实际 {dt:?}"
    );
    // 已提交 memory 保留。
    assert_eq!(get_map(&s.memory_snapshot(), "before"), &num(1.0));
    // 世界照常结算并推进；主进程（会话）保持响应。
    assert_eq!(s.world.tick, 1);
    // 恢复：脚本级错误不销毁执行环境，清故障后继续。
    assert!(s.resume_after_script_error());
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script)); // 再次无限循环
    assert!(s.resume_after_script_error());
}

#[test]
fn swallow_attempt_ends_within_bound() {
    let mut s = session(demo_world());
    assert!(s.load_code(&fixture("swallow_interrupt.js")).ok);
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    // quickjs-ng 的中断不可捕获（本机探针验证），预期脚本级中断；
    // 若未来引擎行为变化允许吞掉，看门狗宽限期终止也满足验收。
    match out.kind {
        OutcomeKind::Fault(FaultClass::Script) => {
            assert_eq!(s.fault.as_ref().unwrap().code, "INTERRUPTED");
            assert!(dt < Duration::from_secs(3));
        }
        OutcomeKind::Fault(FaultClass::HostTerminated("watchdog")) => {
            assert!(dt < Duration::from_secs(6));
        }
        other => panic!("意外结局：{other:?}"),
    }
    assert_eq!(s.world.tick, 1, "任一结局下世界都照常结算推进");
    assert_eq!(
        get_map(&s.memory_snapshot(), "sw"),
        &num(1.0),
        "已提交写保留"
    );
}

#[test]
fn long_uninterruptible_op_killed_by_watchdog() {
    // TypedArray.sort 是 C 层 qsort，quickjs-ng 不轮询中断（本机探针验证），
    // 只有主进程看门狗能终止：验证第二层看门狗真实生效。
    // 专属预算：看门狗在 ~400ms 终止，32M 元素排序（秒级）不可能先完成；
    // 初始化放宽到 8s 容纳慢 CI 的填充循环。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.tick_budget_base_ms = 100;
    cfg.grace_ms = 300;
    cfg.init_budget_ms = 8_000;
    let mut s = ztw_api::harness::Session::new(cfg, demo_world());
    assert!(
        s.load_code(&fixture("long_sort.js")).ok,
        "初始化（建大数组）失败"
    );
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("watchdog")),
        "长运算必须由主进程看门狗终止：{:?}",
        s.fault
    );
    // 预算 100ms + 宽限 300ms → 终止延迟应在 ~400ms 之后不久。
    assert!(
        dt >= Duration::from_millis(350) && dt < Duration::from_secs(5),
        "终止延迟 {dt:?}"
    );
    // 已提交状态保留：sort_started 在长运算前写入。
    assert_eq!(get_map(&s.memory_snapshot(), "sort_started"), &num(0.0));
    // 请求关闭点：最后已提交请求是那次 memory 写（docs 验收“记录请求关闭点”）。
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.last_request_id, 1, "关闭点：{rec:?}");
    assert_eq!(rec.last_op, "mem.map_set");
    assert!(rec.message.contains("#1"), "终止报错须含请求编号：{rec:?}");
    // 世界照常推进；宿主重启后恢复。
    assert_eq!(s.world.tick, 1);
    assert!(!s.host_alive());
    let init = s.restart_host();
    assert!(init.ok, "重启失败：{:?}", init.fault);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("watchdog"))
    );
    // 再次 kill 也能恢复。
    assert!(s.restart_host().ok);
}

// ---------------------------------------------------------------------------
// 故障分类防伪造（review 发现）：玩家可构造 InternalError 对象，
// 但触发不了中断处理器——伪造的中断必须按普通脚本错误分类。
// 伪造 OOM 允许自伤（环境重建），不影响他人；见 runtime classify_fault 注释。
// ---------------------------------------------------------------------------

#[test]
fn forged_interrupt_and_oom_classification() {
    let code = r#"
function loop() {
  if (Game.tick === 0) {
    throw Object.assign(new Error("interrupted"), { name: "InternalError" });
  }
  if (Game.tick === 1) {
    throw new InternalError("interrupted");  // 构造器伪造
  }
  throw new Error("real error");
}
"#;
    let mut s = session(demo_world());
    assert!(s.load_code(code).ok);
    for expect in ["forged-1", "forged-2", "real"] {
        let out = s.tick();
        assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script), "{expect}");
        let rec = s.fault.as_ref().unwrap();
        assert_ne!(
            rec.code, "INTERRUPTED",
            "伪造中断不得被分类为运行时内中断（{expect}）：{rec:?}"
        );
        assert!(s.resume_after_script_error(), "{expect}");
    }
    // 伪造 OOM：按环境级处理（自伤路径），宿主存活、可恢复。
    let code = r#"
function loop() {
  throw Object.assign(new Error("out of memory"), { name: "InternalError" });
}
"#;
    let mut s2 = session(demo_world());
    assert!(s2.load_code(code).ok);
    let out = s2.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Environment));
    assert!(s2.reinit_after_env_fault().ok);
}

// ---------------------------------------------------------------------------
// 验收 2：无限分配
// ---------------------------------------------------------------------------

#[test]
fn oom_triggers_env_fault_then_memory_accessible() {
    let mut s = session(demo_world());
    assert!(s.load_code(&fixture("oom_alloc.js")).ok);
    let t0 = Instant::now();
    let out = s.tick();
    let dt = t0.elapsed();
    // JS 内存超限是环境级故障：宿主内销毁执行环境（宿主进程仍存活）。
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Environment));
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "MEMORY_LIMIT");
    assert!(dt < Duration::from_secs(5), "堆上限应快速触发：{dt:?}");
    assert!(s.host_alive(), "环境级故障不重启宿主进程");
    // 世界推进、已提交 memory 保留。
    assert_eq!(s.world.tick, 1);
    assert_eq!(get_map(&s.memory_snapshot(), "oom_mark"), &num(1.0));
    // 恢复：重新初始化（宿主内重建执行环境 + 重新建立 memory 句柄），
    // 无救援序列化、无旧拷贝回退。
    let init = s.reinit_after_env_fault();
    assert!(init.ok, "环境重建失败：{:?}", init.fault);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Environment)); // 脚本继续分配
    assert!(s.reinit_after_env_fault().ok);
    // 换一个温和脚本热重载，验证 memory 与会话完全可用。
    assert!(s.load_code(&fixture("demo_patrol.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_eq!(get_map(&s.memory_snapshot(), "oom_mark"), &num(1.0));
}

#[test]
fn accepted_move_survives_host_termination() {
    // docs/architecture/03：宿主被终止时，已受理动作完好保存在 World 中，
    // 照常结算。宿主在 move 回复送达后硬崩溃（abort 注入）。
    let cfg = SessionConfig::new(host_bin()).with_fault("abort_after_reply:robot.move");
    let mut s = ztw_api::harness::Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("move_then_crash.js")).ok);
    let t0 = Instant::now();
    let out = s.tick();
    assert!(t0.elapsed() < Duration::from_secs(3));
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("crash"))
    );
    // 关闭点：move 是最后已提交请求。
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.last_op, "robot.move");
    assert_eq!(rec.last_request_id, 1);
    // 受理的移动照常结算：机器人已向东一步。
    assert_eq!(s.world.robots[&1].pos, ztw_model::Position::new(2, 1));
    assert_eq!(s.world.tick, 1);
    // 下一 tick 可见结算结果（重启后）。
    assert!(s.restart_host().ok);
    let code = r#"
function loop() {
  const r = Game.robots()[0];
  Game.log("res", r.last_result ? r.last_result.code + ":" + r.last_result.action + ":" + r.last_result.arg : "none");
}
"#;
    assert!(s.load_code(code).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert!(
        s.logs.back().unwrap().1.contains("OK:move:EAST"),
        "上一 tick 的结算结果应可见：{}",
        s.logs.back().unwrap().1
    );
}

// ---------------------------------------------------------------------------
// 验收 3：初始化失败（写 memory 后抛错 / 入口缺失 / 宿主崩溃）
// ---------------------------------------------------------------------------

#[test]
fn init_failure_discards_branch_world_unchanged() {
    // 写 memory 后抛错。
    let mut s = session(demo_world());
    let init = s.load_code(&fixture("init_throw_after_write.js"));
    assert!(!init.ok);
    let rec = init.fault.unwrap();
    assert_eq!(rec.class, FaultClass::Script);
    assert_eq!(rec.code, "Error");
    assert!(rec.message.contains("boom"));
    assert!(rec.stack.contains("<player>"), "应带源码名堆栈：{rec:?}");
    assert_eq!(s.memory_snapshot(), MemValue::Map(vec![]), "临时分支被丢弃");
    assert_eq!(s.world.tick, 0);

    // 入口缺失。
    let mut s2 = session(demo_world());
    let init = s2.load_code(&fixture("init_no_loop.js"));
    assert!(!init.ok);
    assert_eq!(init.fault.unwrap().code, "ENTRY_MISSING");
    assert_eq!(s2.memory_snapshot(), MemValue::Map(vec![]));

    // 宿主在初始化中途硬崩溃（写 memory 之后、完成之前 abort）。
    let cfg = SessionConfig::new(host_bin()).with_fault("abort_init_after_mem");
    let mut s3 = ztw_api::harness::Session::new(cfg, demo_world());
    let init = s3.load_code(&fixture("init_mem_write.js"));
    assert!(!init.ok);
    let rec = init.fault.unwrap();
    assert_eq!(rec.class, FaultClass::HostTerminated("crash"));
    assert_eq!(
        s3.memory_snapshot(),
        MemValue::Map(vec![]),
        "初始化分支不落地"
    );
    assert_eq!(s3.world.tick, 0);
    // 注入按进程生效（env 随会话配置重新下发），因此用干净会话验证
    // 同一代码本身可正常加载提交。
    let mut s4 = session(demo_world());
    assert!(s4.load_code(&fixture("init_mem_write.js")).ok);
    assert_eq!(get_map(&s4.memory_snapshot(), "half"), &num(1.0));
}

// ---------------------------------------------------------------------------
// 验收 4：接单与记账间崩溃 → 订单成立而 memory 记录缺失 → 对账修复
// ---------------------------------------------------------------------------

fn assert_single_take(s: &ztw_api::harness::Session) {
    // 只扣一次款、只成立一单、只剩一档挂单。
    assert_eq!(s.world.gold_milli, 200_000 - 10_000, "买入扣款恰好一次");
    assert_eq!(s.world.my_orders.len(), 1);
    assert_eq!(s.world.listings.len(), 1);
}

#[test]
fn crash_between_take_and_bookkeeping_script_error() {
    let mut s = session(demo_world());
    assert!(s.load_code(&fixture("take_then_throw.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    assert!(s.fault.as_ref().unwrap().message.contains("bookkeeping"));
    // 订单成立、扣款发生、memory 无任务记录。
    assert_single_take(&s);
    assert_eq!(s.memory_snapshot(), MemValue::Map(vec![]), "记账缺失");

    // 示例程序修复：查询真实订单补记账，不重复接单。
    let init = s.load_code(&fixture("repair_demo.js"));
    assert!(init.ok, "修复程序初始化失败：{:?}", init.fault);
    let repaired = s.memory_snapshot();
    let tasks = get_map(&repaired, "tasks");
    let order_id = s.world.my_orders.keys().next().unwrap().to_string();
    let entry = get_map(tasks, &order_id);
    assert_eq!(get_map(entry, "side"), &MemValue::Str("sell".into()));
    assert!(s.logs.iter().any(|(_, l)| l.starts_with("repair")));

    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_single_take(&s); // 不重复接单：金币不再变化
    // 修复程序不再抛错，会话健康。
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
}

#[test]
fn crash_between_take_and_bookkeeping_host_abort() {
    // 宿主硬崩溃（abort）发生在 take 回复送达之后、JS 继续之前。
    let cfg = SessionConfig::new(host_bin()).with_fault("abort_after_reply:market.take");
    let mut s = ztw_api::harness::Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("take_then_hostcrash.js")).ok);
    let t0 = Instant::now();
    let out = s.tick();
    assert!(t0.elapsed() < Duration::from_secs(3));
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("crash"))
    );
    // 订单成立（受理与结算都在主进程），memory 记录缺失。
    assert_single_take(&s);
    assert_eq!(s.memory_snapshot(), MemValue::Map(vec![]));

    // 重启宿主 → 修复程序对账 → 车辆在下一 tick 边界到场。
    let init = s.restart_host();
    assert!(init.ok, "重启失败：{:?}", init.fault);
    // 先跑修复程序。
    let init = s.load_code(&fixture("repair_demo.js"));
    assert!(init.ok);
    let order_id = s.world.my_orders.keys().next().unwrap().to_string();
    assert!(get_map(get_map(&s.memory_snapshot(), "tasks"), &order_id) != &MemValue::Null);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    // take 发生在 tick0：tick1 边界到场。当前已推进到 tick2 之内，车辆应在场。
    assert_eq!(s.world.vehicles.len(), 1, "车辆应已到场");
    assert_single_take(&s);
}
