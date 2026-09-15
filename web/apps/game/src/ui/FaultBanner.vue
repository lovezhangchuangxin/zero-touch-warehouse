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
  <div
    class="absolute inset-x-3 bottom-3 z-10 rounded-md border border-bad bg-bad-deep/95 px-3 py-2 shadow-lg shadow-black/40"
  >
    <div class="flex items-baseline gap-2.5">
      <strong>{{ classLabel(fault.class) }}</strong>
      <code class="text-bad">{{ fault.code }}</code>
      <span class="text-dim">tick {{ fault.tick }}</span>
    </div>
    <div class="mt-1 whitespace-pre-wrap">{{ fault.message }}</div>
    <div
      v-if="fault.stack"
      class="mt-1 max-h-[90px] overflow-auto whitespace-pre-wrap font-mono text-xs text-dim"
    >
      {{ fault.stack }}
    </div>
    <div class="mt-1 font-mono text-xs text-dim">
      最后已提交请求 #{{ fault.last_request_id }} {{ fault.last_op || "—" }} ｜ 本执行已应答
      {{ fault.requests_served }} 条
    </div>
  </div>
</template>
