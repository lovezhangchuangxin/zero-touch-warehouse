<script setup lang="ts">
// 画布挂载：Vue 侧只做桥接（挂载/卸载与快照喂入），Pixi 对象与渲染
// 节奏全部在 render/Stage 内，不进响应式系统（docs/architecture/05）。
// background 用于主菜单全屏背景：无操作提示、无边框圆角、不接收指针
// 事件（世界仍照常渲染）。prop 取反命名是刻意的：Vue 会把缺省布尔
// 铸成 false，"不传 = 游戏屏交互画布"恰好与铸造默认一致。
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Stage } from "../render/Stage";
import { store } from "../store";

const props = defineProps<{ background?: boolean }>();

const el = ref<HTMLDivElement>();
const ready = ref(false);
// 卸载竞态守卫：菜单背景画布随浮层频繁挂载/卸载，若 Stage.create 完成
// 前组件已卸载，必须销毁刚落地的实例，否则 WebGL 上下文与 rAF 泄漏。
let stage: Stage | null = null;
let disposed = false;

onMounted(async () => {
  if (!el.value) {
    return;
  }
  const created = await Stage.create(el.value);
  if (disposed) {
    created.destroy();
    return;
  }
  stage = created;
  stage.setStatic(store.static);
  if (store.snapshot) {
    stage.render(store.snapshot, store.selectedRobot);
  }
  ready.value = true;
});

onBeforeUnmount(() => {
  disposed = true;
  stage?.destroy();
  stage = null;
});

watch(
  () => store.static,
  (s) => stage?.setStatic(s),
);
watch(
  () => [store.snapshot, store.selectedRobot] as const,
  ([s, sel]) => {
    if (s) {
      stage?.render(s, sel);
    }
  },
);
</script>

<template>
  <div
    ref="el"
    class="relative min-h-0 overflow-hidden bg-panel"
    :class="props.background ? 'pointer-events-none' : 'rounded-md border border-line'"
  >
    <div v-if="!ready" class="absolute inset-0 grid place-items-center text-dim">加载精灵…</div>
    <div
      v-else-if="!props.background"
      class="pointer-events-none absolute bottom-1.5 left-2 z-10 rounded-sm bg-bg/60 px-1.5 py-0.5 text-2xs text-dim"
    >
      拖拽平移 · 滚轮缩放 · 双击适配
    </div>
  </div>
</template>
