<script setup lang="ts">
// 画布占位：Pixi 8 渲染在下一提交接入（本组件持有 Pixi Application，
// 对象不进 Vue 响应式系统，docs/architecture/05）。
import { store } from "../store";
</script>

<template>
  <div class="canvas">
    <div v-if="store.snapshot" class="meta mono">
      {{ store.snapshot.map_w }}×{{ store.snapshot.map_h }} · tick {{ store.snapshot.tick }}
    </div>
    <div class="placeholder">Pixi 画布将在下一步接入</div>
  </div>
</template>

<style scoped>
.canvas {
  position: relative;
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 6px;
  display: grid;
  place-items: center;
  min-height: 0;
  overflow: hidden;
}
.placeholder {
  color: var(--dim);
}
.meta {
  position: absolute;
  top: 8px;
  left: 10px;
  color: var(--dim);
}
</style>
