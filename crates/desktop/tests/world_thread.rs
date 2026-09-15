//! 世界线程状态机验收（docs/architecture/02 §界面命令生效点、§变速与暂停；
//! 08 原型 B“最小画布、单步、动作结果面板”的后端面）。
//! 需要真实宿主二进制：cargo test --workspace 会先构建 ztw-runtime。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ztw_desktop::hostbin::Language;
use ztw_desktop::scenario::{self, B2_ONE};
use ztw_desktop::{Ctrl, StatusView, WorldHandle};

fn spawn(scenario: &str) -> WorldHandle {
    WorldHandle::spawn(scenario, ztw_desktop::hostbin::resolve_host_bins())
}

/// 单文件便捷形态：整个源码即入口（与 PlayerProgram::single_* 同构）。
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
    panic!("等待状态超时：{what}，当前 {:?}", status(h));
}

/// 单机闭环脚本（B2 地图：北墙双装卸位随机停靠，交互位为第二格即库内格，
/// 站位取其南侧）：吃卖单 → 卸车到地面暂存（站位与暂存格分离）→
/// 吃买单 → 按货物类型匹配逐箱搬运交付。六单全了结。
const SIMPLE_LOOP: &str = r#"
let taken = 0;

// 车辆交互站位：装卸位第二格（interact_pos）的南侧一格（缺口都在北墙）。
function standNear(v) {
  return [v.interact_pos.x, v.interact_pos.y + 1];
}

function stepToward(r, tx, ty) {
  const dx = tx - r.pos.x, dy = ty - r.pos.y;
  const sx = Math.sign(dx), sy = Math.sign(dy);
  const tries = Math.abs(dx) >= Math.abs(dy) ? [[sx,0],[0,sy]] : [[0,sy],[sx,0]];
  // 同行 / 同列被挡（如暂存区的箱子）时向垂直轴侧向绕一步。
  const side = sx === 0 ? [[1,0],[-1,0]] : [[0,1],[0,-1]];
  for (const [mx, my] of [...tries, ...side]) {
    if ((mx !== 0 || my !== 0) && r.move([mx, my]) === Game.E.OK) return true;
  }
  return false;
}

// 地面暂存表：[格x, 格y, 站位x, 站位y]；站位行与暂存行分离，互不阻挡。
const PARK = [
  [2, 5, 2, 4], [2, 3, 2, 4],
  [3, 5, 3, 4], [3, 3, 3, 4],
  [4, 5, 4, 4],
];

function parkBox(r) {
  const occ = new Set();
  for (const b of Game.ground_boxes()) occ.add(b.location.x + "," + b.location.y);
  for (const [cx, cy, sx, sy] of PARK) {
    if (occ.has(cx + "," + cy)) continue;
    if (r.pos.x === sx && r.pos.y === sy) { r.drop(cx, cy); return; }
    stepToward(r, sx, sy);
    return;
  }
}

export function loop() {
  const r = Game.robots()[0];
  const vin = Game.vehicles("in")[0];
  const vout = Game.vehicles("out")[0];

  if (vout) {
    // 有出库车：优先按其货物类型逐箱交付。携带物类型不匹配（卸车途中
    // 新车到场）→ 先放回地面暂存，避免对着水车递电池死循环。
    if (r.carry && r.carry.goods_type !== vout.goods_type) {
      parkBox(r);
      return;
    }
    if (r.carry) {
      const [tx, ty] = standNear(vout);
      if (r.pos.x === tx && r.pos.y === ty) { r.give(vout); return; }
      stepToward(r, tx, ty);
      return;
    }
    const boxes = Game.ground_boxes().filter(function (b) {
      return b.goods_type === vout.goods_type;
    });
    if (!boxes.length) return; // 缺货（不该发生）：等待
    const b = boxes[0];
    const bx = b.location.x, by = b.location.y;
    if (Math.abs(r.pos.x - bx) + Math.abs(r.pos.y - by) === 1) { r.pick(bx, by); return; }
    stepToward(r, bx, by);
    return;
  }

  if (vin) {
    // 有入库车：卸空到地面暂存（取走最后一箱的同一结算内车辆离场）。
    if (r.carry) { parkBox(r); return; }
    const [tx, ty] = standNear(vin);
    if (r.pos.x === tx && r.pos.y === ty) { r.take(vin, vin.boxes[0].id); return; }
    stepToward(r, tx, ty);
    return;
  }

  // 无车：按顺序吃单（先买后卖；两个装卸位允许双车并行）。
  const asks = Game.market.sell_orders();
  if (asks.length) { Game.market.take(asks[0].id); return; }
  const bids = Game.market.buy_orders();
  if (bids.length) { Game.market.take(bids[0].id); return; }
}
"#;

fn snapshot(h: &WorldHandle) -> serde_json::Value {
    let raw = h.shared().latest_snapshot().expect("已有快照");
    serde_json::from_str(&raw).expect("合法 JSON")
}

#[test]
fn load_step_pause_resume_flow() {
    let h = spawn(B2_ONE.id);
    // 未加载代码时不推进。
    h.ctrl(Ctrl::Resume { tps: 50 }).expect("cmd");
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(status(&h).tick, 0, "未加载代码不得推进");

    h.ctrl(prog("export function loop() {}", Language::Js))
        .expect("cmd");
    wait_status(&h, |s| s.loaded, "加载成功");
    assert!(!status(&h).running, "热重载后保持暂停");

    // 单步三次：每步恰好 +1，且始终不进入运行态。
    for want in 1..=3u64 {
        h.ctrl(Ctrl::Step).expect("cmd");
        wait_status(&h, |s| s.tick == want && !s.running, "单步推进");
    }

    // 快进 200tps：tick 持续增长。
    h.ctrl(Ctrl::Resume { tps: 200 }).expect("cmd");
    wait_status(&h, |s| s.tick >= 20, "快进推进");
    assert!(status(&h).running);

    // 暂停后 tick 冻结（ctrl 异步：先等暂停生效再采样）。
    h.ctrl(Ctrl::Pause).expect("cmd");
    wait_status(&h, |s| !s.running, "暂停生效");
    let frozen = status(&h).tick;
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(status(&h).tick, frozen, "暂停后不得推进");
    assert!(!status(&h).running);

    // 快照与静态信息就绪（MirrorView 扁平化 + 控制面字段）。
    let snap = snapshot(&h);
    assert_eq!(snap["scenario"], B2_ONE.id);
    assert_eq!(snap["tick"].as_u64(), Some(frozen));
    assert_eq!(snap["robots"].as_array().unwrap().len(), 1);
    assert_eq!(snap["map_w"], 16);
    let stat = h.shared().static_info.lock().expect("static 锁").clone();
    assert_eq!(stat["walls"].as_array().unwrap().len(), 50);

    // 诊断环：控制事件可按游标拉取。
    let page = h.shared().diag_pull(0, 100);
    assert!(page.gap.is_none());
    assert!(
        page.events
            .iter()
            .any(|e| e.kind == "control" && e.payload["op"] == "load_code")
    );
    h.join();
}

#[test]
fn full_loop_completes_orders_with_diag_events() {
    let h = spawn(B2_ONE.id);
    h.ctrl(prog(SIMPLE_LOOP, Language::Js)).expect("cmd");
    wait_status(&h, |s| s.loaded, "加载成功");
    h.ctrl(Ctrl::Resume { tps: 200 }).expect("cmd");

    // 闭环完成：六张订单全部了结 + 金币净增。
    let deadline = Instant::now() + Duration::from_secs(120); // CI 慢机留余量
    let mut cursor = 0;
    let mut done = 0usize;
    let mut manage = 0usize;
    let mut settle_ok = 0usize;
    loop {
        assert!(
            Instant::now() < deadline,
            "闭环超时未完成，游标 {cursor}，终态 {}，事件直方图 {:?}",
            snapshot(&h),
            {
                let mut hist: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                let mut c = 0;
                loop {
                    let pg = h.shared().diag_pull(c, 500);
                    if pg.events.is_empty() {
                        break;
                    }
                    for e in &pg.events {
                        let key = match e.kind.as_str() {
                            "accept_fail" => format!(
                                "accept_fail:{}:{}",
                                e.payload["op"].as_str().unwrap_or("?"),
                                e.payload["code"].as_str().unwrap_or("?")
                            ),
                            "settle" if e.payload["ok"] == serde_json::Value::Bool(false) => {
                                format!(
                                    "settle_fail:{}:{}",
                                    e.payload["action"].as_str().unwrap_or("?"),
                                    e.payload["code"].as_str().unwrap_or("?")
                                )
                            }
                            other => other.to_string(),
                        };
                        *hist.entry(key).or_default() += 1;
                    }
                    c = pg.next;
                }
                hist
            }
        );
        let page = h.shared().diag_pull(cursor, 200);
        cursor = page.next;
        for e in &page.events {
            match e.kind.as_str() {
                "order_done" => done += 1,
                "manage" => manage += 1,
                "settle" if e.payload["ok"] == serde_json::Value::Bool(true) => settle_ok += 1,
                _ => {}
            }
        }
        h.shared().diag_ack(cursor);
        if done >= 6 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    h.ctrl(Ctrl::Pause).expect("cmd");
    // 暂停生效后再采样：世界线程每 tick 先推诊断环、后序列化整帧存
    // latest——从环里看到第 6 个完成事件时，付款 tick 的帧可能仍在序列
    // 化，抢读 latest 会恰好差最后一笔收款（Windows CI 抖动实录：六事件
    // 齐但金币少一笔买单收入）。暂停命令自身触发发布，等 running=false
    // 即可保证 latest 已含最终 tick。
    wait_status(&h, |s| !s.running, "闭环后暂停生效");
    assert!(manage >= 6, "六次接单事件应被采集：{manage}");
    assert!(settle_ok > 0, "成功结算事件应被采集：{settle_ok}");
    let snap = snapshot(&h);
    // water 卖单 4@3.8 买入、买单 4@5.2 卖出：毛利 +5.6；初始 300。
    let gold: i64 = snap["gold_milli"].as_str().unwrap().parse().unwrap();
    assert!(gold > 300_000, "闭环应盈利，当前 {gold}");
    // 日志环与诊断环独立（Game.log 与事件分置）。
    let logs = h.shared().logs_pull(0, 10);
    assert!(logs.events.iter().all(|e| e.kind == "log"));
    h.join();
}

#[test]
fn script_fault_pauses_and_resume_recovers() {
    let h = spawn(B2_ONE.id);
    h.ctrl(prog(
        "export function loop() { throw new Error('boom'); }",
        Language::Js,
    ))
    .expect("cmd");
    wait_status(&h, |s| s.loaded, "加载成功");
    h.ctrl(Ctrl::Step).expect("cmd");
    wait_status(
        &h,
        |s| s.fault_class.as_deref() == Some("script"),
        "脚本错误自动暂停",
    );
    assert!(!status(&h).running);
    // 快照携带故障摘要（含错误码）。
    let snap = snapshot(&h);
    assert!(
        snap["fault"]["message"].as_str().unwrap().contains("boom"),
        "故障消息应含原始错误：{}",
        snap["fault"]["message"]
    );
    // 恢复：脚本级错误保留环境直接续跑——但同一坏代码在下一 tick 仍会抛错，
    // 于是再次自动暂停（docs/game-design/03：恢复运行后从下一 tick 继续）。
    h.ctrl(Ctrl::Resume { tps: 5 }).expect("cmd");
    wait_status(&h, |s| s.tick >= 2, "恢复后推进过故障 tick");
    assert_eq!(
        status(&h).fault_class.as_deref(),
        Some("script"),
        "同一坏代码再次抛错"
    );
    assert!(!status(&h).running, "再次自动暂停");
    // 修码 → 热重载 → 恢复：干净代码持续运行。
    h.ctrl(prog("export function loop() {}", Language::Js))
        .expect("cmd");
    wait_status(
        &h,
        |s| s.loaded && s.fault_class.is_none(),
        "热重载清除故障",
    );
    h.ctrl(Ctrl::Resume { tps: 5 }).expect("cmd");
    wait_status(
        &h,
        |s| s.tick >= 3 && s.running && s.fault_class.is_none(),
        "修码后持续运行",
    );
    h.join();
}

#[test]
fn hot_reload_preserves_world_and_reset_rebuilds() {
    let h = spawn(B2_ONE.id);
    h.ctrl(prog("export function loop() {}", Language::Js))
        .expect("cmd");
    wait_status(&h, |s| s.loaded, "加载成功");
    h.ctrl(Ctrl::Resume { tps: 200 }).expect("cmd");
    wait_status(&h, |s| s.tick >= 4, "推进若干 tick");
    h.ctrl(Ctrl::Pause).expect("cmd");
    wait_status(&h, |s| !s.running, "暂停生效");
    let before = status(&h).tick;

    // 热重载：世界与 tick 保留（只重建执行环境），加载后仍暂停。
    h.ctrl(prog("export function loop() {}", Language::Js))
        .expect("cmd");
    wait_status(&h, |s| s.loaded, "重载成功");
    assert_eq!(status(&h).tick, before, "热重载不推进世界");
    h.ctrl(Ctrl::Resume { tps: 200 }).expect("cmd");
    wait_status(&h, |s| s.tick > before, "重载后继续推进");

    // 重开场景：tick 归零、源码保留（自动重新初始化）。
    h.ctrl(Ctrl::Pause).expect("cmd");
    h.ctrl(Ctrl::Reset {
        scenario: scenario::B2_FIVE.id.to_string(),
    })
    .expect("cmd");
    wait_status(&h, |s| s.tick == 0 && s.loaded, "重开后自动加载");
    let snap = snapshot(&h);
    assert_eq!(snap["scenario"], scenario::B2_FIVE.id);
    assert_eq!(snap["robots"].as_array().unwrap().len(), 5);
    h.join();
}

#[test]
fn snapshot_sink_gating_merges_when_slow() {
    // 慢前端：不 ack 时在途最多一帧；世界线程不反压、持续推进。
    let h = spawn(B2_ONE.id);
    let frames = Arc::new(Mutex::new(0u64));
    let sink_frames = frames.clone();
    h.shared().attach_sink(Arc::new(move |json: &str| {
        let v: serde_json::Value = serde_json::from_str(json).expect("合法快照");
        let _ = v;
        *sink_frames.lock().expect("sink 锁") += 1;
    }));
    h.ctrl(prog("export function loop() {}", Language::Js))
        .expect("cmd");
    wait_status(&h, |s| s.loaded, "加载成功");
    h.ctrl(Ctrl::Resume { tps: 200 }).expect("cmd");
    wait_status(&h, |s| s.tick >= 30, "推进 30+ tick");
    h.ctrl(Ctrl::Pause).expect("cmd");
    let sent = *frames.lock().expect("sink 锁");
    assert!(sent <= 5, "不 ack 时应合并丢旧，实际发送 {sent} 帧");
    assert!(status(&h).tick >= 30);
    // ack 后门控复位（后续帧可继续送达；同修订号不重发）。
    h.shared().ack_snapshot();
    h.shared().ack_snapshot();
    h.join();
}

#[test]
fn snapshot_pushes_control_plane_change_while_paused() {
    // 回归：暂停中热重载不推进世界修订号，控制面帧（loaded 翻转）仍必须
    // 送达——按世界修订判重会把它吞掉。
    let h = spawn(B2_ONE.id);
    let seen = Arc::new(Mutex::new(Vec::<bool>::new()));
    let sink_seen = seen.clone();
    h.shared().attach_sink(Arc::new(move |json: &str| {
        let v: serde_json::Value = serde_json::from_str(json).expect("合法快照");
        sink_seen
            .lock()
            .expect("seen 锁")
            .push(v["loaded"].as_bool().expect("loaded 字段"));
    }));
    // 默认 StatusView 也是「暂停 tick 0」：用 scenario 非空判定首发已完成，
    // 保证 ack 落在首帧在途之后（ack 早于首发会让热重载帧被 inflight 合并）。
    wait_status(&h, |s| s.scenario == B2_ONE.id, "首发快照已发布");
    h.shared().ack_snapshot(); // 确认初始帧，清在途标记
    h.ctrl(prog("export function loop() {}", Language::Js))
        .expect("cmd");
    wait_status(&h, |s| s.loaded, "加载成功");
    let got = seen.lock().expect("seen 锁").clone();
    assert_eq!(
        got.last(),
        Some(&true),
        "暂停中热重载后应推送 loaded=true 帧，实际收到 {got:?}"
    );
    h.join();
}

// ---------------------------------------------------------------------------
// A1：语言切换（docs/architecture/03：切换语言重启宿主，已提交 memory 保留）
// ---------------------------------------------------------------------------

#[test]
fn language_switch_restarts_host_and_preserves_memory() {
    let h = spawn(B2_ONE.id);
    // JS：写 memory 并推进。
    h.ctrl(prog(
        "Game.memory['n'] = 41;\nexport function loop() { Game.memory['n'] += 1; }",
        Language::Js,
    ))
    .expect("cmd");
    wait_status(&h, |s| s.loaded, "JS 加载成功");
    h.ctrl(Ctrl::Step).expect("cmd");
    wait_status(&h, |s| s.tick >= 1, "JS tick1");
    assert_eq!(status(&h).language, "js");

    // 切换 Python：宿主重启（会话重建），memory 由主进程持有应保留；
    // 初始 tick 阶段即可读旧值。
    h.ctrl(prog("def loop():\n    Game.memory['n'] = Game.memory['n'] + 1\n    Game.log('py n', Game.memory['n'])\n", Language::Py))
    .expect("cmd");
    wait_status(
        &h,
        |s| s.loaded && s.language == "py",
        "PY 加载成功且语言切换",
    );
    h.ctrl(Ctrl::Step).expect("cmd");
    wait_status(&h, |s| s.tick >= 2, "PY tick2");
    // 日志面读回 n=43（41 + JS 一次 + PY 一次）：已提交 memory 跨语言保留。
    let mut found = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let page = h.shared().logs_pull(0, 64);
        if let Some(ev) = page.events.iter().find(|e| {
            e.payload
                .get("line")
                .and_then(|l| l.as_str())
                .unwrap_or("")
                .starts_with("py n")
        }) {
            found = Some(
                ev.payload
                    .get("line")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        found.as_deref(),
        Some("py n 43.0"),
        "memory 跨语言保留：{found:?}"
    );
    // 回 JS：再次切换同样成立。
    h.ctrl(prog(
        "export function loop() { Game.log('js n', Game.memory['n']); }",
        Language::Js,
    ))
    .expect("cmd");
    wait_status(&h, |s| s.loaded && s.language == "js", "切回 JS");
    h.join();
}

/// 切换专用“只丢弃”契约（Session 级）：drop_host_for_switch 不得走
/// restart_host 路径——后者会把旧代码拿到新宿主重跑一次 init 并计入
/// host_restarts；已提交 memory 必须保留给新代码。
#[test]
fn drop_host_for_switch_never_reruns_old_code() {
    let bins = ztw_desktop::hostbin::resolve_host_bins();
    let mut s = ztw_api::harness::Session::new(
        ztw_api::harness::SessionConfig::new(&bins.js),
        scenario::build(&B2_ONE),
    );
    assert!(
        s.load_code("Game.memory['n'] = 1;\nexport function loop() {}")
            .ok,
        "{:?}",
        s.fault
    );
    let restarts = s.stats.host_restarts;
    s.drop_host_for_switch();
    assert_eq!(s.stats.host_restarts, restarts, "切换丢弃不是恢复重启路径");
    assert!(
        s.load_code("export function loop() { Game.log('n', Game.memory['n']); }")
            .ok,
        "{:?}",
        s.fault
    );
    s.tick();
    // memory 保留：旧值 1 对新代码可见（若旧代码被重跑重置也无从分辨，
    // 上面的 restarts 断言才是“未重跑”的直接证据）。
    let lines: Vec<String> = s.logs.iter().map(|(_, l)| l.clone()).collect();
    assert!(lines.iter().any(|l| l == "n 1"), "memory 保留：{lines:?}");
}

/// 多文件文件集（评审补测）：文件集整体送达宿主（跨模块 import 可见）、
/// st.program 以多文件形态跨 Reset 保留并自动重初始化、失败提交不污染
/// 已保留程序。
#[test]
fn multi_file_program_survives_reset_and_failed_load() {
    let h = spawn(B2_ONE.id);
    h.ctrl(Ctrl::LoadProgram {
        files: [
            (
                "main.js".to_string(),
                "import { tag } from './lib.js';\nexport function loop() { Game.log('tag', tag); }"
                    .to_string(),
            ),
            (
                "lib.js".to_string(),
                "export const tag = 'lib-ok';".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
        entry: "main.js".to_string(),
        language: Language::Js,
    })
    .expect("cmd");
    wait_status(&h, |s| s.loaded, "多文件加载成功");

    // 单步一步：日志环出现 lib.js 的标记 = 文件集整体送达宿主。
    h.ctrl(Ctrl::Step).expect("cmd");
    wait_status(&h, |s| s.tick == 1, "单步推进");
    let logs = h.shared().logs_pull(0, 50);
    assert!(
        logs.events
            .iter()
            .any(|e| e.payload["line"] == "tag lib-ok"),
        "lib 模块日志缺失：{:?}",
        logs.events
    );

    // 失败提交（缺 lib.js 的悬空 import）：世界与已保留程序不受污染。
    h.ctrl(Ctrl::LoadProgram {
        files: [(
            "main.js".to_string(),
            "import { tag } from './lib.js';\nexport function loop() {}".to_string(),
        )]
        .into_iter()
        .collect(),
        entry: "main.js".to_string(),
        language: Language::Js,
    })
    .expect("cmd");
    wait_status(
        &h,
        |s| !s.loaded && s.fault_class.is_some(),
        "坏文件集以故障打回",
    );

    // Reset：世界重建后自动重载的仍是原两文件程序（tag 再现即证据）。
    h.ctrl(Ctrl::Reset {
        scenario: B2_ONE.id.to_string(),
    })
    .expect("cmd");
    wait_status(&h, |s| s.loaded && s.tick == 0, "Reset 后自动重载");
    h.ctrl(Ctrl::Step).expect("cmd");
    wait_status(&h, |s| s.tick == 1, "Reset 后单步推进");
    let logs = h.shared().logs_pull(0, 50);
    assert!(
        logs.events
            .iter()
            .any(|e| e.payload["line"] == "tag lib-ok"),
        "Reset 后未恢复原两文件程序：{:?}",
        logs.events
    );
    h.join();
}
