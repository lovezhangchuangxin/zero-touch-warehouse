//! Tauri 2 壳：窗口与命令面适配（docs/architecture/01 §代码布局、
//! 05 §与 Rust 的通信）。全部 command 异步实现，不在主线程阻塞等待
//! 世界线程——世界线程挂起时窗口必须保持响应（docs/architecture/03）。
//! 业务面全部在 ztw_desktop lib：壳只做 IPC 适配与生命周期。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use serde_json::Value;
use tauri::State;
use tauri::ipc::Channel;
use ztw_desktop::hostbin::resolve_host_bins;
use ztw_desktop::world_thread::{Ctrl, Sink};
use ztw_desktop::{DEFAULT_ID, DiagPage, StatusView, WorldHandle};

struct App {
    handle: WorldHandle,
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
#[tauri::command]
async fn hot_reload(state: State<'_, App>, code: String, language: String) -> Result<(), String> {
    let lang = ztw_desktop::hostbin::Language::parse(&language)?;
    state.handle.ctrl(Ctrl::LoadCode {
        code,
        language: lang,
    })
}

/// 重开场景（保留玩家源码并自动重新初始化）。
#[tauri::command]
async fn reset(state: State<'_, App>, scenario: String) -> Result<(), String> {
    state.handle.ctrl(Ctrl::Reset { scenario })
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

fn main() {
    // 宿主二进制解析失败不阻断启动：load_code 报 SPAWN_FAILED（面板可见）。
    let handle = WorldHandle::spawn(DEFAULT_ID, resolve_host_bins());
    tauri::Builder::default()
        .manage(App { handle })
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
            kill_host,
            diag_pull,
            diag_ack,
            logs_pull,
            logs_ack,
        ])
        .run(tauri::generate_context!())
        .expect("tauri 应用启动失败");
}
