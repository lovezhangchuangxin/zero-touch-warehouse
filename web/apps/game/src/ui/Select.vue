<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, useId, watch } from "vue";

// 自绘下拉，替换原生 <select>：原生控件只有合上的部分能吃到主题，
// 弹出列表由系统渲染（WKWebView / WebView2 下是原生菜单），与游戏视觉
// 完全脱节。触发器留在文档流原位（class 透传控制宽度，$attrs 手动落到
// 按钮上——Teleport 使组件多根，继承必须关掉），弹层 Teleport 到 body
// 用 fixed 定位：编辑器列外层 overflow-hidden，原地绝对定位会被裁剪。
interface SelectOption {
  value: string;
  label: string;
}

const props = defineProps<{
  modelValue: string;
  options: SelectOption[];
  /** 无匹配项时触发器显示的占位文案（dim 色） */
  placeholder?: string;
}>();

const emit = defineEmits<{ "update:modelValue": [value: string] }>();
defineOptions({ inheritAttrs: false });

const rootEl = ref<HTMLElement | null>(null);
const popEl = ref<HTMLElement | null>(null);
const open = ref(false);
// 键盘高亮与悬停共用一个索引；-1 = 未选中（占位态打开时）
const hi = ref(-1);
const popId = useId();

const current = computed(() => props.options.find((o) => o.value === props.modelValue));
const display = computed(() => current.value?.label ?? props.placeholder ?? "");

// 弹层几何：打开瞬间按触发器盒测一次。下方放不下且上方更宽裕则向上翻。
const ITEM_H = 28; // py-1 + 13px×1.45 行高的近似
const GAP = 4;
const MAX_POP_H = 256;
const pos = ref({ left: 0, top: 0, bottom: 0, minWidth: 0, up: false });

function place() {
  const el = rootEl.value;
  if (!el) return;
  const r = el.getBoundingClientRect();
  const need = Math.min(props.options.length * ITEM_H + GAP * 2, MAX_POP_H);
  const below = window.innerHeight - r.bottom;
  pos.value = {
    left: Math.min(Math.max(8, r.left), window.innerWidth - r.width - 8),
    top: r.bottom + GAP,
    bottom: window.innerHeight - r.top + GAP,
    minWidth: r.width,
    up: below < need + GAP && r.top > below,
  };
}
const popStyle = computed(() => {
  const style: Record<string, string> = {
    left: `${pos.value.left}px`,
    minWidth: `${pos.value.minWidth}px`,
    maxWidth: "min(20rem, calc(100vw - 1rem))",
    maxHeight: `${MAX_POP_H}px`,
    transformOrigin: pos.value.up ? "bottom" : "top",
  };
  if (pos.value.up) style.bottom = `${pos.value.bottom}px`;
  else style.top = `${pos.value.top}px`;
  return style;
});

function openList() {
  if (open.value || props.options.length === 0) return;
  place();
  hi.value = props.options.findIndex((o) => o.value === props.modelValue);
  open.value = true;
  // 挂载后补两件事：弹层实际宽度可能大于触发器（长标签撑到 maxWidth 上限，
  // place() 只能按触发器盒钳制），按实测宽收一次右缘；长列表把选中项
  // 滚进可视区（hi 未变化时下面的 watch 不触发）。
  void nextTick(() => {
    const pop = popEl.value;
    if (!pop) return;
    const overflow = pos.value.left + pop.offsetWidth + 8 - window.innerWidth;
    if (overflow > 0) pos.value.left -= overflow;
    pop.children[Math.max(hi.value, 0)]?.scrollIntoView({ block: "nearest" });
  });
}
function close() {
  open.value = false;
  hi.value = -1;
}
function pick(o: SelectOption) {
  close();
  if (o.value !== props.modelValue) emit("update:modelValue", o.value);
}

// 键盘全程留在触发器上（弹层容器 mousedown.prevent 防焦点转移），
// 因此 combobox 的按键处理都挂在这里；Enter 必须拦默认——原生按钮的
// Enter 激活会在 keydown 后补一发 click，把刚打开的弹层立刻关上。
function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape" && open.value) {
    e.preventDefault(); // 上层（暂停 / 设置）的 ESC 都让位于 defaultPrevented
    close();
    return;
  }
  if (!open.value) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openList();
    }
    return;
  }
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const len = props.options.length;
    const d = e.key === "ArrowDown" ? 1 : -1;
    hi.value = (hi.value + d + len) % len;
  } else if (e.key === "Enter") {
    e.preventDefault();
    const o = props.options[hi.value];
    if (o) pick(o);
  } else if (e.key === "Tab") {
    close(); // 放行走焦点，只收弹层
  }
}

// 高亮跟随视点，越出弹层可视区时最小滚动归位
watch(hi, () => {
  popEl.value?.children[hi.value]?.scrollIntoView({ block: "nearest" });
});

function onPointerDown(e: PointerEvent) {
  if (!open.value) return;
  const t = e.target as Node | null;
  if (rootEl.value?.contains(t) || popEl.value?.contains(t)) return;
  close();
}
function onWindowBlur() {
  close();
}
// 窗口尺寸变化会让打开瞬间测得的几何失效，直接收起最稳妥
function onResize() {
  close();
}
onMounted(() => {
  window.addEventListener("pointerdown", onPointerDown);
  window.addEventListener("resize", onResize);
  window.addEventListener("blur", onWindowBlur);
});
onBeforeUnmount(() => {
  window.removeEventListener("pointerdown", onPointerDown);
  window.removeEventListener("resize", onResize);
  window.removeEventListener("blur", onWindowBlur);
});
</script>

<template>
  <button
    v-bind="$attrs"
    ref="rootEl"
    type="button"
    role="combobox"
    class="inline-flex min-w-0 cursor-pointer items-center gap-1.5 rounded-sm border border-line bg-panel-2 px-2 py-1 text-left transition-colors hover:border-accent"
    :class="open && 'border-accent'"
    :aria-expanded="open"
    aria-haspopup="listbox"
    :aria-controls="open ? popId : undefined"
    :aria-activedescendant="open && hi >= 0 ? `${popId}-${hi}` : undefined"
    @keydown="onKeydown"
    @click="open ? close() : openList()"
  >
    <span class="min-w-0 flex-1 truncate" :class="current ? 'text-fg' : 'text-dim'">{{
      display
    }}</span>
    <svg
      viewBox="0 0 12 12"
      class="size-3 shrink-0 text-dim transition-transform"
      :class="open && 'rotate-180'"
      aria-hidden="true"
    >
      <path
        d="M2.5 4.5 6 8l3.5-3.5"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  </button>
  <Teleport to="body">
    <Transition name="sel-pop">
      <div
        v-if="open"
        ref="popEl"
        :id="popId"
        role="listbox"
        class="fixed z-50 overflow-y-auto rounded-sm border border-line bg-panel-2 py-1 shadow-lg shadow-black/50"
        :style="popStyle"
        @mousedown.prevent
      >
        <button
          v-for="(o, i) in options"
          :id="`${popId}-${i}`"
          :key="o.value"
          type="button"
          role="option"
          class="flex w-full cursor-pointer items-center gap-1.5 px-2 py-1 text-left text-fg"
          :class="i === hi && 'bg-selected'"
          :aria-selected="o.value === modelValue"
          @pointerenter="hi = i"
          @click="pick(o)"
        >
          <span class="min-w-0 flex-1 truncate">{{ o.label }}</span>
          <svg
            v-if="o.value === modelValue"
            viewBox="0 0 12 12"
            class="size-3 shrink-0 text-accent"
            aria-hidden="true"
          >
            <path
              d="M2 6.5 4.8 9 10 3.5"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </button>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.sel-pop-enter-active,
.sel-pop-leave-active {
  transition:
    opacity 0.12s ease,
    transform 0.12s ease;
}
.sel-pop-enter-from,
.sel-pop-leave-to {
  opacity: 0;
  transform: scaleY(0.97);
}
</style>
