<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import * as api from "../api";
import { fileNameFor } from "../editor/files";
import GameCanvas from "../ui/GameCanvas.vue";
import { DEMO_SCRIPTS } from "../scripts";
import { store } from "../store";

// 主菜单：对局世界之上的常驻浮层（非独立屏）。冷启动跑吸引模式——
// 给世界载入内置演示脚本慢速空转，渐晕之下就是一支活着的仓库广告片；
// 对局进行中回主菜单则不打扰世界，背景就是玩家自己的仓库。
const emit = defineEmits<{ start: [string]; openSettings: [] }>();

const scenarios = computed(() => store.static?.scenarios ?? []);
const pickerOpen = ref(false);

onMounted(() => {
  if (store.inGame) return;
  void attract();
});

/** 吸引模式：演示脚本 + 慢速空转。进真实对局时 start 会整包重开覆盖。 */
async function attract() {
  try {
    await api.resetScenario("b2-one", false);
    if (store.inGame) return; // 用户已点开始：后续演示命令不得再扰动对局
    const demo = DEMO_SCRIPTS.find((d) => d.id === "demo-one");
    if (demo) {
      const entry = fileNameFor(demo.language);
      await api.hotReload({ [entry]: demo.code }, entry, demo.language);
    }
    if (store.inGame) return;
    await api.resume(2);
  } catch {
    // 桥未就绪（如纯浏览器预览）：静默，菜单仍可用
  }
}

function onStart(id: string) {
  pickerOpen.value = false;
  emit("start", id); // inGame 置位与重开由宿主 App 统一处理
}

function onQuit() {
  try {
    void api.quit();
  } catch {
    // 纯浏览器预览无 Tauri 桥：同步抛错，静默
  }
}
</script>

<template>
  <div class="absolute inset-0 z-20">
    <!-- 背景：全屏专用画布，只渲染世界本身（盖住游戏屏的 HUD 区）。
         外层用 grid：画布根是普通块级子元素，block 容器下高度会塌。 -->
    <div class="absolute inset-0 grid"><GameCanvas background /></div>
    <!-- 渐晕：世界整体压暗，边缘更重（径向），中央菜单区最亮 -->
    <div class="absolute inset-0 bg-bg/40" />
    <div
      class="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_30%,rgba(18,20,25,0.65)_100%)]"
    />
    <div class="absolute inset-0 flex flex-col items-center justify-center pb-14">
      <div
        class="flex flex-col items-center rounded-lg border border-line/60 bg-bg/80 px-12 py-9 shadow-lg shadow-black/40"
      >
        <header class="text-center">
          <h1 class="text-5xl font-bold tracking-widest text-fg">无人仓库</h1>
        </header>
        <nav class="mt-10 flex w-72 flex-col items-stretch gap-1.5">
          <!-- 开始：列内展开场景卡片（首版场景少，独立关卡选择屏留待内容成体系） -->
          <button
            class="menu-item"
            :class="pickerOpen && 'menu-item-on'"
            @click="pickerOpen = !pickerOpen"
          >
            <span>开始</span>
            <span v-if="pickerOpen" class="menu-hint">收起</span>
          </button>
          <div v-if="pickerOpen" class="flex flex-col gap-1.5 pb-2">
            <div v-if="scenarios.length === 0" class="px-3 py-2 text-sm text-dim">
              场景列表加载中…
            </div>
            <button
              v-for="s in scenarios"
              :key="s.id"
              class="rounded-sm border border-line bg-panel/80 px-3 py-2 text-center transition-colors hover:border-accent hover:bg-panel-2"
              @click="onStart(s.id)"
            >
              <span class="text-sm font-semibold text-fg">{{ s.name }}</span>
              <span class="ml-2 text-2xs text-dim">{{ s.robots }} 台机器人</span>
              <span v-if="s.desc" class="mt-0.5 block text-2xs text-dim">{{ s.desc }}</span>
            </button>
          </div>
          <button
            class="menu-item cursor-not-allowed opacity-40"
            title="存档功能将于后续版本提供"
            disabled
          >
            <span>继续</span>
          </button>
          <button class="menu-item" @click="emit('openSettings')"><span>设置</span></button>
          <button class="menu-item" @click="onQuit"><span>退出</span></button>
        </nav>
      </div>
      <footer class="absolute bottom-5 text-2xs text-dim/70">无人仓库 · 原型 B2</footer>
    </div>
  </div>
</template>
