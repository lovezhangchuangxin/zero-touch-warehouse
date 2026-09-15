//! A2 验收：Python 多文件模块（内存 finder 与收窄交互 / 生命周期与
//! 驱逐 / 入口与文件名校验）。

mod common;

use common::{demo_world, session};
use ztw_api::harness::{OutcomeKind, PlayerProgram};

#[test]
fn py_player_module_import_with_game() {
    let mut s = session(demo_world());
    // 简化：直接用可运行的两文件形态。
    let program = PlayerProgram::new(
        [
            (
                "main.py".to_string(),
                r#"
import helper
MARK = helper.mark()
def loop():
    Game.log("helper saw Game: " + str(helper.has_game()))
"#
                .to_string(),
            ),
            (
                "helper.py".to_string(),
                r#"
def mark():
    return 7

def has_game():
    return Game is not None
"#
                .to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(
        out.ok && out.has_loop,
        "init 失败: {:?}",
        out.fault.map(|f| (f.code, f.message))
    );
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok, "tick 失败: {:?}", s.fault);
    let logged: Vec<&str> = s.logs.iter().map(|(_, l)| l.as_str()).collect();
    assert!(
        logged.iter().any(|l| l.contains("helper saw Game: True")),
        "日志未见 Game 注入证据: {logged:?}"
    );
}

#[test]
fn py_whitelist_still_enforced_in_player_module() {
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [
            (
                "main.py".to_string(),
                r#"
import helper
def loop():
    Game.log("unreachable")
"#
                .to_string(),
            ),
            ("helper.py".to_string(), "import os\n".to_string()),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(!out.ok, "白名单外 import 竟然成功");
    let fault = out.fault.expect("有故障");
    assert!(
        fault.message.contains("白名单"),
        "错误不可读: {} {}",
        fault.code,
        fault.message
    );
}

#[test]
fn py_async_loop_rejected() {
    let mut s = session(demo_world());
    let out = s.load_code("async def loop():\n    Game.log('no')\n");
    assert!(!out.ok, "async def loop 竟然通过");
    let fault = out.fault.expect("有故障");
    assert_eq!(fault.code, "ASYNC_ENTRY", "故障码不符: {}", fault.code);
}

// ---------------------------------------------------------------------------
// 收窄交互：模块内受限 builtins / 白名单 / 保留名
// ---------------------------------------------------------------------------

#[test]
fn py_module_namespace_gets_restricted_builtins() {
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [
            (
                "main.py".to_string(),
                "import helper\nREPORT = helper.probe()\n\ndef loop():\n    Game.log(REPORT)\n".to_string(),
            ),
            (
                "helper.py".to_string(),
                "def probe():\n    try:\n        open('/etc/passwd')\n        return 'open-allowed'\n    except NameError:\n        return 'open-denied'\n".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "init: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok, "tick: {:?}", s.fault);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("open-denied")),
        "模块内不应拿到 open: {logged:?}"
    );
}

#[test]
fn py_whitelist_stdlib_works_inside_player_module() {
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [
            (
                "main.py".to_string(),
                "import helper\n\ndef loop():\n    Game.log(helper.compute())\n".to_string(),
            ),
            (
                "helper.py".to_string(),
                "import math\n\ndef compute():\n    return 'floor=' + str(math.floor(2.7))\n"
                    .to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "init: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("floor=2")),
        "白名单标准库应可用: {logged:?}"
    );
}

#[test]
fn py_whitelist_named_player_file_rejected() {
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [
            ("main.py".to_string(), "def loop():\n    pass\n".to_string()),
            ("bisect.py".to_string(), "LOADED = True\n".to_string()),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(!out.ok, "白名单名玩家文件应被拒");
    let f = out.fault.expect("有故障");
    assert_eq!(f.code, "PROGRAM_INVALID");
    assert!(
        f.message.contains("bisect") && f.message.contains("冲突"),
        "错误应指出冲突名: {}",
        f.message
    );
}

#[test]
fn py_non_py_or_nested_file_rejected() {
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [
            ("main.py".to_string(), "def loop():\n    pass\n".to_string()),
            ("lib/mod.py".to_string(), "X = 1\n".to_string()),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    assert_eq!(out.fault.unwrap().code, "PROGRAM_INVALID");

    let mut s2 = session(demo_world());
    let program2 = PlayerProgram::new(
        [
            ("main.py".to_string(), "def loop():\n    pass\n".to_string()),
            ("data.txt".to_string(), "x".to_string()),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out2 = s2.load_program(&program2);
    assert!(!out2.ok);
    assert_eq!(out2.fault.unwrap().code, "PROGRAM_INVALID");
}

// ---------------------------------------------------------------------------
// 生命周期：缓存 / 热重载驱逐 / 环导入
// ---------------------------------------------------------------------------

#[test]
fn py_module_cache_and_hot_reload_eviction() {
    let mut s = session(demo_world());
    let mk = |v: i32| {
        PlayerProgram::new(
            [
                (
                    "main.py".to_string(),
                    "import counter\nLOADS = counter.LOADS\n\ndef loop():\n    Game.log('loads=' + str(LOADS))\n".to_string(),
                ),
                (
                    "counter.py".to_string(),
                    format!("LOADS = {v}\n"),
                ),
            ]
            .into_iter()
            .collect(),
            "main.py",
        )
    };
    let out = s.load_program(&mk(1));
    assert!(out.ok, "init: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("loads=1")),
        "首次加载: {logged:?}"
    );
    // 热重载换内容：sys.modules 驱逐后应取到新模块态。
    let re = s.load_program(&mk(2));
    assert!(re.ok, "重载: {:?}", re.fault);
    let t2 = s.tick();
    assert_eq!(t2.kind, OutcomeKind::Ok);
    let logged2: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged2.iter().any(|l| l.contains("loads=2")),
        "驱逐后应见新值: {logged2:?}"
    );
}

#[test]
fn py_cyclic_player_imports() {
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [
            (
                "main.py".to_string(),
                "import ca\nR = ca.poke()\n\ndef loop():\n    Game.log(R)\n".to_string(),
            ),
            (
                "ca.py".to_string(),
                "import cb\nN = 0\n\ndef poke():\n    global N\n    N += 1\n    return cb.seen(N)\n".to_string(),
            ),
            (
                "cb.py".to_string(),
                "import ca\n\ndef seen(n):\n    return 'cb sees ca.N=' + str(n)\n".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "环导入 init: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged.iter().any(|l| l.contains("cb sees ca.N=1")),
        "环导入应正常: {logged:?}"
    );
}

#[test]
fn py_entry_missing_from_files_rejected() {
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [("lib.py".to_string(), "X = 1\n".to_string())]
            .into_iter()
            .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(!out.ok);
    let f = out.fault.expect("有故障");
    assert_eq!(f.code, "PROGRAM_INVALID");
    assert!(
        f.message.contains("main.py"),
        "错误应指向入口: {}",
        f.message
    );
}

#[test]
fn py_keep_private_names_reserved() {
    // _io/_warnings 被收窄保留在 sys.modules（import 机器硬需求）：
    // 玩家文件不得同名——注册表驱逐按名匹配，会顶掉真模块并永久
    // 破坏宿主的源码装载（评审 P1 修复的回归锁定）。
    for name in ["_io.py", "_warnings.py"] {
        let mut s = session(demo_world());
        let program = PlayerProgram::new(
            [
                ("main.py".to_string(), "def loop():\n    pass\n".to_string()),
                (name.to_string(), "X = 1\n".to_string()),
            ]
            .into_iter()
            .collect(),
            "main.py",
        );
        let out = s.load_program(&program);
        assert!(!out.ok, "{name} 应被拒");
        let f = out.fault.expect("有故障");
        assert_eq!(f.code, "PROGRAM_INVALID", "{name}: {f:?}");
    }
}

#[test]
fn py_non_identifier_module_name_rejected() {
    for name in ["my-lib.py", "a b.py", "class.py"] {
        let mut s = session(demo_world());
        let program = PlayerProgram::new(
            [
                ("main.py".to_string(), "def loop():\n    pass\n".to_string()),
                (name.to_string(), "X = 1\n".to_string()),
            ]
            .into_iter()
            .collect(),
            "main.py",
        );
        let out = s.load_program(&program);
        assert!(!out.ok, "{name} 应被拒（无法 import 的死重量）");
        let f = out.fault.expect("有故障");
        assert_eq!(f.code, "PROGRAM_INVALID", "{name}: {f:?}");
    }
}

#[test]
fn py_same_init_double_import_uses_module_cache() {
    // sys.modules 缓存（docs 08 验收行"sys.modules 缓存"的前半）：
    // 同一程序内两处 import 同一模块，得到同一对象、模块体只执行一次。
    let mut s = session(demo_world());
    let program = PlayerProgram::new(
        [
            (
                "main.py".to_string(),
                "import helper\nimport helper as helper2\nSAME = helper is helper2\nLOADS = helper.LOADS\n\ndef loop():\n    Game.log(\"same=\" + str(SAME) + \" loads=\" + str(LOADS))\n".to_string(),
            ),
            ("helper.py".to_string(), "LOADS = 1\n".to_string()),
        ]
        .into_iter()
        .collect(),
        "main.py",
    );
    let out = s.load_program(&program);
    assert!(out.ok, "init: {:?}", out.fault);
    let t = s.tick();
    assert_eq!(t.kind, OutcomeKind::Ok, "tick: {:?}", s.fault);
    let logged: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(
        logged
            .iter()
            .any(|l| l.contains("same=True") && l.contains("loads=1")),
        "双 import 应命中缓存（同对象、单次执行）: {logged:?}"
    );
}
