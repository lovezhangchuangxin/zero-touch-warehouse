<script setup lang="ts">
import { computed, ref } from "vue";
import * as api from "../api";
import { fmtMilli } from "../format";
import { store } from "../store";
import Select from "./Select.vue";

// 顶栏只留操作，按「传输控制 / 速度档 / 场景 / 资金读数 / 危险区」分组；
// tick / 运行态等持续读数在底部状态栏（StatusBar）。
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
const scenarios = computed(() => store.static?.scenarios ?? []);
const scenarioSel = ref("");
const scenarioOptions = computed(() =>
  scenarios.value.map((s) => ({ value: s.id, label: `${s.name}（${s.robots} 台）` })),
);
// 快照未到时不显示 0.00（误导）；欠款判断用原始 milli 值而非格式化串，
// 1–9 milli 截断后也是 "0.00" 但欠款真实存在。
const gold = computed(() => (store.snapshot ? fmtMilli(store.snapshot.gold_milli) : null));
const hasDebt = computed(() => Number(store.snapshot?.debt_milli ?? "0") > 0);
const debt = computed(() => (store.snapshot ? fmtMilli(store.snapshot.debt_milli) : ""));

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
    <span class="v-sep" />
    <button class="btn" :disabled="!running" @click="onPause">暂停</button>
    <button class="btn" :disabled="running && !faulted" @click="onResume">
      {{ faulted ? "恢复（处理故障）" : "运行" }}
    </button>
    <button class="btn" :disabled="running && !faulted" @click="onStep">单步</button>
    <span class="v-sep" />
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
    <span class="v-sep" />
    <Select
      v-model="scenarioSel"
      class="max-w-44"
      :options="scenarioOptions"
      placeholder="重开场景…"
      aria-label="重开场景"
    />
    <button class="btn" @click="onReset">重开</button>
    <span class="flex-1" />
    <!-- 资金读数：玩家持有的金币（含借贷欠款提示） -->
    <span
      v-if="gold !== null"
      class="inline-flex items-baseline gap-1.5 rounded-sm border border-warn/30 bg-warn/10 px-2 py-0.5"
      title="玩家持有的金币"
    >
      <span class="text-2xs text-dim">金币</span>
      <b class="font-mono text-warn">{{ gold }}</b>
    </span>
    <span
      v-if="hasDebt"
      class="inline-flex items-baseline gap-1.5 rounded-sm border border-bad/30 bg-bad/10 px-2 py-0.5"
      title="借贷欠款"
    >
      <span class="text-2xs text-dim">欠款</span>
      <b class="font-mono text-bad">{{ debt }}</b>
    </span>
    <span class="v-sep" />
    <button
      class="btn btn-danger"
      title="独立控制路径：直接终止宿主进程，不经命令队列"
      @click="onKill"
    >
      终止宿主
    </button>
  </header>
</template>
