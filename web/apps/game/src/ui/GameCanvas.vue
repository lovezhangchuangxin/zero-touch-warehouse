<script setup lang="ts">
// 画布挂载：Vue 侧只做桥接（挂载/卸载与快照喂入），Pixi 对象与渲染
// 节奏全部在 render/Stage 内，不进响应式系统（docs/architecture/05）。
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Stage } from "../render/Stage";
import { store } from "../store";

const el = ref<HTMLDivElement>();
const ready = ref(false);
let stage: Stage | null = null;

onMounted(async () => {
  if (!el.value) {
    return;
  }
  stage = await Stage.create(el.value);
  stage.setStatic(store.static);
  if (store.snapshot) {
    stage.render(store.snapshot, store.selectedRobot);
  }
  ready.value = true;
});

onBeforeUnmount(() => {
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
  <div ref="el" class="relative min-h-0 overflow-hidden rounded-md border border-line bg-panel">
    <div v-if="!ready" class="absolute inset-0 grid place-items-center text-dim">加载精灵…</div>
    <div
      v-else
      class="pointer-events-none absolute bottom-1.5 left-2 z-10 rounded-sm bg-bg/60 px-1.5 py-0.5 text-2xs text-dim"
    >
      拖拽平移 · 滚轮缩放 · 双击适配
    </div>
  </div>
</template>
