<script setup lang="ts">
import { computed, ref } from "vue";
import * as api from "../api";
import { store } from "../store";

// 常速 1–10 / 快进 50–200（docs/architecture/02 §变速与暂停的参考档位）。
const SPEEDS = [
  { v: 1, ff: false },
  { v: 2, ff: false },
  { v: 5, ff: false },
  { v: 10, ff: false },
  { v: 50, ff: true },
  { v: 100, ff: true },
  { v: 200, ff: true },
];
const speed = ref(5);
// 运行态优先取快照（每帧更新）；store.status 仅启动拉取一次，会陈旧。
const running = computed(() => store.snapshot?.running ?? store.status?.running ?? false);
const faultClass = computed(
  () => store.snapshot?.fault?.class ?? store.status?.fault_class ?? null,
);
const faulted = computed(() => faultClass.value != null);
const tick = computed(() => store.snapshot?.tick ?? store.status?.tick ?? 0);
const scenarios = computed(() => store.static?.scenarios ?? []);
const scenarioSel = ref("");

function onPause() {
  void api.pause();
}
function onResume() {
  void api.resume(speed.value);
}
function onStep() {
  void api.step();
}
function onSpeed(v: number) {
  speed.value = v;
  if (running.value) {
    void api.resume(v);
  }
}
function onKill() {
  void api.killHost();
}
function onReset() {
  const id = scenarioSel.value || store.snapshot?.scenario || store.status?.scenario || "b2-one";
  void api.resetScenario(id);
}
</script>

<template>
  <header
    class="flex flex-wrap items-center gap-2 rounded-md border border-line bg-panel px-2.5 py-1.5"
  >
    <span class="font-semibold">无人仓库 <em class="not-italic text-accent">B2</em></span>
    <span class="font-mono text-xs text-dim">tick {{ tick }}</span>
    <button class="btn" :disabled="!running" @click="onPause">暂停</button>
    <button class="btn" :disabled="running && !faulted" @click="onResume">
      {{ faulted ? "恢复（处理故障）" : "运行" }}
    </button>
    <button class="btn" :disabled="running && !faulted" @click="onStep">单步</button>
    <span class="inline-flex gap-1">
      <button
        v-for="s in SPEEDS"
        :key="s.v"
        class="btn"
        :class="[speed === s.v ? 'btn-on' : '', s.ff ? 'text-warn' : '']"
        :title="s.ff ? '快进' : '常速'"
        @click="onSpeed(s.v)"
      >
        {{ s.v }}×
      </button>
    </span>
    <button
      class="btn btn-danger"
      title="独立控制路径：直接终止宿主进程，不经命令队列"
      @click="onKill"
    >
      终止宿主
    </button>
    <select v-model="scenarioSel" class="field" aria-label="重开场景">
      <option value="" disabled>重开场景…</option>
      <!-- 标签整体放单个插值：oxfmt 强制子节点分行，裸文本首尾空白会被编译进 DOM 文本 -->
      <option v-for="s in scenarios" :key="s.id" :value="s.id">
        {{ `${s.name}（${s.robots} 台）` }}
      </option>
    </select>
    <button class="btn" @click="onReset">重开</button>
    <span class="flex-1" />
    <span class="text-dim">{{ store.static?.name ?? "" }}</span>
    <span v-if="store.snapshot" :class="running ? 'text-ok' : 'text-dim'">
      {{ running ? `运行中 ${store.snapshot.tps}/s` : "已暂停" }}
    </span>
    <span v-if="!store.snapshot?.loaded" class="text-warn">未加载代码</span>
  </header>
</template>
