// 响应式应用状态（仅 DOM 面板消费；Pixi 对象一律不进本层，
// docs/architecture/05「Pixi 对象不进入 Vue 响应式系统」）。
//
// 快照：channel 推送 → 记录 prev/cur（渲染插值用）→ 立即 ack。
// 诊断/日志：150ms 游标轮询 + 确认，gap 时提示“部分历史已过期”。

import { reactive } from "vue";
import * as api from "./api";
import type { DiagEvent, Snapshot, StatusView, StaticInfo } from "./types";

const UI_EVENT_KEEP = 600;
const UI_LOG_KEEP = 400;
const POLL_MS = 150;

export const store = reactive({
  snapshot: null as Snapshot | null,
  prev: null as Snapshot | null,
  static: null as StaticInfo | null,
  status: null as StatusView | null,
  diagEvents: [] as DiagEvent[],
  diagCursor: 0,
  diagGap: null as { from: number; to: number } | null,
  logLines: [] as { seq: number; tick: number; line: string }[],
  logCursor: 0,
  selectedRobot: null as number | null,
  /** 编辑器草稿（保存并重载时提交给 hot_reload）。 */
  code: "",
  lastScenario: "",
});

export function onSnapshot(s: Snapshot): void {
  // 场景切换（reset）后清面板积压：事件环 seq 单调延续，但 tick 语义重开。
  if (store.lastScenario !== "" && s.scenario !== store.lastScenario) {
    store.diagEvents = [];
    store.logLines = [];
    store.diagGap = null;
  }
  store.lastScenario = s.scenario;
  store.prev = store.snapshot;
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
