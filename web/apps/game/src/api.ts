// Tauri command 封装（docs/architecture/05 §与 Rust 的通信）：
// 快照经 channel 推送 + 收到即 ack（世界线程侧合并丢旧）；诊断与日志按
// 游标分页拉取 / 确认；控制命令入队后即返回，状态以快照/轮询为准。

import { Channel, invoke } from "@tauri-apps/api/core";
import type { DiagPage, Snapshot, StatusView, StaticInfo } from "./types";

export function attachChannel(onMessage: (s: Snapshot) => void): Promise<void> {
  const channel = new Channel<Snapshot>();
  channel.onmessage = (s) => onMessage(s);
  return invoke("attach", { channel });
}

export function ackSnapshot(): Promise<void> {
  return invoke("ack_snapshot");
}

export function latestSnapshot(): Promise<string | null> {
  return invoke("latest_snapshot");
}

export function fetchStatus(): Promise<StatusView> {
  return invoke("status");
}

export function fetchStaticInfo(): Promise<StaticInfo> {
  return invoke("static_info");
}

export function pause(): Promise<void> {
  return invoke("pause");
}

export function resume(tps: number): Promise<void> {
  return invoke("resume", { tps });
}

export function step(): Promise<void> {
  return invoke("step");
}

/** 热重载：文件集整包提交 + 入口文件名（协议 v3，多文件）。 */
export function hotReload(
  files: Record<string, string>,
  entry: string,
  language: string,
): Promise<void> {
  return invoke("hot_reload", { files, entry, language });
}

/** 重开场景。keepProgram = false（主菜单"开始新场景"）不保留玩家程序，
 *  落进未加载代码的干净初态；默认 true（游戏内"重开"）。 */
export function resetScenario(scenario: string, keepProgram = true): Promise<void> {
  return invoke("reset", { scenario, keepProgram });
}

/** 退出应用（主菜单"退出"）。 */
export function quit(): Promise<void> {
  return invoke("quit");
}

export function killHost(): Promise<void> {
  return invoke("kill_host");
}

export function diagPull(cursor: number, limit: number): Promise<DiagPage> {
  return invoke("diag_pull", { cursor, limit });
}

export function diagAck(cursor: number): Promise<void> {
  return invoke("diag_ack", { cursor });
}

export function logsPull(cursor: number, limit: number): Promise<DiagPage> {
  return invoke("logs_pull", { cursor, limit });
}

export function logsAck(cursor: number): Promise<void> {
  return invoke("logs_ack", { cursor });
}
