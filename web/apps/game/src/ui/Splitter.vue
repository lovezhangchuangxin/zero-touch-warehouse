<script setup lang="ts">
// 可拖分隔条：指针捕获后按移动轴持续上报位移增量（px），尺寸钳制由
// 父级负责，落盘在拖拽结束（end）时进行。双击恢复默认尺寸（reset）；
// 方向键微调（Shift 加速）。lostpointercapture 兜底防止捕获丢失后
// dragging 卡死。
import { onBeforeUnmount } from "vue";
import { suppressPageSelect } from "../pageSelect";

const props = defineProps<{ dir: "x" | "y" }>();
const emit = defineEmits<{ drag: [number]; end: []; reset: [] }>();

let last = 0;
let dragging = false;
let releaseSelect: (() => void) | null = null;

function onDown(e: PointerEvent) {
  if (e.button !== 0 || !e.isPrimary || dragging) return;
  // preventDefault 掐断兼容 mousedown，原生拖拽选择无从发起；它同时
  // 取消点击聚焦的默认动作，手动补上以保留方向键微调。双击复位不受
  // 影响——click 系事件独立于兼容鼠标事件派发。
  e.preventDefault();
  (e.currentTarget as HTMLElement).focus({ preventScroll: true });
  last = props.dir === "x" ? e.clientX : e.clientY;
  try {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  } catch {
    // 捕获失败（极少）：退化为无捕获拖拽，lostpointercapture 不会再
    // 派发，挂窗口级 once up 兜底复位（endDrag 幂等，不与 onUp 重复）。
    window.addEventListener("pointerup", endDrag, { once: true });
  }
  dragging = true;
  releaseSelect = suppressPageSelect();
}
function onMove(e: PointerEvent) {
  // buttons 校验：捕获丢失且 up 未送达时，悬停不再继续拖。
  if (!dragging || !e.isPrimary || !(e.buttons & 1)) return;
  const cur = props.dir === "x" ? e.clientX : e.clientY;
  emit("drag", cur - last);
  last = cur;
}
function onUp(e: PointerEvent) {
  if (!dragging) return;
  const el = e.currentTarget as HTMLElement;
  if (el.hasPointerCapture(e.pointerId)) {
    el.releasePointerCapture(e.pointerId);
  }
  endDrag();
}
function onLostCapture() {
  // 正常路径 up 已把 dragging 复位，这里只兜底捕获被系统夺走的情形。
  if (!dragging) return;
  endDrag();
}
function endDrag(): void {
  if (!dragging) return;
  dragging = false;
  releaseSelect?.();
  releaseSelect = null;
  emit("end");
}
// 拖拽中卸载（HMR 重载等）：复位全局禁选，文档级拦截器不残留。
onBeforeUnmount(() => releaseSelect?.());
function onKeydown(e: KeyboardEvent) {
  const step = e.shiftKey ? 48 : 16;
  let d = 0;
  if (props.dir === "x") {
    if (e.key === "ArrowLeft") d = -step;
    else if (e.key === "ArrowRight") d = step;
  } else if (e.key === "ArrowUp") {
    d = -step;
  } else if (e.key === "ArrowDown") {
    d = step;
  }
  if (d !== 0) {
    e.preventDefault();
    emit("drag", d);
    emit("end");
  }
}
</script>

<template>
  <!-- 6px 命中区，中间 1px 可见缝线，悬停 / 拖拽时亮起 -->
  <div
    role="separator"
    :aria-orientation="dir === 'x' ? 'vertical' : 'horizontal'"
    aria-label="拖拽调整分栏大小"
    tabindex="0"
    class="group relative shrink-0 touch-none select-none bg-bg transition-colors hover:bg-line/60 focus-visible:bg-accent/40"
    :class="dir === 'x' ? 'w-1.5 cursor-col-resize' : 'h-1.5 cursor-row-resize'"
    @pointerdown="onDown"
    @pointermove="onMove"
    @pointerup="onUp"
    @pointercancel="onUp"
    @lostpointercapture="onLostCapture"
    @dblclick="emit('reset')"
    @keydown="onKeydown"
  >
    <div
      class="absolute m-auto bg-line transition-colors group-hover:bg-accent group-active:bg-accent"
      :class="
        dir === 'x'
          ? 'inset-y-0 left-1/2 h-10 w-px -translate-x-1/2'
          : 'inset-x-0 top-1/2 h-px w-10 -translate-y-1/2'
      "
    />
  </div>
</template>
