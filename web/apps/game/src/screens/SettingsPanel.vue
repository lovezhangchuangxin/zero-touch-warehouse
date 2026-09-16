<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { LANG_OPTIONS } from "../scripts";
import { settings, updateSettings } from "../settings";
import SaveSlots from "../ui/SaveSlots.vue";
import Select from "../ui/Select.vue";
import * as api from "../api";
import { store } from "../store";
import type { SaveSummary } from "../types";

// 设置面板：居中卡片，独立于主菜单 / 暂停浮层的层级（关闭即回到来处）。
// 本机设置经 settings.json 原子落盘（C1 迁移，取代 localStorage）；
// 存档管理在此删除任意档（自动档轮换保留 3 份）。
const emit = defineEmits<{ close: [] }>();

const lang = ref(settings.lang);
function setLang(v: string) {
  lang.value = v === "py" ? "py" : "js";
  void updateSettings({ lang: lang.value });
}

// 存档管理列表
const saves = ref<SaveSummary[]>([]);
const manageNote = ref("");
const scenarioNames = computed<Record<string, string>>(() =>
  Object.fromEntries((store.static?.scenarios ?? []).map((s) => [s.id, s.name])),
);
async function refreshSaves() {
  try {
    saves.value = await api.listSaves();
  } catch {
    saves.value = [];
  }
}
async function onDelete(id: string) {
  try {
    await api.deleteSave(id);
    manageNote.value = "已删除";
  } catch (e) {
    manageNote.value = `删除失败：${String(e)}`;
  }
  await refreshSaves();
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape" && !e.defaultPrevented) {
    e.preventDefault();
    emit("close");
  }
}
onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  void refreshSaves();
});
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <div class="absolute inset-0 z-30" @click.self="emit('close')">
    <div
      class="absolute left-1/2 top-1/2 flex max-h-[80vh] w-[24rem] max-w-[calc(100vw-4rem)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-md border border-line bg-panel p-4 shadow-lg shadow-black/40"
    >
      <h2 class="text-lg font-bold tracking-wider text-fg">设置</h2>
      <div class="mt-4 flex flex-col gap-4 overflow-y-auto pr-1">
        <label class="flex items-center justify-between gap-4">
          <span class="text-sm text-fg">默认脚本语言</span>
          <Select
            class="max-w-40"
            :model-value="lang"
            :options="LANG_OPTIONS"
            aria-label="默认脚本语言"
            @update:model-value="setLang"
          />
        </label>
        <p class="text-2xs text-dim">对新开的编辑器生效，已打开的会话不变。</p>
        <div class="flex items-center justify-between gap-4 opacity-40">
          <span class="text-sm text-fg">音量</span>
          <span class="text-xs text-dim">后续版本提供</span>
        </div>
        <div class="flex flex-col gap-2">
          <div class="flex items-center justify-between">
            <span class="text-sm text-fg">存档管理</span>
            <span v-if="manageNote" class="text-2xs text-dim">{{ manageNote }}</span>
          </div>
          <SaveSlots
            :saves="saves"
            :scenario-names="scenarioNames"
            deletable
            empty-hint="暂无存档"
            @delete="onDelete"
          />
          <p class="text-2xs text-dim">自动档轮换保留 3 份；损坏档灰条列出、不可载入。</p>
        </div>
      </div>
      <div class="mt-5 flex justify-end">
        <button class="btn" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>
