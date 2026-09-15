<script setup lang="ts">
import { computed } from "vue";
import { codeLabel } from "../codes";
import { vFade } from "./fade";
import { store } from "../store";
import type { DiagEvent } from "../types";

// 事件面板（docs/architecture/05 §调试器）：受理失败 / 结算 / 管理操作 /
// 订单完成 / 故障 / 控制事件，按游标分页；游标超窗时明确提示历史已过期。
const events = computed(() => [...store.diagEvents].reverse());

const KIND_LABELS: Record<string, string> = {
  accept_fail: "受理失败",
  settle: "结算",
  manage: "管理",
  order_done: "订单完成",
  fault: "故障",
  control: "控制",
  log: "日志",
};

function describe(e: DiagEvent): string {
  const p = e.payload;
  switch (e.kind) {
    case "settle":
      return `机器人#${p.robot_id} ${String(p.action)}(${String(p.arg)}) → ${String(p.code)} ${codeLabel(String(p.code))}`;
    case "accept_fail": {
      const op = String(p.op ?? "?");
      const code = String(p.code ?? "?");
      const detail = p.detail ? `（${String(p.detail)}）` : "";
      const subj = p.subject ? ` #${String(p.subject)}` : "";
      return `${op}${subj} → ${code} ${codeLabel(code)}${detail}`;
    }
    case "manage":
      return `#${String(p.subject ?? "?")} ${String(p.detail ?? "")}`;
    case "order_done":
      return `订单#${String(p.subject ?? "?")} ${String(p.detail ?? "")}`;
    case "fault":
      return `${String(p.class ?? "")} ${String(p.code ?? "")}：${String(p.message ?? "")}`;
    case "control":
      return `${String(p.op ?? "")}${p.ok === true ? "" : "（失败）"} ${String(p.error ?? "")}`.trim();
    default:
      return JSON.stringify(p);
  }
}

function kindClass(kind: string): string {
  switch (kind) {
    case "fault":
      return "text-bad";
    case "accept_fail":
      return "text-warn";
    case "order_done":
      return "text-ok";
    default:
      return "text-dim";
  }
}
</script>

<template>
  <div class="grid min-h-0 grid-rows-[auto_minmax(0,1fr)]">
    <div
      v-if="store.diagGap"
      class="mx-2 my-1.5 rounded-sm border border-dashed border-warn px-2 py-1 text-xs text-warn"
    >
      部分历史已过期：事件 #{{ store.diagGap.from }}–{{ store.diagGap.to }} 已被覆盖
      （诊断环有界，docs/architecture/02）
    </div>
    <div v-fade class="min-h-0 flex-1 overflow-auto px-2 pb-2 pt-1">
      <div v-for="e in events" :key="e.seq" class="flex gap-2 py-px text-xs">
        <span class="min-w-10 text-right font-mono text-dim">{{ e.tick }}</span>
        <span class="min-w-14" :class="kindClass(e.kind)">{{ KIND_LABELS[e.kind] ?? e.kind }}</span>
        <span>{{ describe(e) }}</span>
      </div>
      <div v-if="events.length === 0" class="px-2 py-2 text-dim">（暂无事件）</div>
    </div>
  </div>
</template>
