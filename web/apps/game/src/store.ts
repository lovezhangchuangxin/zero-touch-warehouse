// 响应式应用状态（仅 DOM 面板消费；Pixi 对象一律不进本层，
// docs/architecture/05「Pixi 对象不进入 Vue 响应式系统」）。
//
// 快照：channel 推送 → 更新 cur → 立即 ack（插值由 Stage 内部的
// RobotAnim from/to 承担，无需 prev 快照）。
// 诊断/日志：150ms 游标轮询 + 确认，gap 时提示“部分历史已过期”。

import { reactive } from "vue";
import * as api from "./api";
import type { DiagEvent, Snapshot, StatusView, StaticInfo } from "./types";

const UI_EVENT_KEEP = 600;
const UI_LOG_KEEP = 400;
const POLL_MS = 150;

export const store = reactive({
  snapshot: null as Snapshot | null,
  static: null as StaticInfo | null,
  status: null as StatusView | null,
  diagEvents: [] as DiagEvent[],
  diagCursor: 0,
  diagGap: null as { from: number; to: number } | null,
  logLines: [] as { seq: number; tick: number; line: string }[],
  /** 最近一次轮询新增的日志条数（刷屏提示用，节奏回落自然复位）。 */
  logBurst: 0,
  logCursor: 0,
  selectedRobot: null as number | null,
  lastScenario: "",
});

export function onSnapshot(s: Snapshot): void {
  // 场景切换（reset）后清面板积压并重拉场景静态信息：墙格 / 尺寸 /
  // 装卸位足迹随场景重建，不重拉则画布永远停在旧场景（渲染以
  // snap.scenario !== static.id 早退）。事件环 seq 单调延续，tick 语义重开。
  if (store.lastScenario !== "" && s.scenario !== store.lastScenario) {
    store.diagEvents = [];
    store.logLines = [];
    store.diagGap = null;
    void api
      .fetchStaticInfo()
      .then((v) => {
        store.static = v;
      })
      .catch((e) => console.warn("场景静态信息重拉失败", e));
  }
  store.lastScenario = s.scenario;
  store.snapshot = s;
  void api.ackSnapshot();
}

async function pollDiagAndLogs(): Promise<void> {
  try {
    const page = await api.diagPull(store.diagCursor, 200);
    if (page.gap) {
      store.diagGap = page.gap;
    }
    if (page.events.length > 0) {
      store.diagEvents.push(...page.events);
      if (store.diagEvents.length > UI_EVENT_KEEP) {
        store.diagEvents.splice(0, store.diagEvents.length - UI_EVENT_KEEP);
      }
    }
    store.diagCursor = page.next;
    void api.diagAck(page.next);

    const lp = await api.logsPull(store.logCursor, 200);
    store.logBurst = lp.events.length;
    if (lp.events.length > 0) {
      const lines = lp.events.map((e) => ({
        seq: e.seq,
        tick: e.tick,
        line: String(e.payload.line ?? ""),
      }));
      store.logLines.push(...lines);
      if (store.logLines.length > UI_LOG_KEEP) {
        store.logLines.splice(0, store.logLines.length - UI_LOG_KEEP);
      }
    }
    store.logCursor = lp.next;
    void api.logsAck(lp.next);
  } catch (err) {
    console.warn("诊断轮询失败（世界线程可能未就绪）", err);
  }
}

export async function bootstrap(): Promise<void> {
  store.static = await api.fetchStaticInfo();
  store.status = await api.fetchStatus();
  const latest = await api.latestSnapshot();
  if (latest) {
    onSnapshot(JSON.parse(latest) as Snapshot);
  }
  await api.attachChannel(onSnapshot);
  window.setInterval(() => void pollDiagAndLogs(), POLL_MS);
}
