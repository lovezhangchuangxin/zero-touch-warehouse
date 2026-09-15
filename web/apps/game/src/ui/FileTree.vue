<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import { isMain, isValidFileName, type EditorFile } from "../editor/files";
import { vFade } from "./fade";

// 目录树（平铺）：入口文件固定置顶不可删，其余文件可新建 / 删除 /
// 双击重命名。删除走两段确认（首次点击进入待确认态，2.5 秒内再点执行；
// 点任意行取消待确认态）。
const props = defineProps<{ files: EditorFile[]; activeId: string }>();
const emit = defineEmits<{
  select: [string];
  create: [];
  delete: [string];
  rename: [string, string];
}>();

const renamingId = ref<string | null>(null);
const renameText = ref("");
const armedDeleteId = ref<string | null>(null);
let armedTimer: number | undefined;

// 只在挂载时聚焦：函数式 ref 每次重渲染都会重触发，会抢回焦点。
const vFocus = { mounted: (el: HTMLInputElement) => el.focus() };

function startRename(f: EditorFile) {
  if (isMain(f)) return;
  renamingId.value = f.id;
  renameText.value = f.name;
}
function commitRename() {
  const id = renamingId.value;
  renamingId.value = null;
  if (!id) return;
  const name = renameText.value.trim();
  const file = props.files.find((f) => f.id === id);
  // 非法名 / 与他人重名 = 取消（保持原名），不静默改成第三种结果。
  if (
    file &&
    name !== file.name &&
    isValidFileName(name, file.language) &&
    !props.files.some((f) => f.name === name)
  ) {
    emit("rename", id, name);
  }
}
function armDelete(id: string) {
  if (armedDeleteId.value === id) {
    cancelArm();
    emit("delete", id);
    return;
  }
  armedDeleteId.value = id;
  clearTimeout(armedTimer);
  armedTimer = window.setTimeout(() => (armedDeleteId.value = null), 2500);
}
function cancelArm() {
  clearTimeout(armedTimer);
  armedDeleteId.value = null;
}
function onRowSelect(id: string) {
  cancelArm();
  emit("select", id);
}
onBeforeUnmount(cancelArm);
</script>

<template>
  <div class="flex min-h-0 w-36 shrink-0 flex-col border-r border-line bg-panel-2/40">
    <div class="flex items-center gap-1 px-1.5 pb-1 pt-1.5">
      <span class="panel-h flex-1">{{ "文件" }}</span>
      <button class="btn px-1.5 py-0 text-xs" title="新建文件" @click="emit('create')">＋</button>
    </div>
    <div v-fade class="min-h-0 flex-1 overflow-auto pb-1.5 text-xs">
      <div
        v-for="f in files"
        :key="f.id"
        class="group flex cursor-pointer items-center gap-1 px-1.5 py-0.5"
        :class="f.id === activeId ? 'bg-selected text-fg' : 'text-dim hover:bg-panel-2'"
        @click="onRowSelect(f.id)"
      >
        <template v-if="renamingId === f.id">
          <input
            v-focus
            v-model="renameText"
            class="field w-full font-mono text-xs"
            @keydown.enter.prevent="commitRename"
            @keydown.escape.prevent="renamingId = null"
            @blur="commitRename"
            @click.stop
          />
        </template>
        <template v-else>
          <span
            class="truncate font-mono"
            :class="isMain(f) ? 'text-fg' : ''"
            :title="isMain(f) ? '入口文件（不可删除 / 改名）' : '双击重命名'"
            @dblclick="startRename(f)"
          >
            {{ f.name }}
          </span>
          <template v-if="isMain(f)">
            <span
              class="ml-auto shrink-0 rounded-sm bg-panel-2 px-1 text-2xs leading-4 text-dim"
              title="入口文件（不可删除 / 改名）"
            >
              入口
            </span>
          </template>
          <button
            v-else
            class="ml-auto shrink-0 px-0.5 text-2xs leading-4"
            :class="
              armedDeleteId === f.id
                ? 'font-semibold text-bad'
                : 'text-dim opacity-0 group-hover:opacity-100'
            "
            :title="armedDeleteId === f.id ? '再点一次确认删除' : '删除'"
            @click.stop="armDelete(f.id)"
          >
            {{ armedDeleteId === f.id ? "确认？" : "✕" }}
          </button>
        </template>
      </div>
    </div>
  </div>
</template>
