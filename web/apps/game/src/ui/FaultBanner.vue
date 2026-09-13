<script setup lang="ts">
import type { FaultView } from "../types";

const props = defineProps<{ fault: FaultView }>();

const CLASS_LABELS: Record<string, string> = {
  script: "脚本错误",
  environment: "环境级故障",
};
function classLabel(cls: string): string {
  if (CLASS_LABELS[cls]) {
    return CLASS_LABELS[cls];
  }
  if (cls.startsWith("host_terminated:")) {
    return `宿主终止（${cls.slice("host_terminated:".length)}）`;
  }
  return cls;
}
void props;
</script>

<template>
  <div class="banner">
    <div class="head">
      <strong>{{ classLabel(fault.class) }}</strong>
      <code class="code">{{ fault.code }}</code>
      <span class="dim">tick {{ fault.tick }}</span>
    </div>
    <div class="msg">{{ fault.message }}</div>
    <div v-if="fault.stack" class="stack mono">{{ fault.stack }}</div>
    <div class="meta dim mono">
      最后已提交请求 #{{ fault.last_request_id }} {{ fault.last_op || "—" }} ｜ 本执行已应答
      {{ fault.requests_served }} 条
    </div>
  </div>
</template>

<style scoped>
.banner {
  position: absolute;
  left: 12px;
  right: 12px;
  bottom: 12px;
  background: rgba(46, 22, 26, 0.92);
  border: 1px solid var(--bad);
  border-radius: 6px;
  padding: 8px 12px;
  z-index: 5;
}
.head {
  display: flex;
  gap: 10px;
  align-items: baseline;
}
.code {
  color: var(--bad);
}
.msg {
  margin-top: 4px;
  white-space: pre-wrap;
}
.stack {
  margin-top: 4px;
  color: var(--dim);
  white-space: pre-wrap;
  max-height: 90px;
  overflow: auto;
}
.meta {
  margin-top: 4px;
}
</style>
