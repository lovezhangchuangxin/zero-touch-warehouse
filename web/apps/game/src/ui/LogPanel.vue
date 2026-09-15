<script setup lang="ts">
import { computed, ref } from "vue";
import { vFade } from "./fade";
import { store } from "../store";

// 日志面板：Game.log 输出 + 故障摘要置顶（快照内的 fault 不受刷屏影响，
// 见 FaultBanner 与 docs/architecture/02「日志有单独配额」）。
const lines = computed(() => [...store.logLines].reverse());
const filter = ref("");
const filtered = computed(() =>
  filter.value === "" ? lines.value : lines.value.filter((l) => l.line.includes(filter.value)),
);
// 刷屏提示按轮询页新增量判定：一次 150ms 窗口新增 >60 条即视为刷屏，
// 下一页节奏回落后提示自然消失（不受 UI_LOG_KEEP 裁剪干扰）。
const flood = computed(() => store.logBurst > 60);
</script>

<template>
  <div class="grid min-h-0 grid-rows-[auto_minmax(0,1fr)]">
    <div class="flex items-center gap-2.5 px-2 pt-1">
      <input v-model="filter" placeholder="过滤…" class="field w-40" />
      <span v-if="flood" class="text-warn">日志刷屏中：仅显示最近条目</span>
    </div>
    <div v-fade class="min-h-0 flex-1 overflow-auto px-2 pb-2 pt-1 font-mono text-xs">
      <div v-for="l in filtered" :key="l.seq" class="flex gap-2.5 whitespace-pre-wrap">
        <span class="min-w-9 text-right text-dim">{{ l.tick }}</span>
        <span>{{ l.line }}</span>
      </div>
      <div v-if="filtered.length === 0" class="px-2 py-2 text-dim">（暂无 Game.log 输出）</div>
    </div>
  </div>
</template>
