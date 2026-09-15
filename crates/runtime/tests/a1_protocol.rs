//! A1 协议验收（docs/architecture/08「旧消息与重复消息」「IPC 故障注入」行）：
//! 重复请求不重复执行；异负载同号拒绝；旧 epoch / 已关闭执行拒绝；
//! 正常完成与强制关闭竞态只结算一次；三时点断连后已提交状态一致、重启不重放。

mod common;

use common::{demo_world, fixture, host_bin};
use ztw_api::harness::{FaultClass, OutcomeKind, Session, SessionConfig};
use ztw_model::Position;

fn fault_session(faults: &str) -> Session {
    Session::new(
        SessionConfig::new(host_bin()).with_fault(faults),
        demo_world(),
    )
}

fn assert_single_take(s: &Session) {
    assert_eq!(s.world.my_orders.len(), 1, "恰好一单");
    assert_eq!(s.world.gold_milli, 200_000 - 10_000, "扣款恰好一次");
    assert_eq!(s.world.listings.len(), 1);
}

// ---------------------------------------------------------------------------
// 重复消息：同号同负载 → 原结果重发，不重复执行
// ---------------------------------------------------------------------------

#[test]
fn duplicate_request_replays_cached_result_without_reexecution() {
    let mut s = fault_session("dup_request:2");
    assert!(s.load_code(&fixture("dup_take.js")).ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Ok,
        "去重路径对正常宿主透明：{:?}",
        s.fault
    );
    // take 只执行一次：一单、一次扣款、一档剩余挂单。
    assert_single_take(&s);
}

#[test]
fn duplicate_request_with_different_payload_is_protocol_fault() {
    let mut s = fault_session("dup_request_corrupt:1");
    assert!(s.load_code(&fixture("dup_log.js")).ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("protocol"))
    );
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "DUP_REQUEST_MISMATCH", "{rec:?}");
    assert!(rec.message.contains("同号异负载"), "{rec:?}");
    assert!(!s.host_alive());
}

// ---------------------------------------------------------------------------
// 旧消息拒绝：旧执行号 → EXEC_CLOSED；旧代次 → STALE_EPOCH
// ---------------------------------------------------------------------------

#[test]
fn old_execution_request_rejected_as_closed() {
    let mut s = fault_session("old_exec_request");
    assert!(s.load_code(&fixture("old_exec_log.js")).ok);
    let out = s.tick();
    // 主进程回 EXEC_CLOSED 错误结果 → 绑定层抛带 code 的异常 → 脚本级错误。
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    let rec = s.fault.as_ref().unwrap();
    assert!(
        rec.message.contains("执行已关闭"),
        "错误链应携带 EXEC_CLOSED 语义：{rec:?}"
    );
    // 被拒绝的请求不得执行：日志环为空。
    assert!(s.logs.is_empty(), "被拒的 log 不应落地：{:?}", s.logs);
    // 宿主存活（可恢复拒绝，不是协议破坏），下一 tick 正常。
    assert!(s.host_alive());
    assert!(s.resume_after_script_error());
}

/// EXEC_CLOSED 拒绝须计入已见号：在途旧执行请求被拒后，宿主捕获异常
/// 继续发号、正常完成——不得被 REQUEST_ID_GAP / 完成帧对账误杀
/// （可恢复拒绝不是协议故障；主进程与字段注释声称的语义一致）。
#[test]
fn exec_closed_rejected_request_keeps_reconciliation() {
    let mut s = fault_session("old_exec_request_first");
    assert!(s.load_code(&fixture("old_exec_recover.js")).ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Ok,
        "被拒后继续发号 + 完成帧对账都应通过：{:?}",
        s.fault
    );
    // 被拒请求不落地；后续正常请求照常生效。
    assert!(
        !s.logs.iter().any(|(_, l)| l.contains("rejected")),
        "{:?}",
        s.logs
    );
    assert!(
        s.logs.iter().any(|(_, l)| l == "after-reject-1")
            && s.logs.iter().any(|(_, l)| l == "after-reject-2"),
        "{:?}",
        s.logs
    );
    // 宿主存活，下一 tick 同款路径继续正常（注入按执行重置：闸门随
    // 新 Exec 帧复位，首请求再次被改写为旧执行号拒绝）。
    assert!(s.host_alive());
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    // 两 tick 各拒一条首请求、各落两条后续日志：恰好 4 条，被拒的
    // "first-rejected" 从不落地。闸门复位失守（改为一次性全局）则
    // tick2 首请求正常执行，条数变 5——此断言钉死复位语义。
    assert_eq!(s.logs.len(), 4, "每 tick 恰两条后续日志：{:?}", s.logs);
    assert!(
        !s.logs.iter().any(|(_, l)| l == "first-rejected"),
        "被拒请求永不落地：{:?}",
        s.logs
    );
}

#[test]
fn stale_epoch_request_kills_host() {
    let mut s = fault_session("stale_epoch");
    assert!(s.load_code(&fixture("old_exec_log.js")).ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("protocol"))
    );
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "STALE_EPOCH", "{rec:?}");
    assert!(!s.host_alive());
    // 旧代次消息不得执行：日志为空。
    assert!(s.logs.is_empty());
    // 注入随会话配置重新下发：重启的新代次（epoch=2）宿主依旧发 epoch=1，
    // 仍被拒绝——旧代次消息永远拒绝，跨代次也不例外。
    let out2 = {
        let init = s.restart_host();
        assert!(init.ok, "重启即重新初始化本身应成功：{:?}", init.fault);
        s.tick()
    };
    assert_eq!(
        out2.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("protocol"))
    );
    assert_eq!(s.fault.as_ref().unwrap().code, "STALE_EPOCH");
    // 干净会话验证同款代码本身可正常运行（排除注入以外的问题）。
    let mut clean = common::session(demo_world());
    assert!(clean.load_code(&fixture("old_exec_log.js")).ok);
    assert_eq!(clean.tick().kind, OutcomeKind::Ok);
    assert!(clean.logs.iter().any(|(_, l)| l == "hello"));
}

// ---------------------------------------------------------------------------
// IPC 故障注入：三时点断连（docs 08 验证表）
// 回复后断连（abort_after_reply）已由 a0_faults 的
// crash_between_take_and_bookkeeping_host_abort 覆盖，此处补齐前两个时点。
// ---------------------------------------------------------------------------

#[test]
fn disconnect_before_send_leaves_request_uncommitted() {
    // take 已提交；log 请求在发出前宿主 abort——主进程从未见过它。
    let mut s = fault_session("abort_before_send:log");
    assert!(s.load_code(&fixture("take_then_log.js")).ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("crash"))
    );
    assert_single_take(&s); // take 的提交不受断连影响
    assert!(
        s.logs.iter().all(|(_, l)| !l.contains("after take")),
        "未送达的请求不得落地：{:?}",
        s.logs
    );
    // 重启恢复。
    assert!(s.restart_host().ok);
}

#[test]
fn disconnect_after_send_before_reply_is_consistent_and_not_replayed() {
    // take 请求发出后立即 abort：主进程可能已提交（竞态两分支都必须一致）。
    let mut s = fault_session("abort_after_send");
    assert!(s.load_code(&fixture("take_if_missing.js")).ok);
    let out = s.tick();
    assert!(
        matches!(
            out.kind,
            OutcomeKind::Fault(FaultClass::HostTerminated("crash" | "write"))
        ),
        "断连症状可能是 EOF 或回复写失败：{:?}",
        out.kind
    );
    // 分支 A（未提交）：无订单、未扣款；分支 B（已提交）：恰好一单。
    let orders = s.world.my_orders.len();
    assert!(orders <= 1, "至多一单：{orders}");
    assert_eq!(
        s.world.gold_milli,
        200_000 - 10_000 * orders as i64,
        "扣款与订单数一致"
    );
    assert!(!s.host_alive());

    // 重启不重放：程序先查真实订单，只有尚未接单才接。
    let init = s.restart_host();
    assert!(init.ok, "重启失败：{:?}", init.fault);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_single_take(&s); // 无论崩溃前是否已提交，最终恰好一单、一次扣款
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    assert_single_take(&s); // 不会重复接单
}

// ---------------------------------------------------------------------------
// 竞态只结算一次：完成帧连发 → 第二帧按旧执行丢弃
// ---------------------------------------------------------------------------

#[test]
fn duplicate_complete_frame_settles_once() {
    let mut s = fault_session("dup_complete");
    assert!(s.load_code(&fixture("move_only.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert_eq!(
        s.world.robots.values().next().unwrap().pos,
        Position::new(2, 1),
        "tick0 东移一步"
    );
    // 第二 tick：上一执行的残留完成帧先到，必须被丢弃（exec id 不符），
    // 等待并处理本执行的真实帧——世界恰好再推进一步（西移往返）。
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Ok,
        "残留完成帧不得冒充本执行结局：{:?}",
        s.fault
    );
    assert_eq!(s.world.tick, 2);
    assert_eq!(
        s.world.robots.values().next().unwrap().pos,
        Position::new(1, 1),
        "两 tick 恰好两次 move（东进西回）"
    );
}

/// 完成后的迟到强杀不产生二次结算，已提交状态保留（竞态收尾侧）。
#[test]
fn late_kill_after_complete_does_not_resettle() {
    let mut s = common::session(demo_world());
    assert!(s.load_code(&fixture("move_only.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
    let (tick_after_settle, gold_after_settle) = (s.world.tick, s.world.gold_milli);
    // 执行已完成后模拟迟到的强制关闭：杀进程不回滚、也不再结算。
    s.kill_host_now();
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("killed")),
        "{:?}",
        s.fault
    );
    assert_eq!(
        s.world.tick,
        tick_after_settle + 1,
        "tick 仍推进（结算照常）"
    );
    assert_eq!(s.world.gold_milli, gold_after_settle);
    // 重启后继续，位置从结算后的状态出发。
    assert!(s.restart_host().ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok);
}

// ---------------------------------------------------------------------------
// 每执行请求数上限：达到上限暂停报错（docs 03）
// ---------------------------------------------------------------------------

#[test]
fn request_limit_pauses_execution() {
    let mut cfg = SessionConfig::new(host_bin());
    cfg.request_limit_per_exec = 3;
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("request_limit.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "REQUEST_LIMIT", "{rec:?}");
    assert_eq!(s.logs.len(), 3, "超限后的请求不落地：{:?}", s.logs);
    // 脚本级暂停可恢复；上限按执行计，恢复后新执行重新计数、再次触限。
    assert!(s.resume_after_script_error());
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    assert_eq!(s.fault.as_ref().unwrap().code, "REQUEST_LIMIT");
}

#[test]
fn request_limit_fault_path_reports_limit_code() {
    // 触限后脚本不捕获绑定层抛错：错误冒泡成 Fault 帧（宿主只报异常名
    // 不携带错误码），结局仍须改判 REQUEST_LIMIT——与捕获后正常完成的
    // 路径一致（docs 03：触限即暂停报超限，脚本级可恢复）。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.request_limit_per_exec = 3;
    let mut s = Session::new(cfg, demo_world());
    assert!(s.load_code(&fixture("request_limit_fault.js")).ok);
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Fault(FaultClass::Script));
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "REQUEST_LIMIT", "Fault 终态同款改判：{rec:?}");
    assert!(rec.message.contains("上限"), "保留超限语义：{rec:?}");
    assert!(
        rec.message.contains("原故障"),
        "原始错误留在 message：{rec:?}"
    );
}

// ---------------------------------------------------------------------------
// 去重缓存字节上界：预算内透明回放；耗尽后降级为 DUP_REQUEST_MISMATCH
// ---------------------------------------------------------------------------

#[test]
fn dedup_cache_within_byte_budget_still_replays() {
    // 显式小预算（足以容纳本执行全部条目）：重放行为与默认配置一致，
    // 字节上界对预算内的去重透明。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.dedup_cache_bytes = 4096;
    let mut s = Session::new(cfg.with_fault("dup_request:1"), demo_world());
    assert!(s.load_code(&fixture("dup_log.js")).ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Ok,
        "预算内重发透明回放：{:?}",
        s.fault
    );
    assert_eq!(s.logs.len(), 1, "不重复执行：{:?}", s.logs);
}

#[test]
fn dedup_cache_byte_exhaustion_degrades_to_dup_mismatch() {
    // 字节预算远小于单条超长日志的指纹+结果：不入缓存，请求本身照常
    // 执行并应答；注入的重发无缓存可回放 → DUP_REQUEST_MISMATCH 终止
    // 宿主。主进程持有世界与已提交 memory，须以协议故障优雅降级，
    // 而非随请求风暴无界堆积镜像级结果（docs 03：结果构造受字节上界）。
    let mut cfg = SessionConfig::new(host_bin());
    cfg.dedup_cache_bytes = 512;
    let mut s = Session::new(cfg.with_fault("dup_request:1"), demo_world());
    assert!(s.load_code(&fixture("dup_big_log.js")).ok);
    let out = s.tick();
    assert_eq!(
        out.kind,
        OutcomeKind::Fault(FaultClass::HostTerminated("protocol"))
    );
    let rec = s.fault.as_ref().unwrap();
    assert_eq!(rec.code, "DUP_REQUEST_MISMATCH", "{rec:?}");
    assert!(
        rec.message.contains("不在本执行结果缓存内"),
        "缓存缺失路径：{rec:?}"
    );
    assert!(!s.host_alive());
    // 未缓存 ≠ 未执行：原请求恰好落地一次（超长日志本身被受理）。
    assert_eq!(s.logs.len(), 1, "{:?}", s.logs);
    assert_eq!(s.logs[0].1.len(), 1600);
}

#[test]
fn market_accepts_objects_per_docs() {
    // docs 04：凡接受 id 的参数同样接受对象本身。JS 侧曾用 Number() 把
    // 视图对象转成 NaN 而违背该承诺（评审修复，与 destroy 的 idOf 对齐）。
    let mut s = Session::new(SessionConfig::new(host_bin()), demo_world());
    assert!(
        s.load_code(
            "export function loop() {\n  const r = Game.robots()[0];\n  Game.log('by_obj ' + (Game.get_object_by_id(r).id === r.id));\n  Game.market.take(Game.market.sell_orders()[0]);\n}\n"
        )
        .ok,
        "{:?}",
        s.fault
    );
    let out = s.tick();
    assert_eq!(out.kind, OutcomeKind::Ok, "{:?}", s.fault);
    assert!(
        s.logs.iter().any(|(_, l)| l == "by_obj true"),
        "{:?}",
        s.logs
    );
    assert_single_take(&s);
}
