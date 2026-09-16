//! Tauri 2 壳：窗口与命令面适配（docs/architecture/01 §代码布局、
//! 05 §与 Rust 的通信）。全部 command 异步实现，不在主线程阻塞等待
//! 世界线程——世界线程挂起时窗口必须保持响应（docs/architecture/03）。
//! 业务面全部在 ztw_desktop lib：壳只做 IPC 适配与生命周期。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use serde_json::Value;
use tauri::State;
use tauri::ipc::Channel;
use ztw_desktop::hostbin::resolve_host_bins;
use ztw_desktop::saves::{SaveStore, SaveSummary, now_unix_ms, read_json, write_json_atomic};
use ztw_desktop::world_thread::{Ctrl, Sink};
use ztw_desktop::{DEFAULT_ID, DiagPage, StatusView, WorldHandle};

struct App {
    handle: WorldHandle,
    /// 存档存储（`<app_data>/saves`）；设置与草稿文件经 data_dir 直达。
    store: Arc<SaveStore>,
    data_dir: PathBuf,
}

/// 注册快照推送通道（收到即由前端 ack；世界线程侧做合并丢旧门控）。
#[tauri::command]
async fn attach(state: State<'_, App>, channel: Channel<Value>) -> Result<(), String> {
    let sink: Sink = Arc::new(move |json: &str| {
        if let Ok(v) = serde_json::from_str::<Value>(json) {
            let _ = channel.send(v);
        }
    });
    state.handle.shared().attach_sink(sink);
    Ok(())
}

/// 前端确认收帧（清在途标记；有更新帧时由世界线程补发最新）。
#[tauri::command]
async fn ack_snapshot(state: State<'_, App>) -> Result<(), String> {
    state.handle.shared().ack_snapshot();
    Ok(())
}

/// 最新快照兜底拉取（attach 前或丢帧时）。
#[tauri::command]
async fn latest_snapshot(state: State<'_, App>) -> Result<Option<String>, String> {
    Ok(state
        .handle
        .shared()
        .latest_snapshot()
        .map(|s| s.to_string()))
}

#[tauri::command]
async fn status(state: State<'_, App>) -> Result<StatusView, String> {
    Ok(state
        .handle
        .shared()
        .status
        .lock()
        .expect("status 锁")
        .clone())
}

/// 静态场景信息（墙格、装卸位、场景清单；一次拉取）。
#[tauri::command]
async fn static_info(state: State<'_, App>) -> Result<Value, String> {
    Ok(state
        .handle
        .shared()
        .static_info
        .lock()
        .expect("static 锁")
        .clone())
}

// -- 调试与会话控制（命令在世界线程安全点生效） ------------------------------

#[tauri::command]
async fn pause(state: State<'_, App>) -> Result<(), String> {
    state.handle.ctrl(Ctrl::Pause)
}

/// 恢复运行（含故障三分类恢复：脚本错误续跑 / 环境级重建 / 宿主重启）。
#[tauri::command]
async fn resume(state: State<'_, App>, tps: u32) -> Result<(), String> {
    state.handle.ctrl(Ctrl::Resume { tps })
}

#[tauri::command]
async fn step(state: State<'_, App>) -> Result<(), String> {
    state.handle.ctrl(Ctrl::Step)
}

/// 热重载：当前 tick 结束后暂停 → 保存即重建执行环境（docs/architecture/05）。
/// 多文件程序整包提交：全部文件集 + 入口文件名 + 语言。
#[tauri::command]
async fn hot_reload(
    state: State<'_, App>,
    files: BTreeMap<String, String>,
    entry: String,
    language: String,
) -> Result<(), String> {
    let lang = ztw_desktop::hostbin::Language::parse(&language)?;
    state.handle.ctrl(Ctrl::LoadProgram {
        files,
        entry,
        language: lang,
    })
}

/// 重开场景。keep_program（默认 true，游戏内"重开"按钮）保留玩家源码
/// 并自动重新初始化；主菜单"开始新场景"传 false，落进未加载代码的初态。
#[tauri::command]
async fn reset(
    state: State<'_, App>,
    scenario: String,
    keep_program: Option<bool>,
) -> Result<(), String> {
    state.handle.ctrl(Ctrl::Reset {
        scenario,
        keep_program: keep_program.unwrap_or(true),
    })
}

/// 退出应用（主菜单"退出"）。走 Tauri request_exit → process::exit，
/// 世界线程与宿主进程不经优雅关闭：宿主在 stdin EOF 后自行退出
/// （runtime 主循环语义），与关窗退出路径对称，不依赖 Session::drop。
#[tauri::command]
async fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// 终止宿主：独立控制路径，不经世界线程命令队列（docs/architecture/03
/// 「看门狗和终止按钮不排在 Game 请求队列后」）。
#[tauri::command]
async fn kill_host(state: State<'_, App>) -> Result<(), String> {
    let ctl = state
        .handle
        .shared()
        .host_ctl
        .lock()
        .expect("host_ctl 锁")
        .clone();
    if let Some(ctl) = ctl {
        ctl.kill();
    }
    Ok(())
}

// -- 诊断与日志游标分页 ------------------------------------------------------

#[tauri::command]
async fn diag_pull(state: State<'_, App>, cursor: u64, limit: u64) -> Result<DiagPage, String> {
    Ok(state
        .handle
        .shared()
        .diag_pull(cursor, limit.clamp(1, 512) as usize))
}

#[tauri::command]
async fn diag_ack(state: State<'_, App>, cursor: u64) -> Result<(), String> {
    state.handle.shared().diag_ack(cursor);
    Ok(())
}

#[tauri::command]
async fn logs_pull(state: State<'_, App>, cursor: u64, limit: u64) -> Result<DiagPage, String> {
    Ok(state
        .handle
        .shared()
        .logs_pull(cursor, limit.clamp(1, 512) as usize))
}

#[tauri::command]
async fn logs_ack(state: State<'_, App>, cursor: u64) -> Result<(), String> {
    state.handle.shared().logs_ack(cursor);
    Ok(())
}

// -- 存档与设置（docs/architecture/06；世界线程安全点取数 + 壳层异步写盘） --

/// 保存进度。name = Some → 手动档，None → 自动档（轮换）。草稿段由
/// 前端透传入档（与已加载程序分开保存，不冒充正在运行的代码）。
#[tauri::command]
async fn save_game(
    state: State<'_, App>,
    name: Option<String>,
    drafts: Option<Value>,
) -> Result<SaveSummary, String> {
    // 1) 世界线程安全点组装（不可变存档数据，docs 06）。
    let (tx, rx) = mpsc::channel();
    state.handle.ctrl(Ctrl::SaveGame {
        name: name.clone(),
        drafts,
        reply: tx,
    })?;
    let data = tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| format!("等待世界线程失败：{e}"))?
        .map_err(|_| "世界线程已退出".to_string())??;
    // 2) 壳层异步写盘（临时文件 + 原子替换 + 序号门，docs 06 写入安全）。
    let created = now_unix_ms();
    let ticket = if name.is_some() {
        state.store.ticket_manual(&data.scenario_id, created)
    } else {
        state.store.ticket_auto(&data.scenario_id)
    };
    let text = ztw_api::save::encode(&data);
    let store = state.store.clone();
    let id = tauri::async_runtime::spawn_blocking(move || store.write(&ticket, &text, created))
        .await
        .map_err(|e| format!("写盘任务失败：{e}"))??;
    // 从盘上取摘要（列表同口径，顺带复核刚写的档可解码）。
    let store = state.store.clone();
    let summary = tauri::async_runtime::spawn_blocking(move || store.summarize(&id))
        .await
        .map_err(|e| format!("摘要任务失败：{e}"))?;
    if !summary.ok {
        return Err(format!(
            "存档已写入但复核失败：{}",
            summary.error.unwrap_or_default()
        ));
    }
    Ok(summary)
}

/// 读取存档：壳层解码过全部门禁 → 世界线程临时会话先成功初始化才换入
/// （读档初始化失败不破坏当前对局）。返回存档内草稿段（编辑器恢复用）。
#[tauri::command]
async fn load_game(state: State<'_, App>, id: String) -> Result<Option<Value>, String> {
    let data = {
        let store = state.store.clone();
        tauri::async_runtime::spawn_blocking(move || store.read(&id))
            .await
            .map_err(|e| format!("读取任务失败：{e}"))?
    }?;
    let (tx, rx) = mpsc::channel();
    state.handle.ctrl(Ctrl::LoadGame {
        data: Box::new(data),
        reply: tx,
    })?;
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| format!("等待世界线程失败：{e}"))?
        .map_err(|_| "世界线程已退出".to_string())?
}

/// 存档摘要列表（跨场景；损坏档 ok=false 灰条展示）。
#[tauri::command]
async fn list_saves(state: State<'_, App>) -> Result<Vec<SaveSummary>, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.list())
        .await
        .map_err(|e| format!("列表任务失败：{e}"))
}

#[tauri::command]
async fn delete_save(state: State<'_, App>, id: String) -> Result<(), String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.delete(&id))
        .await
        .map_err(|e| format!("删除任务失败：{e}"))?
}

/// 编辑器草稿防丢（去抖落盘；`<app_data>/drafts.json`，独立于存档）。
#[tauri::command]
async fn save_drafts(state: State<'_, App>, drafts: Value) -> Result<(), String> {
    let path = state.data_dir.join("drafts.json");
    tauri::async_runtime::spawn_blocking(move || write_json_atomic(&path, &drafts))
        .await
        .map_err(|e| format!("草稿写盘失败：{e}"))?
}

#[tauri::command]
async fn load_drafts(state: State<'_, App>) -> Result<Option<Value>, String> {
    let path = state.data_dir.join("drafts.json");
    tauri::async_runtime::spawn_blocking(move || read_json(&path))
        .await
        .map_err(|e| format!("草稿读取失败：{e}"))?
}

/// 本机设置（`<app_data>/config/settings.json`；布局 / 默认语言等，
/// 不入 Cloud 同步集）。缺失返回 null，由前端回默认值。
#[tauri::command]
async fn get_settings(state: State<'_, App>) -> Result<Option<Value>, String> {
    let path = state.data_dir.join("config").join("settings.json");
    tauri::async_runtime::spawn_blocking(move || read_json(&path))
        .await
        .map_err(|e| format!("设置读取失败：{e}"))?
}

#[tauri::command]
async fn set_settings(state: State<'_, App>, settings: Value) -> Result<(), String> {
    let path = state.data_dir.join("config").join("settings.json");
    tauri::async_runtime::spawn_blocking(move || write_json_atomic(&path, &settings))
        .await
        .map_err(|e| format!("设置写盘失败：{e}"))?
}

fn main() {
    // 宿主二进制解析失败不阻断启动：load_code 报 SPAWN_FAILED（面板可见）。
    let handle = WorldHandle::spawn(DEFAULT_ID, resolve_host_bins());
    tauri::Builder::default()
        .setup(move |app| {
            use tauri::Manager;
            // 应用数据目录一次解析（bundle id dev.ztw.b2 → 平台规范路径）；
            // 存档根与设置 / 草稿文件都挂在其下（docs/architecture/06）。
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("应用数据目录不可用：{e}"))?;
            let store = SaveStore::new(data_dir.join("saves"));
            app.manage(App {
                handle,
                store: Arc::new(store),
                data_dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            attach,
            ack_snapshot,
            latest_snapshot,
            status,
            static_info,
            pause,
            resume,
            step,
            hot_reload,
            reset,
            save_game,
            load_game,
            list_saves,
            delete_save,
            save_drafts,
            load_drafts,
            get_settings,
            set_settings,
            quit,
            kill_host,
            diag_pull,
            diag_ack,
            logs_pull,
            logs_ack,
        ])
        .run(tauri::generate_context!())
        .expect("tauri 应用启动失败");
}
