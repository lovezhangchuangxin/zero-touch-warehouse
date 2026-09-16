//! C1 存档管线验收（docs/architecture/08 原型 C 存档条款）：
//! - 存档→推进→读档对账（tick / memory / 草稿段 / 场景）；
//! - 运行中的存档请求在 tick 边界安全点生效（08 §存档）；
//! - 读档初始化失败不破坏当前对局（08 §存档）；
//! - 写入安全故障注入矩阵（08 §存档：旧档有效、旧序号不覆盖新档）。
//!
//! 模式沿用 world_thread.rs 先例（真实宿主 + wait_status 轮询）；
//! 崩溃旋钮经测试二进制自再执行（child 模式下 write 内 abort）。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use ztw_api::harness::PlayerProgram;
use ztw_api::save::SaveData;
use ztw_desktop::hostbin::Language;
use ztw_desktop::saves::{AUTO_SLOTS, SAVE_FAULT_KNOBS, SaveStore};
use ztw_desktop::scenario::{self, B2_ONE, M3_TRADE};
use ztw_desktop::{Ctrl, StatusView, WorldHandle};
use ztw_model::MemValue;

fn spawn(scenario: &str) -> WorldHandle {
    WorldHandle::spawn(scenario, ztw_desktop::hostbin::resolve_host_bins())
}

fn prog(code: impl Into<String>, language: Language) -> Ctrl {
    let entry = if language == Language::Js {
        "main.js"
    } else {
        "main.py"
    };
    Ctrl::LoadProgram {
        files: [(entry.to_string(), code.into())].into_iter().collect(),
        entry: entry.to_string(),
        language,
    }
}

/// 每 tick 写一次 memory 的最小程序（memory 存续的可观测探针）。
/// JS 入口契约：ESM 导出 loop（A2 文件集语义）。
const COUNTER_JS: &str = r#"
let n = 0;
export function loop() { Game.memory["counter"] = ++n; }
"#;

fn status(h: &WorldHandle) -> StatusView {
    h.shared().status.lock().expect("status 锁").clone()
}

fn wait_status(h: &WorldHandle, pred: impl Fn(&StatusView) -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30); // CI 慢机留余量
    while Instant::now() < deadline {
        if pred(&status(h)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let diag = h.shared().diag_pull(0, 16);
    let events: Vec<String> = diag
        .events
        .iter()
        .map(|e| format!("[{}] {} {}", e.tick, e.kind, e.payload))
        .collect();
    panic!(
        "等待状态超时：{what}，当前 {:?}，诊断：\n{}",
        status(h),
        events.join("\n")
    );
}

/// 世界线程安全点取数（存档请求 → 回执）。
fn save_at_safe_point(
    h: &WorldHandle,
    name: Option<String>,
    drafts: Option<Value>,
) -> Result<SaveData, String> {
    let (tx, rx) = mpsc::channel();
    h.ctrl(Ctrl::SaveGame {
        name,
        drafts,
        reply: tx,
    })
    .expect("世界线程存活");
    rx.recv_timeout(Duration::from_secs(30))
        .map_err(|_| "等待存档回执超时".to_string())?
}

fn tmp_store(tag: &str) -> (SaveStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("ztw-c1-saves-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = SaveStore::new(dir.clone());
    (store, dir)
}

fn write_manual(
    store: &SaveStore,
    scenario: &str,
    data: &SaveData,
    created: u64,
) -> Result<String, String> {
    let t = store.ticket_manual(scenario, created);
    store.write(&t, &ztw_api::save::encode(data), created)
}

fn empty_memory() -> MemValue {
    MemValue::Map(vec![])
}

/// memory 根映射取键（对账辅助）。
fn mem_get<'a>(v: &'a MemValue, key: &str) -> Option<&'a MemValue> {
    match v {
        MemValue::Map(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 端到端：存档 → 推进 → 读档（真实宿主）
// ---------------------------------------------------------------------------

/// 存读档闭环：读档回退到存档点（tick 与 memory 一起回退，docs 06 恢复
/// 行为表），草稿段随档返回，程序自动重新初始化。
#[test]
fn save_then_load_restores_tick_and_memory() {
    let h = spawn(M3_TRADE.id);
    h.ctrl(prog(COUNTER_JS, Language::Js)).unwrap();
    wait_status(&h, |s| s.loaded, "程序加载");
    // 推进到 tick ≥ 10 后暂停存档。
    h.ctrl(Ctrl::Resume { tps: 200 }).unwrap();
    wait_status(&h, |s| s.tick >= 10, "推进 10 tick");
    h.ctrl(Ctrl::Pause).unwrap();
    wait_status(&h, |s| !s.running, "暂停");
    let drafts = json!({"active": "js", "files": {"main.js": "// 草稿"}});
    let data = save_at_safe_point(&h, Some("进度一".into()), Some(drafts.clone()))
        .expect("安全点组装成功");
    let saved_tick = data.tick;
    assert!(saved_tick >= 10);
    // 写盘（手动档）。
    let (store, dir) = tmp_store("roundtrip");
    let id = write_manual(&store, M3_TRADE.id, &data, data.created_at_ms).expect("写盘成功");
    // 继续推进（超出存档点）再读档回退。
    h.ctrl(Ctrl::Resume { tps: 200 }).unwrap();
    wait_status(&h, |s| s.tick >= saved_tick + 5, "推进超出存档点");
    h.ctrl(Ctrl::Pause).unwrap();
    wait_status(&h, |s| !s.running, "再次暂停");
    let loaded = store.read(&id).expect("读盘过门禁");
    let (tx, rx) = mpsc::channel();
    h.ctrl(Ctrl::LoadGame {
        data: Box::new(loaded),
        reply: tx,
    })
    .unwrap();
    let returned_drafts = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("读档回执")
        .expect("读档成功");
    assert_eq!(returned_drafts, Some(drafts), "草稿段随档返回");
    wait_status(
        &h,
        |s| s.tick == saved_tick && s.loaded && !s.running,
        "读档回退",
    );
    assert_eq!(status(&h).scenario, M3_TRADE.id);
    // 读档后 memory 与存档点一致（再次存档取数对账）。
    let again = save_at_safe_point(&h, None, None).expect("读档后可再存档");
    let saved_counter = mem_get(&data.memory.root, "counter")
        .cloned()
        .expect("存档含 counter");
    assert_eq!(
        mem_get(&again.memory.root, "counter"),
        Some(&saved_counter),
        "memory 回退到存档点"
    );
    assert!(again.memory.revision >= data.memory.revision);
    h.join();
    let _ = std::fs::remove_dir_all(dir);
}

/// 未加载程序时没有可存进度（存档只在初始化结束的安全点取数）。
#[test]
fn save_without_program_rejected() {
    let h = spawn(B2_ONE.id);
    let err = save_at_safe_point(&h, None, None).expect_err("无程序须拒绝");
    assert!(err.contains("尚未加载程序"), "错误须可读：{err}");
    h.join();
}

/// 读档初始化失败（坏程序档）不破坏当前对局：tick / loaded 原样。
#[test]
fn load_with_failing_init_keeps_current_session() {
    let h = spawn(B2_ONE.id);
    h.ctrl(prog(COUNTER_JS, Language::Js)).unwrap();
    wait_status(&h, |s| s.loaded, "程序加载");
    h.ctrl(Ctrl::Resume { tps: 200 }).unwrap();
    wait_status(&h, |s| s.tick >= 5, "推进");
    h.ctrl(Ctrl::Pause).unwrap();
    wait_status(&h, |s| !s.running, "暂停");
    let before = status(&h);
    // 坏档：顶层抛错的程序（初始化必失败）。
    let bad = SaveData::capture(
        &scenario::build(&B2_ONE),
        empty_memory(),
        0,
        &PlayerProgram::single_js("throw new Error('boom')"),
        "js",
        B2_ONE.id,
        None,
        None,
        0,
    );
    let (tx, rx) = mpsc::channel();
    h.ctrl(Ctrl::LoadGame {
        data: Box::new(bad),
        reply: tx,
    })
    .unwrap();
    let err = rx
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap_err();
    assert!(err.contains("读档初始化失败"), "错误须可读：{err}");
    let after = status(&h);
    assert_eq!(after.tick, before.tick, "tick 不受坏档影响");
    assert!(after.loaded, "当前程序仍加载");
    assert!(!after.running);
    // 未知场景同样拒绝。
    let alien = SaveData::capture(
        &scenario::build(&B2_ONE),
        empty_memory(),
        0,
        &PlayerProgram::single_js("export function loop() {}"),
        "js",
        "no-such-scenario",
        None,
        None,
        0,
    );
    let (tx, rx) = mpsc::channel();
    h.ctrl(Ctrl::LoadGame {
        data: Box::new(alien),
        reply: tx,
    })
    .unwrap();
    let err = rx
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap_err();
    assert!(err.contains("未知场景"), "错误须点名场景：{err}");
    assert_eq!(status(&h).tick, before.tick);
    h.join();
}

/// 运行中的存档请求在 tick 边界生效（docs 08 §存档：活动 tick 中收到
/// 的请求排到 tick 结束）：回执 tick 为整数边界，且模拟随后继续越过
/// 该点——读档精确落在回执 tick。
#[test]
fn save_while_running_lands_on_tick_boundary() {
    let h = spawn(B2_ONE.id);
    h.ctrl(prog(COUNTER_JS, Language::Js)).unwrap();
    wait_status(&h, |s| s.loaded, "程序加载");
    h.ctrl(Ctrl::Resume { tps: 100 }).unwrap();
    // 立即请求存档（此刻大概率在活动 tick 中）。
    let data = save_at_safe_point(&h, None, None).expect("运行中存档回执");
    let saved_tick = data.tick;
    wait_status(&h, |s| s.tick > saved_tick, "模拟越过存档点");
    h.ctrl(Ctrl::Pause).unwrap();
    wait_status(&h, |s| !s.running, "暂停");
    let (tx, rx) = mpsc::channel();
    h.ctrl(Ctrl::LoadGame {
        data: Box::new(data),
        reply: tx,
    })
    .unwrap();
    rx.recv_timeout(Duration::from_secs(30))
        .unwrap()
        .expect("读档成功");
    wait_status(&h, |s| s.tick == saved_tick, "读档落在边界 tick");
    h.join();
}

// ---------------------------------------------------------------------------
// SaveStore 写入安全（无宿主；故障旋钮经环境变量——本文件内单测试串行
// 使用，避免并行污染）
// ---------------------------------------------------------------------------

/// 组装一个可直接落盘的最小存档（无宿主路径）。
fn mini_save(scenario: &str, created: u64, mark: u64) -> SaveData {
    let mut w = scenario::build(&B2_ONE);
    w.gold_milli = 500_000 + mark as i64; // 肉眼可辨的版本标记
    SaveData::capture(
        &w,
        empty_memory(),
        0,
        &PlayerProgram::single_js("export function loop() {}"),
        "js",
        scenario,
        None,
        None,
        created,
    )
}

/// 旋钮互锁（模式仿 a1_fault_parity 的清单↔探针对账）：清单即探针
/// 集合，任何增删须同步本文件矩阵。
#[test]
fn every_knob_has_a_probe() {
    assert_eq!(
        SAVE_FAULT_KNOBS,
        &[
            "fail_tmp_write",
            "corrupt_payload",
            "crash_before_replace",
            "crash_after_replace",
            "stale_seq",
        ],
        "SAVE_FAULT_KNOBS 增删须同步 c1_save_load 的探针矩阵"
    );
}

/// 写入安全矩阵：序号新鲜度（旧序号不覆盖新档）、临时写失败保留旧档、
/// 载荷篡改可被校验和检出、路径穿越拒绝、列表灰条。旋钮经 `with_fault`
/// 注入单个 store 实例（同二进制并行测试共享进程环境，禁用 set_var）。
#[test]
fn store_write_safety_matrix() {
    let (store, dir) = tmp_store("safety");
    // 序号新鲜度：同目标两张票，旧票执行时已被新票取代。
    let d1 = mini_save(B2_ONE.id, 100, 1);
    let t_old = store.ticket_manual(B2_ONE.id, 100);
    let t_new = store.ticket_manual(B2_ONE.id, 100);
    let err = store
        .write(&t_old, &ztw_api::save::encode(&d1), 100)
        .unwrap_err();
    assert!(err.contains("取代"), "旧序号须被拒：{err}");
    assert!(
        store
            .write(&t_new, &ztw_api::save::encode(&d1), 100)
            .is_ok()
    );

    // 临时写失败：明确报错、不产出文件。
    let (_fail_dir_root, dir2) = tmp_store("safety-fail-tmp");
    let faulted = SaveStore::with_fault(dir2.clone(), Some("fail_tmp_write"));
    let d2 = mini_save(B2_ONE.id, 200, 2);
    let t = faulted.ticket_manual(B2_ONE.id, 200);
    let err = faulted
        .write(&t, &ztw_api::save::encode(&d2), 200)
        .unwrap_err();
    assert!(err.contains("临时文件"), "磁盘满模拟须可读：{err}");
    assert!(faulted.list().is_empty());
    let _ = std::fs::remove_dir_all(dir2);

    // 载荷篡改：写入成功但读盘时被校验和拒（灰条 ok=false）。
    let (_corrupt_dir_root, dir3) = tmp_store("safety-corrupt");
    let faulted = SaveStore::with_fault(dir3.clone(), Some("corrupt_payload"));
    let d3 = mini_save(B2_ONE.id, 300, 3);
    let t = faulted.ticket_manual(B2_ONE.id, 300);
    let id3 = faulted.write(&t, &ztw_api::save::encode(&d3), 300).unwrap();
    let sum = faulted.summarize(&id3);
    assert!(!sum.ok, "篡改档须灰条");
    assert!(sum.error.as_deref().unwrap_or_default().contains("校验和"));
    assert!(faulted.read(&id3).is_err(), "读档须过校验和门禁");
    let _ = std::fs::remove_dir_all(dir3);

    // 路径穿越拒绝。
    assert!(store.read("../../etc/passwd").is_err());
    assert!(store.read("/etc/passwd").is_err());

    // 删除后读取报错。
    let id1 = store.list().iter().map(|s| s.id.clone()).next().unwrap();
    store.delete(&id1).unwrap();
    assert!(store.read(&id1).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

/// 自动档轮换：连续 5 次自动存档只保留 AUTO_SLOTS 份，且最新内容是
/// 最后一次写入。
#[test]
fn auto_rotation_keeps_slots() {
    let (store, dir) = tmp_store("rotation");
    for i in 0..5u64 {
        let d = mini_save(B2_ONE.id, 1000 + i, i);
        let t = store.ticket_auto(B2_ONE.id);
        store
            .write(&t, &ztw_api::save::encode(&d), 1000 + i)
            .unwrap();
    }
    let autos: Vec<_> = store.list().into_iter().filter(|s| s.auto).collect();
    assert_eq!(autos.len() as u32, AUTO_SLOTS, "自动档只保留轮换槽数");
    // 全部可读，且最新一次（mark=4）在场。
    let marks: Vec<i64> = autos
        .iter()
        .filter(|s| s.ok)
        .map(|s| store.read(&s.id).unwrap().world.gold_milli)
        .collect();
    assert!(marks.contains(&(500_000 + 4)), "最新自动档在场：{marks:?}");
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// 崩溃旋钮：测试二进制自再执行（child 在 write 内 abort，父进程断言
// 旧档 / 新档完好性——docs 08「替换前后崩溃、旧档有效」）
// ---------------------------------------------------------------------------

#[test]
fn knob_crash_matrix() {
    // 子进程模式：按环境变量执行一次自动档写入（旋钮在 write 内触发
    // abort；若未 abort 则正常退出）。
    if let Ok(dir) = std::env::var("ZTW_SAVE_CHILD_DIR") {
        let mark: u64 = std::env::var("ZTW_SAVE_CHILD_MARK")
            .ok()
            .and_then(|m| m.parse().ok())
            .unwrap_or(99);
        let store = SaveStore::new(dir.into());
        let d = mini_save(B2_ONE.id, 9000 + mark, mark);
        let t = store.ticket_auto(B2_ONE.id);
        let _ = store.write(&t, &ztw_api::save::encode(&d), 9000 + mark);
        std::process::exit(0);
    }
    // 父进程：逐旋钮布满自动档轮换（3 槽全满才会覆写最旧槽）→ 子进程
    // 崩溃写 → 断言旧档 / 新档完好性。
    let exe = std::env::current_exe().expect("测试二进制路径");
    for knob in ["crash_before_replace", "crash_after_replace"] {
        let (store, dir) = tmp_store("crash");
        // 三槽全满（mark=0..2；最旧 mark=0 落在 auto-1，将被子进程覆写）。
        for i in 0..3u64 {
            let d = mini_save(B2_ONE.id, 8000 + i, i);
            let t = store.ticket_auto(B2_ONE.id);
            store
                .write(&t, &ztw_api::save::encode(&d), 8000 + i)
                .unwrap();
        }
        let out = std::process::Command::new(&exe)
            .args(["knob_crash_matrix", "--exact", "--nocapture"])
            .env("ZTW_SAVE_CHILD_DIR", &dir)
            .env("ZTW_SAVE_CHILD_MARK", "9")
            .env("ZTW_SAVE_FAULT", knob)
            .status()
            .expect("子进程启动");
        assert!(!out.success(), "crash 旋钮须使子进程异常终止（{knob}）");
        // 无论崩在哪，盘上必须有一份完整可读的自动档；崩前旧档保留、
        // 崩后新档在场。
        let fresh = SaveStore::new(dir.clone());
        let autos: Vec<_> = fresh.list().into_iter().filter(|s| s.auto).collect();
        assert_eq!(autos.len(), 3, "崩溃不产生多余文件（{knob}）");
        let slot1 = fresh
            .read(&format!("{}/auto/auto-1.json", B2_ONE.id))
            .expect("崩溃后目标槽必须可读");
        let mark = slot1.world.gold_milli - 500_000;
        match knob {
            "crash_before_replace" => assert_eq!(mark, 0, "替换前崩溃：旧档原样"),
            "crash_after_replace" => assert_eq!(mark, 9, "替换后崩溃：新档完整"),
            _ => unreachable!(),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
