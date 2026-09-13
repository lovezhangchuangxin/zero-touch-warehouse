<script setup lang="ts">
import { computed, ref, watch } from "vue";
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
const flood = ref(false);
watch(
  () => store.logLines.length,
  (n, old) => {
    if (old != null && n - old > 60) {
      flood.value = true; // 刷屏提示：日志被节流显示，故障摘要不受影响
    }
  },
);
</script>

<template>
  <div class="wrap">
    <div class="tools">
      <input v-model="filter" placeholder="过滤…" class="filter" />
      <span v-if="flood" class="dim">日志刷屏中：仅显示最近条目</span>
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
.dim {
  color: var(--dim);
}
</style>
