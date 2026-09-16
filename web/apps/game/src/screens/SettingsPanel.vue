<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";

// 设置面板：居中卡片，独立于主菜单 / 暂停浮层的层级（关闭即回到来处）。
// 首版只有真实可用的默认脚本语言；音量 / 存档为占位行，标明归属版本。
const emit = defineEmits<{ close: [] }>();

const LANG_KEY = "ztw.editor.lang";
function storedLang(): "js" | "py" {
  try {
    return localStorage.getItem(LANG_KEY) === "py" ? "py" : "js";
  } catch {
    return "js"; // 存储不可用：回默认
  }
}
const lang = ref<"js" | "py">(storedLang());
function setLang(v: string) {
  lang.value = v === "py" ? "py" : "js";
  try {
    localStorage.setItem(LANG_KEY, lang.value);
  } catch {
    // 存储不可用：仅本次会话生效
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape" && !e.defaultPrevented) {
    e.preventDefault();
    emit("close");
  }
}
onMounted(() => window.addEventListener("keydown", onKeydown));
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <div class="absolute inset-0 z-30" @click.self="emit('close')">
    <div
      class="absolute left-1/2 top-1/2 w-[24rem] max-w-[calc(100vw-4rem)] -translate-x-1/2 -translate-y-1/2 rounded-md border border-line bg-panel p-4 shadow-lg shadow-black/40"
    >
      <h2 class="text-lg font-bold tracking-wider text-fg">设置</h2>
      <div class="mt-4 flex flex-col gap-4">
        <label class="flex items-center justify-between gap-4">
          <span class="text-sm text-fg">默认脚本语言</span>
          <select
            class="field"
            :value="lang"
            @change="setLang(($event.target as HTMLSelectElement).value)"
          >
            <option value="js">JavaScript</option>
            <option value="py">Python</option>
          </select>
        </label>
        <p class="text-2xs text-dim">对新开的编辑器生效，已打开的会话不变。</p>
        <div class="flex items-center justify-between gap-4 opacity-40">
          <span class="text-sm text-fg">音量</span>
          <span class="text-xs text-dim">后续版本提供</span>
        </div>
        <div class="flex items-center justify-between gap-4 opacity-40">
          <span class="text-sm text-fg">存档管理</span>
          <span class="text-xs text-dim">随存档系统（原型 C）提供</span>
        </div>
      </div>
      <div class="mt-5 flex justify-end">
        <button class="btn" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>
