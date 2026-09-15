//! A2 验收：JS 多文件模块 + ESM 化（探针 S1–S12 的进程级锁定）。
//!
//! 覆盖：静态 import 与相对说明符、入口契约（export loop）、缺失模块
//! 可读错误、模块体故障（promise Rejected 分类）、模块内死循环中断、
//! 顶层 await 未完成分类、动态 import 经微任务解析、循环导入活绑定、
//! 环境重建模块态重置、跨 tick 挂起框内恢复、未处理 Promise 拒绝、
//! 热重载换文件集、说明符与文件名校验。

mod common;

use common::{demo_world, session};
use std::collections::BTreeMap;
use ztw_api::harness::{FaultClass, OutcomeKind, PlayerProgram};
use ztw_sim::World;

fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn module_world() -> World {
    demo_world()
}

// ---------------------------------------------------------------------------
// 基础：静态导入 + 入口契约
// ---------------------------------------------------------------------------

#[test]
fn static_imports_and_entry_contract() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[
            (
                "main.js",
                r#"
                import { helper } from "./lib.js";
                import { deep } from "./util/geo.js";
                globalThis.__marks = [helper(1), deep];
                export function loop() { Game.log("run " + helper(2)); }
                "#,
            ),
            (
                "lib.js",
                r#"
                export function helper(x) { return x + 40; }
                Game.log("lib loaded");
                "#,
            ),
            ("util/geo.js", r#"export const deep = 7;"#),
        ]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(
        out.ok && out.has_loop,
        "init 失败: {:?}",
        out.fault.map(|f| (f.code.clone(), f.message.clone()))
    );
    // init 期间已执行导入图（依赖模块体顶层在初始化执行内运行）。
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("lib loaded")),
        "模块体顶层应在 init 执行: {logged:?}"
    );
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok, "tick 失败: {:?}", s.fault);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("run 42")),
        "loop 调用未见: {logged:?}"
    );
}

#[test]
fn missing_module_readable_error() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[(
            "main.js",
            r#"import { x } from "./missing.js"; export function loop() {}"#,
        )]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    let f = out.fault.expect("有故障");
    assert_eq!(f.class, FaultClass::Script);
    assert!(
        f.message.contains("missing.js") || f.stack.contains("missing.js"),
        "错误应指向缺失模块: {f:?}"
    );
}

#[test]
fn bare_specifier_rejected() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[(
            "main.js",
            r#"import lodash from "lodash"; export function loop() {}"#,
        )]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    let f = out.fault.expect("有故障");
    assert!(
        f.message.contains("相对说明符") || f.stack.contains("lodash"),
        "裸名应给可读拒绝: {f:?}"
    );
}

#[test]
fn bad_filename_rejected_at_init() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[
            ("main.js", "export function loop() {}"),
            ("evil.txt", "whatever"),
        ]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    let f = out.fault.expect("有故障");
    assert_eq!(f.code, "PROGRAM_INVALID");

    // 相对路径越界形态。
    let mut s2 = session(module_world());
    let program2 = PlayerProgram::new(
        files(&[
            ("a/../b.js", "export const x = 1;"),
            ("main.js", "export function loop() {}"),
        ]),
        "main.js",
    );
    let out2 = s2.load_program(&program2);
    assert!(!out2.ok);
    assert_eq!(out2.fault.unwrap().code, "PROGRAM_INVALID");
}

// ---------------------------------------------------------------------------
// 故障形态：模块体错误 / 中断 / TLA / 未处理拒绝
// ---------------------------------------------------------------------------

#[test]
fn module_body_throw_classified_as_script_fault() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[
            (
                "main.js",
                r#"import "./boom.js"; export function loop() {}"#,
            ),
            ("boom.js", r#"throw new Error("模块体故障");"#),
        ]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    let f = out.fault.expect("有故障");
    assert_eq!(f.class, FaultClass::Script);
    assert!(f.message.contains("模块体故障"), "模块体错误应上抛: {f:?}");
}

#[test]
fn infinite_loop_in_module_interrupted() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[
            (
                "main.js",
                r#"import "./spin.js"; export function loop() {}"#,
            ),
            ("spin.js", r#"export const x = (() => { for(;;){} })();"#),
        ]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    let f = out.fault.expect("有故障");
    assert_eq!(f.class, FaultClass::Script);
    assert_eq!(f.code, "INTERRUPTED", "模块死循环应被预算中断: {f:?}");
}

#[test]
fn top_level_await_never_resolving_faults_readably() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[(
            "main.js",
            r#"
            const x = await new Promise(() => {});
            export function loop() { return x; }
            "#,
        )]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    let f = out.fault.expect("有故障");
    assert_eq!(f.class, FaultClass::Script);
    assert_eq!(f.code, "ASYNC_INIT_PENDING", "TLA 未完成应可读分类: {f:?}");
}

#[test]
fn unhandled_rejection_faults_readably() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[(
            "main.js",
            r#"
            export function loop() {
                if (Game.tick === 0) {
                    Promise.reject(new Error("异步分支炸了"));
                }
            }
            "#,
        )]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "init 应成功: {:?}", out.fault);
    let t = s.tick();
    assert!(
        matches!(t.kind, OutcomeKind::Fault(FaultClass::Script)),
        "未处理拒绝应为脚本级故障: {:?}",
        s.fault
    );
    let f = s.fault.as_ref().expect("有故障");
    assert_eq!(f.code, "UNHANDLED_REJECTION");
    assert!(f.message.contains("异步分支炸了"), "应附拒绝原因: {f:?}");

    // 有 catch 则不打 fault。
    let mut s2 = session(module_world());
    let program2 = PlayerProgram::new(
        files(&[(
            "main.js",
            r#"
            export function loop() {
                if (Game.tick === 0) {
                    Promise.reject(new Error("被接住"))
                        .catch(e => Game.log("caught " + e.message));
                }
            }
            "#,
        )]),
        "main.js",
    );
    let out2 = s2.load_program(&program2);
    assert!(out2.ok);
    let t2 = s2.tick();
    assert_eq!(
        t2.kind,
        OutcomeKind::Ok,
        "有 catch 不应故障: {:?}",
        s2.fault
    );
    let logged: Vec<String> = s2.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("caught 被接住")),
        "catch 分支应执行: {logged:?}"
    );
}

// ---------------------------------------------------------------------------
// 微任务语义：动态 import / 跨 tick 挂起 / 循环导入
// ---------------------------------------------------------------------------

#[test]
fn dynamic_import_resolves_within_tick() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[
            (
                "main.js",
                r#"
                export function loop() {
                    if (Game.tick === 0) {
                        import("./dyn.js").then(m => Game.log("dyn " + m.v()));
                    }
                }
                "#,
            ),
            ("dyn.js", r#"export function v() { return 7; }"#),
        ]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(out.ok);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok, "tick: {:?}", s.fault);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("dyn 7")),
        "动态 import 应在 tick 内解析: {logged:?}"
    );
}

#[test]
fn cross_tick_suspension_resumes_inside_later_tick() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[(
            "main.js",
            r#"
            let resume = null;
            export async function worker() {
                const v = await new Promise(r => { resume = r; });
                Game.log("resumed " + v + " at " + Game.tick);
            }
            let started = false;
            export function loop() {
                if (!started) { started = true; worker(); }
                if (resume && Game.tick >= 2) {
                    const r = resume; resume = null; r("go");
                }
            }
            "#,
        )]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "init: {:?}", out.fault);
    let t1 = s.tick();
    assert_eq!(
        t1.kind,
        OutcomeKind::Ok,
        "tick1 挂起不应故障: {:?}",
        s.fault
    );
    let t2 = s.tick();
    assert_eq!(t2.kind, OutcomeKind::Ok);
    let t3 = s.tick();
    assert_eq!(t3.kind, OutcomeKind::Ok, "tick3: {:?}", s.fault);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("resumed go at 2")),
        "恢复应发生在 Game.tick==2 执行帧尾部的 drain 内: {logged:?}"
    );
    assert!(
        !logged
            .iter()
            .any(|l| l.contains("resumed go at 0") || l.contains("resumed go at 1")),
        "恢复不得早于 resolver 所在 tick: {logged:?}"
    );
}

#[test]
fn cyclic_imports_live_bindings() {
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[
            (
                "a.js",
                r#"
                import { snapshot } from "./b.js";
                export let count = 0;
                export function inc() { count += 1; }
                export function aSide() { return snapshot(); }
                "#,
            ),
            (
                "b.js",
                r#"
                import { count, inc } from "./a.js";
                export function snapshot() { inc(); return "count=" + count; }
                "#,
            ),
            (
                "main.js",
                r#"
                import { aSide } from "./a.js";
                globalThis.__s1 = aSide();
                globalThis.__s2 = aSide();
                export function loop() { Game.log(globalThis.__s2); }
                "#,
            ),
        ]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "环导入 init: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("count=2")),
        "活绑定应看到最新值: {logged:?}"
    );
}

// ---------------------------------------------------------------------------
// 生命周期：环境重建 / 热重载换文件集
// ---------------------------------------------------------------------------

#[test]
fn env_rebuild_resets_module_state() {
    // 观测通道：counter.js 模块体自增全局计数；若 init 不再重建
    // Runtime（模块图/全局态泄漏跨代），计数会累积而非归 1。
    let counter = r#"globalThis.__loads = (globalThis.__loads || 0) + 1;"#;
    let mk = |tag: &str| {
        PlayerProgram::new(
            files(&[
                (
                    "main.js",
                    format!(
                        r#"
                        import "./counter.js";
                        export function loop() {{ Game.log("{tag} " + globalThis.__loads); }}
                        "#
                    )
                    .as_str(),
                ),
                ("counter.js", counter),
            ]),
            "main.js",
        )
    };
    let mut s = session(module_world());
    let out = s.load_program(&mk("first"));
    assert!(out.ok, "init: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("first") && l.contains("1")),
        "{logged:?}"
    );

    // 热重载（Runtime 重建）：模块态与全局态归零，计数重新从 1 起。
    let re = s.load_program(&mk("reload"));
    assert!(re.ok, "重载: {:?}", re.fault);
    let t2 = s.tick();
    assert_eq!(t2.kind, OutcomeKind::Ok);
    let logged2: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged2
            .iter()
            .any(|l| l.contains("reload") && l.contains("1")),
        "重建后模块态应重置（泄漏会得到 2+）: {logged2:?}"
    );
}

#[test]
fn hot_reload_with_changed_file_set() {
    let mut s = session(module_world());
    let v1 = PlayerProgram::new(
        files(&[
            (
                "main.js",
                r#"import { v } from "./lib.js"; export function loop() { Game.log("v " + v()); }"#,
            ),
            ("lib.js", r#"export function v() { return 1; }"#),
        ]),
        "main.js",
    );
    let out = s.load_program(&v1);
    assert!(out.ok);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(logged.iter().any(|l| l.contains("v 1")), "{logged:?}");

    // 换文件集热重载：lib 内容变化生效。
    let mut v2_files = v1.files.clone();
    v2_files.insert(
        "lib.js".to_string(),
        "export function v() { return 2; }".to_string(),
    );
    let v2 = PlayerProgram::new(v2_files, "main.js");
    let re = s.load_program(&v2);
    assert!(re.ok, "换文件集重载: {:?}", re.fault);
    let t2 = s.tick();
    assert_eq!(t2.kind, OutcomeKind::Ok, "tick2: {:?}", s.fault);
    let logged2: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged2.iter().any(|l| l.contains("v 2")),
        "新 lib 应生效: {logged2:?}"
    );
}

#[test]
fn oversized_program_within_host_read_limit_ok() {
    // 主进程→宿主的 Exec 帧走宿主的宽读取上限（HOST_READ_LIMIT，非
    // frame_limit——后者只约束宿主→主进程请求与 harness 读侧，对 init
    // 帧从无预检，ESM 化不改变该语义）：多文件大文件集照常初始化。
    let mut s = session(module_world());
    let big = "x".repeat(64 * 1024);
    let program = PlayerProgram::new(
        files(&[
            (
                "main.js",
                r#"
                import { big } from "./big.js";
                export function loop() { Game.log("len " + big.length); }
                "#,
            ),
            ("big.js", format!("export const big = \"{big}\";").as_str()),
        ]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "大文件集 init 应成功: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(logged.iter().any(|l| l.contains("len 65536")), "{logged:?}");
}

#[test]
fn runtime_rebinding_entry_reports_entry_missing() {
    // 文档声明（docs/architecture/03 JS 宿主节）：运行期重绑定入口
    //（export let loop = 5）以 ENTRY_MISSING 暴露——namespace 活绑定
    // 语义下每 tick 重查导出。
    let mut s = session(module_world());
    let program = PlayerProgram::new(
        files(&[(
            "main.js",
            r#"
            export let loop = () => {
                Game.log("tick " + Game.tick);
                if (Game.tick === 0) { loop = 5; }
            };
            "#,
        )]),
        "main.js",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "init: {:?}", out.fault);
    let t1 = s.tick();
    assert_eq!(
        t1.kind,
        OutcomeKind::Ok,
        "tick0（重绑定发生在 loop 返回后，本帧不受影响）: {:?}",
        s.fault
    );
    let t2 = s.tick();
    assert!(
        matches!(t2.kind, OutcomeKind::Fault(FaultClass::Script)),
        "重绑定后下一 tick 应报脚本级故障: {:?}",
        s.fault
    );
    let f = s.fault.as_ref().expect("有故障");
    assert_eq!(f.code, "ENTRY_MISSING", "故障码: {f:?}");
}
