<script setup lang="ts">
import { computed, ref } from "vue";
import { store } from "../store";

// 日志面板：Game.log 输出 + 故障摘要置顶（快照内的 fault 不受刷屏影响，
// 见 FaultBanner 与 docs/architecture/02「日志有单独配额」）。
const lines = computed(() => [...store.logLines].reverse());
const filter = ref("");
const filtered = computed(() =>
  filter.value === ""
    ? lines.value
    : lines.value.filter((l) => l.line.includes(filter.value)),
);
// 刷屏提示按轮询页新增量判定：一次 150ms 窗口新增 >60 条即视为刷屏，
// 下一页节奏回落后提示自然消失（不受 UI_LOG_KEEP 裁剪干扰）。
const flood = computed(() => store.logBurst > 60);
</script>

<template>
  <div class="wrap">
    <div class="tools">
      <input v-model="filter" placeholder="过滤…" class="filter" />
      <span v-if="flood" class="warn">日志刷屏中：仅显示最近条目</span>
    </div>
    <div class="list mono">
      <div v-for="l in filtered" :key="l.seq" class="line">
        <span class="dim tick">{{ l.tick }}</span>
        <span class="text">{{ l.line }}</span>
      </div>
      <div v-if="filtered.length === 0" class="dim empty">（暂无 Game.log 输出）</div>
    </div>
  </div>
</template>

<style scoped>
.wrap {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  min-height: 0;
}
.tools {
  display: flex;
  gap: 10px;
  align-items: center;
  padding: 4px 8px 0;
}
.filter {
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 4px;
  padding: 2px 8px;
  width: 160px;
}
.list {
  overflow: auto;
  padding: 4px 8px 8px;
}
.line {
  display: flex;
  gap: 10px;
  white-space: pre-wrap;
}
.tick {
  min-width: 36px;
  text-align: right;
  color: var(--dim);
}
.empty {
  padding: 8px;
}
.warn {
  color: #e8a24a;
}

.dim {
  color: var(--dim);
}
</style>
