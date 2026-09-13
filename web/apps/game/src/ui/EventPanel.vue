<script setup lang="ts">
import { computed } from "vue";
import { codeLabel } from "../codes";
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
      return "bad";
    case "accept_fail":
      return "warn";
    case "order_done":
      return "ok";
    default:
      return "dim";
  }
}
</script>

<template>
  <div class="wrap">
    <div v-if="store.diagGap" class="gap">
      部分历史已过期：事件 #{{ store.diagGap.from }}–{{ store.diagGap.to }} 已被覆盖
     （诊断环有界，docs/architecture/02）
    </div>
    <div class="list">
      <div v-for="e in events" :key="e.seq" class="ev">
        <span class="mono dim tick">{{ e.tick }}</span>
        <span class="kind" :class="kindClass(e.kind)">{{ KIND_LABELS[e.kind] ?? e.kind }}</span>
        <span class="text">{{ describe(e) }}</span>
      </div>
      <div v-if="events.length === 0" class="dim empty">（暂无事件）</div>
    </div>
  </div>
</template>

<style scoped>
.wrap {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  min-height: 0;
}
.gap {
  margin: 6px 8px;
  padding: 4px 8px;
  border: 1px dashed var(--warn);
  border-radius: 4px;
  color: var(--warn);
  font-size: 12px;
}
.list {
  overflow: auto;
  padding: 4px 8px 8px;
}
.ev {
  display: flex;
  gap: 8px;
  font-size: 12px;
  padding: 1px 0;
}
.tick {
  min-width: 40px;
  text-align: right;
}
.kind {
  min-width: 56px;
}
.kind.bad {
  color: var(--bad);
}
.kind.warn {
  color: var(--warn);
}
.kind.ok {
  color: var(--ok);
}
.dim {
  color: var(--dim);
}
.empty {
  padding: 8px;
}
</style>
