<script setup lang="ts">
import CodeEditor from "./ui/CodeEditor.vue";
import DebugBar from "./ui/DebugBar.vue";
import EventPanel from "./ui/EventPanel.vue";
import FaultBanner from "./ui/FaultBanner.vue";
import GameCanvas from "./ui/GameCanvas.vue";
import LogPanel from "./ui/LogPanel.vue";
import MarketPanel from "./ui/MarketPanel.vue";
import RobotPanel from "./ui/RobotPanel.vue";
import Splitter from "./ui/Splitter.vue";
import StatusBar from "./ui/StatusBar.vue";
import { store } from "./store";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";

// 抽屉页签：事件 / 日志 / 机器人 / 市场（四合一底部抽屉）。
const tab = ref<"events" | "logs" | "robots" | "market">("events");
const fault = computed(() => store.snapshot?.fault ?? null);

// 分栏尺寸（px）：编辑器列宽 + 抽屉高度，localStorage 持久化。
// 钳制保证画布至少保留 ~480×300 的可视区。
const LAYOUT_KEY = "ztw.layout.v1";
const ED_W_DEFAULT = 400;
const DR_H_DEFAULT = 220;
function loadLayout(): { edW: number; drH: number } {
  try {
    const v = JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? "");
    if (typeof v?.edW === "number" && typeof v?.drH === "number") return v;
  } catch {
    // 损坏即回默认
  }
  return { edW: ED_W_DEFAULT, drH: DR_H_DEFAULT };
}
const edW = ref(loadLayout().edW);
const drH = ref(loadLayout().drH);
function clampEdW(v: number): number {
  return Math.round(Math.min(Math.max(300, window.innerWidth - 480), Math.max(300, v)));
}
function clampDrH(v: number): number {
  return Math.round(Math.min(Math.max(120, window.innerHeight - 400), Math.max(120, v)));
}
function persist() {
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify({ edW: edW.value, drH: drH.value }));
  } catch {
    // 隐私模式等存储不可用：忽略，仅本次会话生效
  }
}
function onEdDrag(d: number) {
  edW.value = clampEdW(edW.value - d); // 向左拖 = 编辑器加宽
}
function onDrDrag(d: number) {
  drH.value = clampDrH(drH.value - d); // 向上拖 = 抽屉加高
}
function resetEd() {
  edW.value = ED_W_DEFAULT;
  persist();
}
function resetDr() {
  drH.value = DR_H_DEFAULT;
  persist();
}
// 窗口 resize 后重钳制：分栏存的是绝对 px，外层不滚动且滚动条隐藏，
// 不重钳的话缩窗会把画布挤没且无从察觉。
function onWindowResize() {
  const ed = clampEdW(edW.value);
  const dr = clampDrH(drH.value);
  if (ed !== edW.value || dr !== drH.value) {
    edW.value = ed;
    drH.value = dr;
    persist();
  }
}
onMounted(() => window.addEventListener("resize", onWindowResize));
onBeforeUnmount(() => window.removeEventListener("resize", onWindowResize));
</script>

<template>
  <div class="grid h-full grid-rows-[auto_1fr_auto] gap-1.5 p-1.5">
    <DebugBar />
    <main
      class="grid min-h-0"
      :style="{ gridTemplateColumns: `minmax(0,1fr) 6px ${clampEdW(edW)}px` }"
    >
      <!-- 左列：画布 + 抽屉 -->
      <section
        class="grid min-h-0"
        :style="{ gridTemplateRows: `minmax(0,1fr) 6px ${clampDrH(drH)}px` }"
      >
        <div class="relative grid min-h-0">
          <GameCanvas />
          <FaultBanner v-if="fault" :fault="fault" />
        </div>
        <Splitter dir="y" @drag="onDrDrag" @end="persist" @reset="resetDr" />
        <!-- 底部抽屉：四页签 -->
        <section
          class="flex min-h-0 flex-col overflow-hidden rounded-md border border-line bg-panel"
        >
          <div class="flex items-center gap-1 border-b border-line px-2">
            <button class="tab" :class="tab === 'events' && 'tab-on'" @click="tab = 'events'">
              事件
            </button>
            <button class="tab" :class="tab === 'logs' && 'tab-on'" @click="tab = 'logs'">
              日志
            </button>
            <button class="tab" :class="tab === 'robots' && 'tab-on'" @click="tab = 'robots'">
              机器人
            </button>
            <button class="tab" :class="tab === 'market' && 'tab-on'" @click="tab = 'market'">
              市场
            </button>
          </div>
          <div class="min-h-0 flex-1">
            <EventPanel v-show="tab === 'events'" class="h-full" />
            <LogPanel v-show="tab === 'logs'" class="h-full" />
            <RobotPanel v-show="tab === 'robots'" class="h-full" />
            <MarketPanel v-show="tab === 'market'" class="h-full" />
          </div>
        </section>
      </section>
      <!-- 编辑器列：全高 -->
      <Splitter dir="x" @drag="onEdDrag" @end="persist" @reset="resetEd" />
      <CodeEditor class="min-h-0" />
    </main>
    <StatusBar />
  </div>
</template>
