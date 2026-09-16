//! Game 请求 op 分发：per-op 类型化 payload（serde derive）与会话侧执行。
//!
//! 字段契约的单一事实源是 [`OP_FIELDS`]（op → 必填 / 可选字段），集成
//! 测试 tests/payload_parity.rs 以此对账 bootstrap.js / bootstrap.py 的
//! 调用点——同 codes::ALL ↔ codes_parity.rs 的先例。语义约定：
//! - 未知字段忽略（与协议帧层一致，见 protocol.rs）；
//! - 可选字段（give / drop 的 box_id、mem 写的 value）缺失走缺省语义，
//!   错型一律 BAD_PAYLOAD——不再静默降级（此前 box_id 错型会静默走
//!   「当前携带物」语义、mem 写缺 value 会静默写 Null）；
//! - mem 的 gen / node 缺失仍按 u64::MAX 退化走记忆侧错误码
//!   （protocol.rs raw_or_quoted 注释中已文档化的角落）；
//! - payload 契约错误（BAD_PAYLOAD）优先于阶段错误（INIT_PHASE）：
//!   坏负载是绑定层 bug，无论阶段都应以契约错误暴露。附带的两条
//!   同向收紧：重复 JSON 字段被拒（serde 默认，旧 Value 解析为
//!   last-wins）；未知 mem.* op 携带错型字段时报 BAD_PAYLOAD 而非
//!   UNKNOWN_OP（契约错误优先的延伸）。

use serde::Deserialize;
use serde_json::value::RawValue;
use ztw_model::{Id, MemValue, MilliGold, Position};
use ztw_sim::{PATH_NODE_BUDGET, PathOutcome};

use crate::harness::{DiagTapKind, Session};
use crate::memory::{NodeKind, ReadResult};
use crate::mirror::MirrorDelta;
use crate::protocol::{err_result, ok_result, raw_or_quoted};

// ---------------------------------------------------------------------------
// payload 结构（op → 字段的类型化契约；OP_FIELDS 是其清单形式）
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MoveArgs {
    robot_id: Id,
    dx: i32,
    dy: i32,
}

#[derive(Deserialize)]
struct ChargeArgs {
    robot_id: Id,
}

#[derive(Deserialize)]
struct TakeArgs {
    robot_id: Id,
    target_id: Id,
    box_id: Id,
}

#[derive(Deserialize)]
struct GiveArgs {
    robot_id: Id,
    target_id: Id,
    /// 缺省 = 当前携带物；错型 → BAD_PAYLOAD（不静默降级）。
    box_id: Option<Id>,
}

#[derive(Deserialize)]
struct PickArgs {
    robot_id: Id,
    x: i32,
    y: i32,
}

#[derive(Deserialize)]
struct DropArgs {
    robot_id: Id,
    x: i32,
    y: i32,
    /// 缺省 = 当前携带物；错型 → BAD_PAYLOAD（不静默降级）。
    box_id: Option<Id>,
}

/// find_path / move_to 的坐标已由绑定层解析为裸坐标（对象取坐标的规则
/// 见 docs/game-design/08；镜像在宿主本地，解析不产生 IPC）。
#[derive(Deserialize)]
struct FindPathArgs {
    sx: i32,
    sy: i32,
    gx: i32,
    gy: i32,
    /// 到达判定半径；缺省 0（目的格本身须可通行）。
    range: Option<i32>,
}

#[derive(Deserialize)]
struct MoveToArgs {
    robot_id: Id,
    x: i32,
    y: i32,
    /// 到达判定半径；缺省 1（抵达或正交相邻即到达）。
    range: Option<i32>,
}

#[derive(Deserialize)]
struct MarketTakeArgs {
    order_id: Id,
}

#[derive(Deserialize)]
struct MarketCancelArgs {
    order_id: Id,
}

#[derive(Deserialize)]
struct DestroyArgs {
    target_id: Id,
}

#[derive(Deserialize)]
struct BorrowArgs {
    amount_milli: MilliGold,
}

#[derive(Deserialize)]
struct RepayArgs {
    amount_milli: MilliGold,
}

#[derive(Deserialize)]
struct BuyArgs {
    kind: String,
    x: i32,
    y: i32,
}

#[derive(Deserialize)]
struct LogArgs {
    line: String,
}

/// mem.* 共用一个宽结构：字段按 op 用 needs_* 声明必需性（见
/// handle_mem_op）。gen / node 缺失按 u64::MAX 退化（模块文档），
/// value 保留原始 Value 在使用点解码 MemValue（保留既有错误文案）。
#[derive(Deserialize)]
struct MemArgs {
    /// gen 是 Rust 2024 保留字，线字段名经 serde rename 保持。
    #[serde(rename = "gen")]
    r#gen: Option<u64>,
    node: Option<u64>,
    key: Option<String>,
    index: Option<u64>,
    robot_id: Option<Id>,
    value: Option<serde_json::Value>,
}

/// op →（必填字段， 可选字段）：payload 字段契约的单一事实源。
/// tests/payload_parity.rs 以此对账 bootstrap.js / bootstrap.py 调用点，
/// 本模块单测据此校验结构体必填性——新增 / 改动 op 时三处必须同步
/// 落点，任一处漂移即测试失败并指出差集。mem.* 的 gen / node「必填」
/// 指绑定层必发；服务端对缺失仍按 u64::MAX 退化（见模块文档）。
pub const OP_FIELDS: &[(&str, &[&str], &[&str])] = &[
    ("robot.move", &["robot_id", "dx", "dy"], &[]),
    ("robot.charge", &["robot_id"], &[]),
    ("robot.take", &["robot_id", "target_id", "box_id"], &[]),
    ("robot.give", &["robot_id", "target_id"], &["box_id"]),
    ("robot.pick", &["robot_id", "x", "y"], &[]),
    ("robot.drop", &["robot_id", "x", "y"], &["box_id"]),
    ("robot.move_to", &["robot_id", "x", "y"], &["range"]),
    ("game.find_path", &["sx", "sy", "gx", "gy"], &["range"]),
    ("market.take", &["order_id"], &[]),
    ("market.cancel", &["order_id"], &[]),
    ("manage.destroy", &["target_id"], &[]),
    ("manage.borrow", &["amount_milli"], &[]),
    ("manage.repay", &["amount_milli"], &[]),
    ("manage.buy", &["kind", "x", "y"], &[]),
    ("log", &["line"], &[]),
    ("mirror.fetch", &[], &[]),
    ("mem.map_get", &["gen", "node", "key"], &[]),
    ("mem.map_set", &["gen", "node", "key", "value"], &[]),
    ("mem.map_delete", &["gen", "node", "key"], &[]),
    ("mem.map_has", &["gen", "node", "key"], &[]),
    ("mem.map_size", &["gen", "node"], &[]),
    ("mem.map_keys", &["gen", "node"], &[]),
    ("mem.list_get", &["gen", "node", "index"], &[]),
    ("mem.list_set", &["gen", "node", "index", "value"], &[]),
    ("mem.list_append", &["gen", "node", "value"], &[]),
    ("mem.list_remove", &["gen", "node", "index"], &[]),
    ("mem.list_size", &["gen", "node"], &[]),
    ("mem.list_entries", &["gen", "node"], &[]),
    ("mem.to_value", &["gen", "node"], &[]),
    ("mem.robot_memory", &["gen", "robot_id"], &[]),
];

// ---------------------------------------------------------------------------
// op 分发（Session 扩展 impl；调用点在 harness::run_exec 的消息循环）
// ---------------------------------------------------------------------------

impl Session {
    pub(crate) fn handle_op(&mut self, op: &str, payload: &RawValue) -> Box<RawValue> {
        macro_rules! parse {
            ($t:ty) => {
                match serde_json::from_str::<$t>(payload.get()) {
                    Ok(v) => v,
                    Err(e) => return err_result("BAD_PAYLOAD", &format!("{op} {e}")),
                }
            };
        }
        match op {
            "robot.move" => {
                let a = parse!(MoveArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.robot_id));
                }
                let code = self.world.accept_move(a.robot_id, a.dx, a.dy);
                self.diag_accept(op, code, a.robot_id, format!("dx={},dy={}", a.dx, a.dy));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.charge" => {
                let a = parse!(ChargeArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.robot_id));
                }
                let code = self.world.accept_charge(a.robot_id);
                self.diag_accept(op, code, a.robot_id, String::new());
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.take" => {
                let a = parse!(TakeArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.robot_id));
                }
                let code = self.world.accept_take(a.robot_id, a.target_id, a.box_id);
                self.diag_accept(op, code, a.robot_id, format!("box={}", a.box_id));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.give" => {
                let a = parse!(GiveArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.robot_id));
                }
                let code = self.world.accept_give(a.robot_id, a.target_id, a.box_id);
                self.diag_accept(op, code, a.robot_id, format!("box={:?}", a.box_id));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.pick" => {
                let a = parse!(PickArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.robot_id));
                }
                let code = self.world.accept_pick(a.robot_id, a.x, a.y);
                self.diag_accept(op, code, a.robot_id, format!("x={},y={}", a.x, a.y));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.drop" => {
                let a = parse!(DropArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.robot_id));
                }
                let code = self.world.accept_drop(a.robot_id, a.x, a.y, a.box_id);
                self.diag_accept(op, code, a.robot_id, format!("x={},y={}", a.x, a.y));
                ok_result(serde_json::json!({ "code": code }))
            }
            "robot.move_to" => {
                let a = parse!(MoveToArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.robot_id));
                }
                // 负 range 是参数错误而非"不可达"，按动作受理码族拒绝
                //（与 robot.move 的 INVALID_ARGUMENT 同口径）。
                let range = a.range.unwrap_or(1);
                if range < 0 {
                    self.diag_accept(
                        op,
                        ztw_model::codes::INVALID_ARGUMENT,
                        a.robot_id,
                        format!("range={range}"),
                    );
                    return ok_result(serde_json::json!({
                        "code": ztw_model::codes::INVALID_ARGUMENT
                    }));
                }
                match self.move_to_step(a.robot_id, Position::new(a.x, a.y), range) {
                    Ok((code, detail)) => {
                        // ARRIVED 与 OK 同为成功码（08 结果码表）：停驻目的地
                        // 的机器人每 tick 都会 ARRIVED，不得刷 AcceptFail 噪音。
                        if code != ztw_model::codes::OK && code != ztw_model::codes::ARRIVED {
                            self.diag_accept(op, code, a.robot_id, detail);
                        }
                        ok_result(serde_json::json!({ "code": code }))
                    }
                    Err(msg) => err_result("PATH_BUDGET_EXCEEDED", &msg),
                }
            }
            "game.find_path" => {
                // 纯查询：不占行动机会，init 期放行（docs/architecture/03
                // 初始化允许查询）；计算与配额在主进程（03「宿主本地查询
                // 镜像」节明确 find_path 不进镜像）。
                let a = parse!(FindPathArgs);
                let range = a.range.unwrap_or(0);
                if range < 0 {
                    return err_result("INVALID_ARGUMENT", "range 须为非负整数");
                }
                match self.world.find_path(
                    Position::new(a.sx, a.sy),
                    Position::new(a.gx, a.gy),
                    range,
                    PATH_NODE_BUDGET,
                ) {
                    PathOutcome::Path(p) => ok_result(serde_json::json!({
                        "path": p.iter().map(|q| serde_json::json!([q.x, q.y])).collect::<Vec<_>>()
                    })),
                    PathOutcome::AlreadyThere => ok_result(serde_json::json!({ "path": [] })),
                    PathOutcome::Unreachable => ok_result(serde_json::json!({ "path": null })),
                    PathOutcome::BudgetExceeded => err_result(
                        "PATH_BUDGET_EXCEEDED",
                        &format!(
                            "寻路节点预算 {PATH_NODE_BUDGET} 耗尽（start=({},{}) goal=({},{})）",
                            a.sx, a.sy, a.gx, a.gy
                        ),
                    ),
                }
            }
            "market.take" => {
                let a = parse!(MarketTakeArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.order_id));
                }
                let (code, eff) = self.world.manage_take(a.order_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        op,
                        code,
                        Some(a.order_id),
                        format!(
                            "接单 {} {}×{} @{} milli，装卸位 #{}，余额 {} milli",
                            eff.order.side.as_str(),
                            eff.order.goods_type,
                            eff.order.qty,
                            eff.order.unit_price_milli,
                            eff.dock_id,
                            self.world.gold_milli
                        ),
                    );
                    let delta = MirrorDelta::from_take(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        op,
                        code,
                        Some(a.order_id),
                        String::new(),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "market.cancel" => {
                let a = parse!(MarketCancelArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.order_id));
                }
                let (code, eff) = self.world.manage_cancel(a.order_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        op,
                        code,
                        Some(a.order_id),
                        format!(
                            "取消订单，手续费 {} milli，退款 {} milli，移除车辆 {:?}，释放装卸位 {:?}",
                            eff.fee_milli, eff.refund_milli, eff.vehicle_id, eff.dock_id
                        ),
                    );
                    let delta = MirrorDelta::from_cancel(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        op,
                        code,
                        Some(a.order_id),
                        String::new(),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.destroy" => {
                let a = parse!(DestroyArgs);
                if self.in_init {
                    return self.reject_init_phase(op, Some(a.target_id));
                }
                let (code, eff) = self.world.manage_destroy(a.target_id);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        op,
                        code,
                        Some(a.target_id),
                        format!(
                            "销毁 {:?} #{}，退款 {} milli，释放格 {:?}",
                            eff.kind, eff.target_id, eff.refund_milli, eff.freed_cells
                        ),
                    );
                    let delta = MirrorDelta::from_destroy(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        op,
                        code,
                        Some(a.target_id),
                        String::new(),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.borrow" => {
                let a = parse!(BorrowArgs);
                if self.in_init {
                    return self.reject_init_phase(op, None);
                }
                let (code, eff) = self.world.manage_borrow(a.amount_milli);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        op,
                        code,
                        None,
                        format!(
                            "借款 {} milli，余额 {} milli，欠款 {} milli",
                            eff.amount_milli, self.world.gold_milli, self.world.debt_milli
                        ),
                    );
                    let delta = MirrorDelta::from_borrow(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        op,
                        code,
                        None,
                        format!("amount={}", a.amount_milli),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.repay" => {
                let a = parse!(RepayArgs);
                if self.in_init {
                    return self.reject_init_phase(op, None);
                }
                let (code, eff) = self.world.manage_repay(a.amount_milli);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        op,
                        code,
                        None,
                        format!(
                            "归还 {} milli，余额 {} milli，欠款 {} milli",
                            eff.amount_milli, self.world.gold_milli, self.world.debt_milli
                        ),
                    );
                    let delta = MirrorDelta::from_repay(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        op,
                        code,
                        None,
                        format!("amount={}", a.amount_milli),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "manage.buy" => {
                let a = parse!(BuyArgs);
                if self.in_init {
                    return self.reject_init_phase(op, None);
                }
                let (code, eff) = self.world.manage_buy(&a.kind, a.x, a.y);
                if code == ztw_model::codes::OK {
                    let eff = eff.expect("OK 必带影响摘要");
                    self.diag_push(
                        DiagTapKind::Manage,
                        op,
                        code,
                        Some(eff.id),
                        format!(
                            "购入 {:?} #{} @({},{})，支出 {} milli，余额 {} milli",
                            eff.kind,
                            eff.id,
                            eff.pos.x,
                            eff.pos.y,
                            eff.price_milli,
                            self.world.gold_milli
                        ),
                    );
                    let delta = MirrorDelta::from_buy(&self.world, &eff);
                    self.mgmt_ok(delta)
                } else {
                    self.diag_push(
                        DiagTapKind::AcceptFail,
                        op,
                        code,
                        None,
                        format!("kind={} at({},{})", a.kind, a.x, a.y),
                    );
                    ok_result(serde_json::json!({ "code": code }))
                }
            }
            "log" => {
                let a = parse!(LogArgs);
                if a.line.len() > self.cfg.log_entry_limit {
                    return err_result(
                        "LOG_LIMIT",
                        &format!(
                            "日志条目 {}B 超上限 {}B",
                            a.line.len(),
                            self.cfg.log_entry_limit
                        ),
                    );
                }
                self.logs.push_back((self.world.tick, a.line));
                while self.logs.len() > self.cfg.log_ring_cap {
                    self.logs.pop_front();
                }
                ok_result(serde_json::json!({}))
            }
            "mirror.fetch" => {
                // 回退重建必须反映当前世界（含本 tick 已生效管理操作），
                // 不能复用 tick 开始的旧镜像——镜像是世界状态的纯函数
                //（docs/architecture/03 宿主本地查询镜像）。v4 起镜像作为
                // 原始 JSON 值内嵌（服务端 serde 产物，拼接安全），绑定层
                // 免一层 stringify+parse。payload 恒为空对象，不解析。
                let mirror = self.produce_mirror();
                raw_or_quoted(format!("{{\"ok\":true,\"mirror\":{mirror}}}"))
            }
            _ if op.starts_with("mem.") => self.handle_mem_op(op, payload),
            _ => err_result("UNKNOWN_OP", &format!("未知操作 {op}")),
        }
    }

    pub(crate) fn handle_mem_op(&mut self, op: &str, payload: &RawValue) -> Box<RawValue> {
        let a: MemArgs = match serde_json::from_str(payload.get()) {
            Ok(v) => v,
            Err(e) => return err_result("BAD_PAYLOAD", &format!("{op} {e}")),
        };
        let session_gen = a.r#gen.unwrap_or(u64::MAX);
        let node = a.node.unwrap_or(u64::MAX);
        let needs = mem_needs(op);
        if needs.key && a.key.is_none() {
            return err_result("BAD_PAYLOAD", &format!("{op} 缺少必需参数 key"));
        }
        if needs.index && a.index.is_none() {
            return err_result("BAD_PAYLOAD", &format!("{op} 缺少必需参数 index"));
        }
        if needs.robot && a.robot_id.is_none() {
            return err_result("BAD_PAYLOAD", &format!("{op} 缺少必需参数 robot_id"));
        }
        // 缺 value 此前会静默写 MemValue::Null，收紧为显式契约错误。
        if needs.value && a.value.is_none() {
            return err_result("BAD_PAYLOAD", &format!("{op} 缺少必需参数 value"));
        }
        let value: Option<MemValue> = if needs.value {
            match serde_json::from_value(a.value.clone().expect("needs_value 已检查存在")) {
                Ok(v) => Some(v),
                Err(_) => return err_result("BAD_PAYLOAD", "memory 写入值非法（线值解码失败）"),
            }
        } else {
            None
        };
        let tree = self.memory_target_mut();
        let mut run = || -> Result<serde_json::Value, crate::memory::MemOpError> {
            match op {
                "mem.map_get" => Ok(read_result_json(tree.map_get(
                    session_gen,
                    node,
                    a.key.as_deref().unwrap_or(""),
                )?)),
                "mem.map_set" => {
                    tree.map_set(
                        session_gen,
                        node,
                        a.key.as_deref().unwrap_or(""),
                        value.as_ref().unwrap(),
                    )?;
                    Ok(serde_json::json!({}))
                }
                "mem.map_delete" => {
                    let removed =
                        tree.map_delete(session_gen, node, a.key.as_deref().unwrap_or(""))?;
                    Ok(serde_json::json!({ "value": removed }))
                }
                "mem.map_has" => Ok(serde_json::json!({
                    "value": tree.map_has(session_gen, node, a.key.as_deref().unwrap_or(""))?
                })),
                "mem.map_size" => {
                    Ok(serde_json::json!({ "value": tree.map_size(session_gen, node)? }))
                }
                "mem.map_keys" => {
                    Ok(serde_json::json!({ "keys": tree.map_keys(session_gen, node)? }))
                }
                "mem.list_get" => Ok(read_result_json(tree.list_get(
                    session_gen,
                    node,
                    a.index.map(|i| i as usize).unwrap_or(usize::MAX),
                )?)),
                "mem.list_set" => {
                    tree.list_set(
                        session_gen,
                        node,
                        a.index.map(|i| i as usize).unwrap_or(usize::MAX),
                        value.as_ref().unwrap(),
                    )?;
                    Ok(serde_json::json!({}))
                }
                "mem.list_append" => {
                    tree.list_append(session_gen, node, value.as_ref().unwrap())?;
                    Ok(serde_json::json!({}))
                }
                "mem.list_remove" => {
                    tree.list_remove(
                        session_gen,
                        node,
                        a.index.map(|i| i as usize).unwrap_or(usize::MAX),
                    )?;
                    Ok(serde_json::json!({}))
                }
                "mem.list_size" => {
                    Ok(serde_json::json!({ "value": tree.list_size(session_gen, node)? }))
                }
                "mem.list_entries" => Ok(serde_json::json!({
                    "entries": tree
                        .list_entries(session_gen, node)?
                        .into_iter()
                        .map(read_result_json)
                        .collect::<Vec<_>>()
                })),
                "mem.to_value" => Ok(serde_json::json!({
                    "value": serde_json::to_value(tree.to_value(session_gen, node)?)
                        .unwrap_or(serde_json::Value::Null)
                })),
                "mem.robot_memory" => {
                    let n = tree.robot_memory(session_gen, a.robot_id.unwrap_or(0))?;
                    Ok(serde_json::json!({ "node": n }))
                }
                _ => Err(crate::memory::MemOpError {
                    code: "UNKNOWN_OP",
                    message: format!("未知 memory 操作 {op}"),
                }),
            }
        };
        match run() {
            Ok(fields) => ok_result(fields),
            Err(e) => err_result(e.code, &e.message),
        }
    }

    /// move_to 复合步（docs/game-design/08「move_to 与 robot.memory」）：
    /// 单源实现于主进程（docs/architecture/04——双语言绑定只做参数宽进与
    /// 透传，无第二实现面）。次序：NO_SUCH_OBJECT → 到达判定（ARRIVED，
    /// 优先于行动占用检查）→ ALREADY_ACTED 短路（不寻路不写缓存）→
    /// 缓存校验 / 失效重寻 → 写 `_move` → accept_move 透传受理码（提交的
    /// 就是 move 意图，结算语义零新增）。Err = 寻路预算超限（非结果码）。
    fn move_to_step(
        &mut self,
        robot_id: Id,
        goal: Position,
        range: i32,
    ) -> Result<(&'static str, String), String> {
        let Some(robot) = self.world.robots.get(&robot_id) else {
            return Ok((
                ztw_model::codes::NO_SUCH_OBJECT,
                format!("goal=({},{})", goal.x, goal.y),
            ));
        };
        let pos = robot.pos;
        // 到达判定优先于行动占用检查（文档冻结语义）。i64 距离：goal 是
        // 玩家可控的裸 i32，i32 减法在极值处回绕（审查轮 P0）。
        if (pos.x as i64 - goal.x as i64).abs() + (pos.y as i64 - goal.y as i64).abs()
            <= range as i64
        {
            return Ok((
                ztw_model::codes::ARRIVED,
                format!("goal=({},{}) range={range}", goal.x, goal.y),
            ));
        }
        // 本 tick 已行动：短路省寻路，缓存原样保留（下 tick 原路重试）。
        if self.world.has_pending_intent(robot_id) {
            return Ok((
                ztw_model::codes::ALREADY_ACTED,
                format!("goal=({},{})", goal.x, goal.y),
            ));
        }
        // 缓存校验三失效条件：goal / range 变化；当前位置不在缓存轨迹上
        //（偏离）；下一步被静态障碍占据。缓存的 path 含寻路时起点——结算
        // 失败时位置与索引都不变，天然原路重试；受理成功也不提前消费。
        let cached_next = self
            .memory
            .server_move_cache_get(robot_id)
            .and_then(|v| decode_move_cache(&v))
            .and_then(|(g, r, path)| {
                if g != goal || r != range {
                    return None;
                }
                let i = path.iter().position(|p| *p == pos)?;
                let next = *path.get(i + 1)?;
                (pos.adjacent(&next) && self.world.statically_passable(next)).then_some(next)
            });
        let next = match cached_next {
            Some(n) => n,
            None => match self.world.find_path(pos, goal, range, PATH_NODE_BUDGET) {
                PathOutcome::Path(p) => {
                    // 缓存轨迹含起点（推进语义见上）；限额拒绝降级为不
                    // 缓存（每 tick 重寻），不影响移动本身。
                    let mut trail = Vec::with_capacity(p.len() + 1);
                    trail.push(pos);
                    trail.extend_from_slice(&p);
                    let _ = self
                        .memory
                        .server_move_cache_set(robot_id, &encode_move_cache(goal, range, &trail));
                    p[0]
                }
                PathOutcome::AlreadyThere => {
                    unreachable!("到达判定已先行拦截（dist ≤ range 必先返回 ARRIVED）")
                }
                PathOutcome::Unreachable => {
                    return Ok((
                        ztw_model::codes::NO_PATH,
                        format!("goal=({},{}) range={range}", goal.x, goal.y),
                    ));
                }
                PathOutcome::BudgetExceeded => {
                    return Err(format!(
                        "寻路节点预算 {PATH_NODE_BUDGET} 耗尽（goal=({},{})）",
                        goal.x, goal.y
                    ));
                }
            },
        };
        let code = self
            .world
            .accept_move(robot_id, next.x - pos.x, next.y - pos.y);
        Ok((
            code,
            format!("goal=({},{}) next=({},{})", goal.x, goal.y, next.x, next.y),
        ))
    }

    /// 初始化阶段禁用动作与管理操作（docs/architecture/03），统一记录诊断。
    pub(crate) fn reject_init_phase(
        &mut self,
        op: &str,
        subject: Option<ztw_model::Id>,
    ) -> Box<RawValue> {
        self.diag_push(
            DiagTapKind::AcceptFail,
            op,
            ztw_model::codes::INIT_PHASE,
            subject,
            "初始化阶段禁止动作与管理操作".to_string(),
        );
        ok_result(serde_json::json!({"code": ztw_model::codes::INIT_PHASE}))
    }

    /// 动作受理结果记录：受理失败进诊断；成功受理的最终成败由结算事件
    /// （last_results）承担，不重复记录。
    pub(crate) fn diag_accept(
        &mut self,
        op: &str,
        code: &str,
        robot_id: ztw_model::Id,
        detail: String,
    ) {
        if code != ztw_model::codes::OK {
            self.diag_push(DiagTapKind::AcceptFail, op, code, Some(robot_id), detail);
        }
    }

    /// 管理操作成功路径的统一回复：递增世界修订号并携带镜像同步增量
    /// （docs/architecture/03 管理操作当 tick 可见）。
    pub(crate) fn mgmt_ok(&mut self, delta: MirrorDelta) -> Box<RawValue> {
        self.revision.mgmt_count += 1;
        let delta_json = delta.to_json();
        self.stats.delta_bytes.push(delta_json.len());
        let delta_val: serde_json::Value =
            serde_json::from_str(&delta_json).unwrap_or(serde_json::Value::Null);
        ok_result(serde_json::json!({ "code": ztw_model::codes::OK, "delta": delta_val }))
    }
}

/// mem.* 的字段必需性（key / index / robot_id / value）。OP_FIELDS 的
/// mem 条目与之机械对账（见 op_fields_anchored_to_dispatch 测试）——
/// 新增 mem op 时两处必须同步落点。
struct MemNeeds {
    key: bool,
    index: bool,
    robot: bool,
    value: bool,
}

fn mem_needs(op: &str) -> MemNeeds {
    MemNeeds {
        key: matches!(
            op,
            "mem.map_set" | "mem.map_delete" | "mem.map_has" | "mem.map_get"
        ),
        index: matches!(op, "mem.list_get" | "mem.list_set" | "mem.list_remove"),
        robot: op == "mem.robot_memory",
        value: matches!(op, "mem.map_set" | "mem.list_set" | "mem.list_append"),
    }
}

/// 标量读取返回普通 JSON 值；容器读取返回句柄描述。
/// `_move` 缓存线值编码：JSON 字符串，解析后形如
/// `{"goal":[x,y],"range":n,"path":[[x,y],…]}`，path 为含寻路起点的完整
/// 轨迹（格式冻结点，docs/game-design/08「move_to 与 robot.memory」）。
/// 恒为**标量**（Str）：标量替换不杀容器节点，宿主 memory 槽位缓存的
/// 失效协议（「服务端对玩家树无结构性写」，memory.rs / harness.rs
/// 不变量注释）由此保持成立。
fn encode_move_cache(goal: Position, range: i32, trail: &[Position]) -> MemValue {
    let v = serde_json::json!({
        "goal": [goal.x, goal.y],
        "range": range,
        "path": trail.iter().map(|p| serde_json::json!([p.x, p.y])).collect::<Vec<_>>(),
    });
    MemValue::Str(v.to_string())
}

/// 缓存解码：非字符串或 JSON 形状 / 数值异常（含手改档与旧版本遗留）
/// 一律 None，调用方按缓存 miss 整体重寻——缓存只影响效率，不构成
/// 权威状态（自愈）。
fn decode_move_cache(v: &MemValue) -> Option<(Position, i32, Vec<Position>)> {
    fn num_int(v: &serde_json::Value) -> Option<i32> {
        let n = v.as_f64()?;
        if !n.is_finite() || n.fract() != 0.0 || n < i32::MIN as f64 || n > i32::MAX as f64 {
            return None;
        }
        Some(n as i32)
    }
    fn pos_of(v: &serde_json::Value) -> Option<Position> {
        let items = v.as_array()?;
        if items.len() != 2 {
            return None;
        }
        Some(Position::new(num_int(&items[0])?, num_int(&items[1])?))
    }
    let MemValue::Str(s) = v else {
        return None;
    };
    let root: serde_json::Value = serde_json::from_str(s).ok()?;
    let goal = pos_of(root.get("goal")?)?;
    let range = num_int(root.get("range")?)?;
    if range < 0 {
        return None;
    }
    let path = root
        .get("path")?
        .as_array()?
        .iter()
        .map(pos_of)
        .collect::<Option<Vec<_>>>()?;
    // 空轨迹 = 形状坏（合法轨迹至少含起点与到达格两格）。
    if path.len() < 2 {
        return None;
    }
    Some((goal, range, path))
}

fn read_result_json(r: ReadResult) -> serde_json::Value {
    match r {
        ReadResult::Scalar(v) => serde_json::json!({ "t": "scalar", "v": scalar_plain(&v) }),
        ReadResult::Handle { node, kind } => serde_json::json!({
            "t": "handle",
            "node": node,
            "kind": if kind == NodeKind::Map { "map" } else { "list" },
        }),
        ReadResult::Missing => serde_json::json!({ "t": "missing" }),
    }
}

fn scalar_plain(v: &MemValue) -> serde_json::Value {
    match v {
        MemValue::Null => serde_json::Value::Null,
        MemValue::Bool(b) => serde_json::Value::Bool(*b),
        MemValue::Num(n) => serde_json::json!(n),
        MemValue::Str(s) => serde_json::json!(s),
        _ => serde_json::Value::Null,
    }
}

// ---------------------------------------------------------------------------
// 单测：payload 契约（必填性 / 静默缝闭合 / 故障路径直驱，无需宿主进程）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{Session, SessionConfig};
    use ztw_model::Position;
    use ztw_sim::World;

    fn session() -> Session {
        let mut w = World::new_empty(8, 8, 100_000);
        w.add_robot(Position::new(1, 1));
        Session::new(SessionConfig::new("ztw-host-js"), w)
    }

    fn raw(s: &str) -> Box<RawValue> {
        RawValue::from_string(s.to_string()).expect("测试负载是合法 JSON")
    }

    fn reply(r: &RawValue) -> serde_json::Value {
        serde_json::from_str(r.get()).expect("回复是 JSON")
    }

    fn sample_value(field: &str) -> serde_json::Value {
        match field {
            "kind" => serde_json::json!("shelf"),
            "line" => serde_json::json!("x"),
            "key" => serde_json::json!("k"),
            "node" => serde_json::json!(0),
            // MemValue 是外部标签枚举线值（"Null" / {"Num":1} / …），
            // 与绑定层 toWire 产物同构。
            "value" => serde_json::json!({ "Num": 1 }),
            _ => serde_json::json!(1),
        }
    }

    /// 与样例值类型相反的错型值（字符串 ↔ 数字），用于错型维度对账。
    fn wrong_value(v: &serde_json::Value) -> serde_json::Value {
        if v.is_string() {
            serde_json::json!(1)
        } else {
            serde_json::json!("x")
        }
    }

    /// op → 对应结构体的解析（表驱动测试的类型登记点）。
    fn parse_named(op: &str, json: &str) -> Result<(), String> {
        macro_rules! arm {
            ($t:ty) => {
                return serde_json::from_str::<$t>(json)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            };
        }
        match op {
            "robot.move" => arm!(MoveArgs),
            "robot.charge" => arm!(ChargeArgs),
            "robot.take" => arm!(TakeArgs),
            "robot.give" => arm!(GiveArgs),
            "robot.pick" => arm!(PickArgs),
            "robot.drop" => arm!(DropArgs),
            "robot.move_to" => arm!(MoveToArgs),
            "game.find_path" => arm!(FindPathArgs),
            "market.take" => arm!(MarketTakeArgs),
            "market.cancel" => arm!(MarketCancelArgs),
            "manage.destroy" => arm!(DestroyArgs),
            "manage.borrow" => arm!(BorrowArgs),
            "manage.repay" => arm!(RepayArgs),
            "manage.buy" => arm!(BuyArgs),
            "log" => arm!(LogArgs),
            "mirror.fetch" => Ok(()), // 不解析 payload
            _ => panic!("OP_FIELDS 含未登记的 op {op}"),
        }
    }

    /// 表驱动必填性：全集可解析；逐个去掉必填字段 → 错误点名该字段；
    /// 去掉可选字段 → 仍可解析。
    #[test]
    fn named_op_required_fields_enforced() {
        for (op, required, optional) in OP_FIELDS.iter().filter(|(o, _, _)| !o.starts_with("mem."))
        {
            let mut full = serde_json::Map::new();
            for f in required.iter().chain(optional.iter()) {
                full.insert((*f).to_string(), sample_value(f));
            }
            let full = serde_json::Value::Object(full);
            if let Err(e) = parse_named(op, &full.to_string()) {
                panic!("{op} 全集应可解析：{e}");
            }

            for f in required.iter() {
                let mut v = full.clone();
                v.as_object_mut().unwrap().remove(*f);
                let err = parse_named(op, &v.to_string()).expect_err("缺必填应报错");
                assert!(err.contains(f), "{op} 缺 {f} 的错误应点名字段：{err}");
            }
            for f in optional.iter() {
                let mut v = full.clone();
                v.as_object_mut().unwrap().remove(*f);
                assert!(
                    parse_named(op, &v.to_string()).is_ok(),
                    "{op} 缺可选 {f} 应仍可解析"
                );
            }
        }
    }

    /// OP_FIELDS ↔ 分发臂 / mem 必填性执法的机械锚点：表内每个 op
    /// 直驱 handle_op，全字段负载必须命中真实分发臂（UNKNOWN_OP 即
    /// OP_FIELDS 与 handle_op / mem_needs 脱节）；缺必填 / 必填报错型
    /// 都是 BAD_PAYLOAD 且点名字段。mem 的 gen / node 缺失按 MAX 退化
    /// （protocol.rs 文档化语义）；value 是外部标签线值（复合结构，
    /// 非法线值另有专门文案），两者不参与错型维度。
    #[test]
    fn op_fields_anchored_to_dispatch() {
        let mut s = session();
        for (op, required, optional) in OP_FIELDS {
            let mut full = serde_json::Map::new();
            for f in required.iter().chain(optional.iter()) {
                full.insert((*f).to_string(), sample_value(f));
            }
            let full = serde_json::Value::Object(full);
            let v = reply(&s.handle_op(op, &raw(&full.to_string())));
            assert_ne!(
                v["code"],
                serde_json::json!("UNKNOWN_OP"),
                "{op} 无分发臂（OP_FIELDS 与 handle_op / mem_needs 脱节）"
            );
            let msg = v["message"].as_str().unwrap_or("");
            assert!(
                !(v["code"] == serde_json::json!("BAD_PAYLOAD") && msg.contains("缺少必需参数")),
                "{op} 全字段负载不应缺参：{msg}"
            );

            for f in required.iter() {
                let mut p = full.clone();
                p.as_object_mut().unwrap().remove(*f);
                let v = reply(&s.handle_op(op, &raw(&p.to_string())));
                if op.starts_with("mem.") && (*f == "gen" || *f == "node") {
                    // 缺失按 MAX 退化走记忆侧错误码，不得是 BAD_PAYLOAD。
                    assert_ne!(
                        v["code"],
                        serde_json::json!("BAD_PAYLOAD"),
                        "{op} 缺 {f} 应保留 MAX 退化（protocol.rs 文档化语义）"
                    );
                } else {
                    assert_eq!(
                        v["code"],
                        serde_json::json!("BAD_PAYLOAD"),
                        "{op} 缺 {f} 应报 BAD_PAYLOAD：{}",
                        v["message"].as_str().unwrap_or("")
                    );
                    assert!(
                        v["message"].as_str().unwrap().contains(*f),
                        "{op} 缺 {f} 的错误应点名字段"
                    );
                }
                // 错型维度：必填字段换成相反类型 → BAD_PAYLOAD（mem 的
                // value 除外——外部标签线值自身即复合结构）。
                if *f != "value" {
                    let mut p = full.clone();
                    p.as_object_mut()
                        .unwrap()
                        .insert((*f).to_string(), wrong_value(&sample_value(f)));
                    let v = reply(&s.handle_op(op, &raw(&p.to_string())));
                    assert_eq!(
                        v["code"],
                        serde_json::json!("BAD_PAYLOAD"),
                        "{op} 的 {f} 错型应报 BAD_PAYLOAD"
                    );
                }
            }
        }
        // 表外 mem op 兜底仍是 UNKNOWN_OP。
        let v = reply(&s.handle_op("mem.map_has_typo", &raw(r#"{"gen":1,"node":0,"key":"k"}"#)));
        assert_eq!(v["code"], serde_json::json!("UNKNOWN_OP"));
    }

    /// 静默缝回归：give 的 box_id 错型必须显式报错，不再静默走携带物；
    /// 缺省（无 box_id）仍走携带物语义。
    #[test]
    fn give_box_id_wrong_type_is_bad_payload() {
        let mut s = session();
        let r = s.handle_op(
            "robot.give",
            &raw(r#"{"robot_id":1,"target_id":2,"box_id":"3"}"#),
        );
        let v = reply(&r);
        assert_eq!(v["code"], serde_json::json!("BAD_PAYLOAD"));
        let r2 = s.handle_op("robot.give", &raw(r#"{"robot_id":1,"target_id":2}"#));
        assert_eq!(reply(&r2)["ok"], serde_json::json!(true));
    }

    /// 静默缝回归：mem 写缺 value 必须显式报错，不再静默写 Null。
    #[test]
    fn mem_set_missing_value_is_bad_payload() {
        let mut s = session();
        let r = s.handle_op("mem.map_set", &raw(r#"{"gen":0,"node":0,"key":"k"}"#));
        let v = reply(&r);
        assert_eq!(v["code"], serde_json::json!("BAD_PAYLOAD"));
        assert!(v["message"].as_str().unwrap().contains("value"));
    }

    /// 错型 gen（原 MAX 退化路径）收紧为 BAD_PAYLOAD。
    #[test]
    fn mem_wrong_type_gen_is_bad_payload() {
        let mut s = session();
        let r = s.handle_op("mem.map_size", &raw(r#"{"gen":"0","node":0}"#));
        assert_eq!(reply(&r)["code"], serde_json::json!("BAD_PAYLOAD"));
        // 缺失 gen 仍按 MAX 退化（走到记忆侧错误码，非 BAD_PAYLOAD）。
        let r2 = s.handle_op("mem.map_size", &raw(r#"{"node":0}"#));
        assert_ne!(reply(&r2)["code"], serde_json::json!("BAD_PAYLOAD"));
    }

    /// 契约错误优先于阶段错误：坏 payload 在 init 期报 BAD_PAYLOAD；
    /// 良构 payload 在 init 期仍报 INIT_PHASE（语义不变）。
    #[test]
    fn bad_payload_takes_priority_over_init_phase() {
        let mut s = session();
        s.in_init = true;
        let r = s.handle_op("robot.move", &raw(r#"{"robot_id":1,"dx":1}"#));
        assert_eq!(reply(&r)["code"], serde_json::json!("BAD_PAYLOAD"));
        let r2 = s.handle_op("robot.move", &raw(r#"{"robot_id":1,"dx":1,"dy":0}"#));
        assert_eq!(
            reply(&r2)["code"],
            serde_json::json!(ztw_model::codes::INIT_PHASE)
        );
    }

    /// 越界坐标不再经 as i32 静默回绕；正常受理路径不受影响。
    #[test]
    fn move_happy_path_and_out_of_range_dx() {
        let mut s = session();
        let r = s.handle_op("robot.move", &raw(r#"{"robot_id":1,"dx":1,"dy":0}"#));
        assert_eq!(reply(&r)["ok"], serde_json::json!(true));
        let big: i64 = 1 << 40;
        let r2 = s.handle_op(
            "robot.move",
            &raw(&format!(r#"{{"robot_id":1,"dx":{big},"dy":0}}"#)),
        );
        assert_eq!(reply(&r2)["code"], serde_json::json!("BAD_PAYLOAD"));
    }

    #[test]
    fn market_take_wrong_type_order_id_is_bad_payload() {
        let mut s = session();
        let r = s.handle_op("market.take", &raw(r#"{"order_id":"7"}"#));
        let v = reply(&r);
        assert_eq!(v["code"], serde_json::json!("BAD_PAYLOAD"));
    }

    #[test]
    fn log_roundtrip_and_missing_line() {
        let mut s = session();
        let r = s.handle_op("log", &raw(r#"{"line":"hello"}"#));
        assert_eq!(reply(&r)["ok"], serde_json::json!(true));
        assert!(s.logs.iter().any(|(_, l)| l == "hello"));
        let r2 = s.handle_op("log", &raw("{}"));
        assert_eq!(reply(&r2)["code"], serde_json::json!("BAD_PAYLOAD"));
    }

    /// 未知 op 与非对象负载的兜底形态。
    #[test]
    fn unknown_op_and_non_object_payload() {
        let mut s = session();
        let r = s.handle_op("robot.fly", &raw("{}"));
        assert_eq!(reply(&r)["code"], serde_json::json!("UNKNOWN_OP"));
        let r2 = s.handle_op("robot.move", &raw("[1,2]"));
        assert_eq!(reply(&r2)["code"], serde_json::json!("BAD_PAYLOAD"));
    }

    // -- move_to / find_path（M4；语义见 docs/game-design/08 两节） ------------

    fn move_to(s: &mut Session, x: i32, y: i32, range: Option<i32>) -> serde_json::Value {
        let range_json = match range {
            Some(r) => format!(r#","range":{r}"#),
            None => String::new(),
        };
        let r = s.handle_op(
            "robot.move_to",
            &raw(&format!(r#"{{"robot_id":1,"x":{x},"y":{y}{range_json}}}"#)),
        );
        reply(&r)
    }

    fn cached_trail(s: &mut Session) -> Option<Vec<Position>> {
        s.memory
            .server_move_cache_get(1)
            .and_then(|v| decode_move_cache(&v))
            .map(|(_, _, path)| path)
    }

    /// 到达判定优先于行动占用检查：本 tick 已行动且已到达 → ARRIVED。
    #[test]
    fn move_to_arrived_takes_priority_over_already_acted() {
        let mut s = session(); // 机器人 1 在 (1,1)
        assert_eq!(
            reply(&s.handle_op("robot.move", &raw(r#"{"robot_id":1,"dx":1,"dy":0}"#)))["code"],
            serde_json::json!("OK") // 先占用行动机会
        );
        assert_eq!(
            move_to(&mut s, 2, 1, None), // 距离 1 ≤ 默认 range 1 → 已到达
            serde_json::json!({ "ok": true, "code": "ARRIVED" })
        );
    }

    /// 首次调用写缓存（含起点的完整轨迹）；受理成功不提前消费——settle 前
    /// 缓存与修订号都不再变化；ALREADY_ACTED 短路不寻路不写缓存。
    #[test]
    fn move_to_first_call_writes_cache_and_does_not_preconsume() {
        let mut s = session();
        assert_eq!(move_to(&mut s, 3, 1, None)["code"], serde_json::json!("OK"));
        let trail = cached_trail(&mut s).expect("首次调用应写缓存");
        // 默认 range 1：到达集含 (2,1)（与 goal 相邻即到达），轨迹止于 (2,1)。
        assert_eq!(trail, vec![Position::new(1, 1), Position::new(2, 1)]);
        let rev = s.memory.revision();
        // 未结算的重复调用：未到达时 ALREADY_ACTED，缓存原样。
        assert_eq!(
            move_to(&mut s, 5, 1, None)["code"], // 新目的地未到达
            serde_json::json!("ALREADY_ACTED")
        );
        assert_eq!(s.memory.revision(), rev, "短路不得写缓存");
        assert_eq!(cached_trail(&mut s), Some(trail), "受理成功不提前消费路径");
    }

    /// 跨 tick 按实际位置推进缓存轨迹，走完全程以 ARRIVED 收尾。
    #[test]
    fn move_to_walks_full_trail_across_ticks() {
        let mut s = session(); // (1,1) → (5,1)，默认 range 1（到 (4,1) 即到达）
        for _ in 0..3 {
            assert_eq!(move_to(&mut s, 5, 1, None)["code"], serde_json::json!("OK"));
            s.world.settle();
        }
        assert_eq!(s.world.robots[&1].pos, Position::new(4, 1));
        assert_eq!(
            move_to(&mut s, 5, 1, None)["code"],
            serde_json::json!("ARRIVED")
        );
        assert!(!s.world.has_pending_intent(1), "ARRIVED 不占行动机会");
    }

    /// 缓存失效条件「下一步被静态障碍占据」：range 0 走到 goal 本身，加墙
    /// 挡原路 → 重寻绕行并更新缓存轨迹（方向序北优先，绕 y=0）。
    #[test]
    fn move_to_replans_when_next_step_blocked() {
        let mut s = session();
        assert_eq!(
            move_to(&mut s, 5, 1, Some(0))["code"],
            serde_json::json!("OK")
        );
        s.world.settle(); // 机器人到 (2,1)，缓存下一步 (3,1)
        s.world.add_wall(Position::new(3, 1));
        assert_eq!(
            move_to(&mut s, 5, 1, Some(0))["code"],
            serde_json::json!("OK")
        );
        let trail = cached_trail(&mut s).expect("重寻应更新缓存");
        assert_eq!(trail.first(), Some(&Position::new(2, 1)));
        assert!(
            !trail.iter().any(|p| *p == Position::new(3, 1)),
            "新轨迹不得再穿墙格"
        );
        assert_eq!(trail.last(), Some(&Position::new(5, 1)));
    }

    /// 缓存失效条件「当前位置偏离」：位置不在缓存轨迹上 → 整体重寻。
    #[test]
    fn move_to_replans_on_deviation() {
        let mut s = session();
        assert_eq!(move_to(&mut s, 3, 1, None)["code"], serde_json::json!("OK"));
        s.world.settle();
        // 玩家手动移动（或读档回退）到轨迹之外。
        s.world.robots.get_mut(&1).unwrap().pos = Position::new(6, 6);
        assert_eq!(move_to(&mut s, 3, 1, None)["code"], serde_json::json!("OK"));
        let trail = cached_trail(&mut s).expect("偏离应重寻");
        assert_eq!(trail.first(), Some(&Position::new(6, 6)));
    }

    /// 结算失败（CELL_CONTESTED）不使缓存失效：下一 tick 原路重试——
    /// 位置与索引都不变，缓存与修订号原样，重试同一步。
    #[test]
    fn move_to_settlement_failure_retries_same_step() {
        let mut s = session(); // 机器人 1 在 (1,1)（id 更小，落点争抢必胜）
        let r2 = s.world.add_robot(Position::new(3, 1)); // id 2
        assert_eq!(
            reply(&s.handle_op("robot.move", &raw(r#"{"robot_id":1,"dx":1,"dy":0}"#)))["code"],
            serde_json::json!("OK")
        ); // 1 号抢 (2,1)
        let rm = s.handle_op(
            "robot.move_to",
            &raw(&format!(r#"{{"robot_id":{r2},"x":1,"y":1,"range":0}}"#)),
        );
        assert_eq!(reply(&rm)["code"], serde_json::json!("OK"));
        let res = s.world.settle();
        assert_eq!(res[&r2].code, ztw_model::codes::CELL_CONTESTED);
        assert_eq!(s.world.robots[&r2].pos, Position::new(3, 1));
        let before = s.memory.server_move_cache_get(r2);
        let rev = s.memory.revision();
        // 下一 tick 原路重试：缓存未重写（修订号不变），仍提交同一步。
        let rm2 = s.handle_op(
            "robot.move_to",
            &raw(&format!(r#"{{"robot_id":{r2},"x":1,"y":1,"range":0}}"#)),
        );
        assert_eq!(reply(&rm2)["code"], serde_json::json!("OK"));
        assert_eq!(s.memory.revision(), rev, "原路重试不得重写缓存");
        assert_eq!(s.memory.server_move_cache_get(r2), before);
    }

    /// 目的地不可达返回 NO_PATH 且不占行动机会。
    #[test]
    fn move_to_no_path_when_enclosed() {
        let mut s = session();
        for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
            s.world.add_wall(Position::new(1 + dx, 1 + dy)); // 围死 (1,1)
        }
        assert_eq!(
            move_to(&mut s, 6, 6, None)["code"],
            serde_json::json!("NO_PATH")
        );
        assert!(!s.world.has_pending_intent(1), "NO_PATH 不占行动机会");
        // 缓存不写入（无可达路径可缓存）。
        assert_eq!(cached_trail(&mut s), None);
    }

    /// 目的地或 opts（range）变化 → 缓存失效重寻。
    #[test]
    fn move_to_goal_or_range_change_invalidates_cache() {
        let mut s = session();
        assert_eq!(
            move_to(&mut s, 3, 1, Some(0))["code"],
            serde_json::json!("OK")
        );
        s.world.settle(); // (2,1)，缓存 trail [(2,1),(3,1)] goal (3,1) range 0
        assert_eq!(move_to(&mut s, 3, 3, None)["code"], serde_json::json!("OK"));
        let trail = cached_trail(&mut s).expect("换目的地应重寻");
        assert_eq!(trail.first(), Some(&Position::new(2, 1)));
        // 默认 range 1：到达格是 (3,3) 的邻格——同距两邻中方向序南先
        // 发现（经 (2,2) 到 (2,3)），不必走到 goal 本身。
        assert_eq!(trail.last(), Some(&Position::new(2, 3)));
    }

    /// 负 range 是参数错误（INVALID_ARGUMENT），不是「不可达」。
    #[test]
    fn move_to_negative_range_is_invalid_argument() {
        let mut s = session();
        assert_eq!(
            move_to(&mut s, 3, 1, Some(-1))["code"],
            serde_json::json!("INVALID_ARGUMENT")
        );
        // find_path 是查询：参数错误走 GameError（ok:false）。
        let v = reply(&s.handle_op(
            "game.find_path",
            &raw(r#"{"sx":1,"sy":1,"gx":3,"gy":1,"range":-1}"#),
        ));
        assert_eq!(v["ok"], serde_json::json!(false));
        assert_eq!(v["code"], serde_json::json!("INVALID_ARGUMENT"));
    }

    /// find_path 三分支：路径 / 已在到达范围（[]）/ 不可达（null）。
    #[test]
    fn find_path_three_branches() {
        let mut s = session();
        let v = reply(&s.handle_op("game.find_path", &raw(r#"{"sx":1,"sy":1,"gx":3,"gy":1}"#)));
        assert_eq!(v["path"], serde_json::json!([[2, 1], [3, 1]]));
        let v = reply(&s.handle_op(
            "game.find_path",
            &raw(r#"{"sx":1,"sy":1,"gx":2,"gy":1,"range":1}"#),
        ));
        assert_eq!(v["path"], serde_json::json!([]));
        for (dx, dy) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
            s.world.add_wall(Position::new(1 + dx, 1 + dy));
        }
        let v = reply(&s.handle_op("game.find_path", &raw(r#"{"sx":1,"sy":1,"gx":6,"gy":6}"#)));
        assert_eq!(v["path"], serde_json::json!(null));
    }

    /// init 期：find_path 放行（查询），move_to 拒 INIT_PHASE（动作）。
    #[test]
    fn init_phase_allows_find_path_rejects_move_to() {
        let mut s = session();
        s.in_init = true;
        let v = reply(&s.handle_op("game.find_path", &raw(r#"{"sx":1,"sy":1,"gx":3,"gy":1}"#)));
        assert_eq!(v["ok"], serde_json::json!(true));
        assert_eq!(
            move_to(&mut s, 3, 1, None)["code"],
            serde_json::json!("INIT_PHASE")
        );
    }

    /// 缓存写入撞 memory 限额 → 静默降级为不缓存，移动照常。
    #[test]
    fn move_to_cache_write_limit_degrades_silently() {
        // 限额在 Session 构造时拷入 MemoryTree，须从 cfg 压入。
        let mut w = World::new_empty(8, 8, 100_000);
        w.add_robot(Position::new(1, 1));
        let mut cfg = SessionConfig::new("ztw-host-js");
        cfg.memory.max_bytes = 8; // 轨迹必然写不下
        let mut s = Session::new(cfg, w);
        assert_eq!(move_to(&mut s, 3, 1, None)["code"], serde_json::json!("OK"));
        assert_eq!(cached_trail(&mut s), None, "限额拒绝应降级为不缓存");
        // 移动本身不受影响，后续每 tick 重寻（range 0 确保未到达）。
        s.world.settle();
        assert_eq!(
            move_to(&mut s, 5, 1, Some(0))["code"],
            serde_json::json!("OK")
        );
    }
}
