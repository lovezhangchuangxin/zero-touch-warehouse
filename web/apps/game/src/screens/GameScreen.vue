<script setup lang="ts">
import CodeEditor from "../ui/CodeEditor.vue";
import DebugBar from "../ui/DebugBar.vue";
import EventPanel from "../ui/EventPanel.vue";
import FaultBanner from "../ui/FaultBanner.vue";
import GameCanvas from "../ui/GameCanvas.vue";
import LogPanel from "../ui/LogPanel.vue";
import MarketPanel from "../ui/MarketPanel.vue";
import RobotPanel from "../ui/RobotPanel.vue";
import Splitter from "../ui/Splitter.vue";
import StatusBar from "../ui/StatusBar.vue";
import PauseOverlay from "./PauseOverlay.vue";
import * as api from "../api";
import { store } from "../store";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

// 游戏屏：分栏网格 + 抽屉 + 状态栏。主菜单 / 设置浮层期间仍保持挂载
// （菜单是世界之上的浮层，见 App.vue），编辑器草稿因此跨菜单存活。
const props = defineProps<{ overlayOpen: boolean; menuOpen: boolean }>();
const emit = defineEmits<{ openMenu: []; openSettings: [] }>();

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

// ESC 暂停浮层：打开即暂停世界，"继续"按打开时的速度恢复。编辑器内
// 的 Esc（补全取消 / 搜索关闭 / 重命名取消）都会 preventDefault，这里
// 靠 defaultPrevented 让位；浮层（主菜单 / 设置）打开时不响应。
const pauseOpen = ref(false);
const pauseTps = ref(5);
function togglePause() {
  if (pauseOpen.value) {
    closePause();
  } else {
    pauseTps.value = store.snapshot?.tps || 5;
    pauseOpen.value = true;
    void api.pause();
  }
}
function closePause() {
  if (!pauseOpen.value) return;
  pauseOpen.value = false;
  void api.resume(pauseTps.value);
}
function onKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape" || e.repeat || props.overlayOpen) return;
  if (e.defaultPrevented) return;
  e.preventDefault();
  togglePause();
}
onMounted(() => window.addEventListener("keydown", onKeydown));
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));
// 主菜单 / 设置打开时只摘掉暂停浮层，不得顺手恢复世界——"回主菜单
// 保持暂停"依赖这一点；恢复仅由显式"继续"（按钮 / 点背景 / ESC）触发。
watch(
  () => props.overlayOpen,
  (open) => {
    if (open) pauseOpen.value = false;
  },
);

// "开始"转场彩蛋：每次主菜单关闭（= 开始对局）编辑器列自右侧滑入。
const editorEnter = ref(false);
let editorEnterTimer: number | undefined;
watch(
  () => props.menuOpen,
  (open, before) => {
    if (before && !open) {
      editorEnter.value = true;
      clearTimeout(editorEnterTimer);
      editorEnterTimer = window.setTimeout(() => (editorEnter.value = false), 450);
    }
  },
);
onBeforeUnmount(() => clearTimeout(editorEnterTimer));
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
      <!-- 编辑器列：全高；"开始"时自右滑入（与主菜单列同一书脊） -->
      <Splitter dir="x" @drag="onEdDrag" @end="persist" @reset="resetEd" />
      <div class="grid min-h-0" :class="editorEnter && 'editor-enter'">
        <CodeEditor class="min-h-0" />
      </div>
    </main>
    <StatusBar />
    <PauseOverlay
      v-if="pauseOpen"
      :tps="pauseTps"
      @close="closePause"
      @open-settings="emit('openSettings')"
      @open-menu="emit('openMenu')"
    />
  </div>
</template>
