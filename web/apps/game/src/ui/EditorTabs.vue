<script setup lang="ts">
import type { EditorFile } from "../editor/files";

// 多文件 tab 条（预留）：宿主协议仍是单字符串，当前恒为 1 个主文件，
// 仅在 files.length > 1 时由 CodeEditor 渲染。切换语义：
// view.setState()（撤销历史随 state），见 CodeEditor.vue。
defineProps<{
  files: EditorFile[];
  activeId: string;
}>();

defineEmits<{
  select: [id: string];
}>();
</script>

<template>
  <div class="flex gap-1 px-2 pt-1.5" role="tablist">
    <button
      v-for="f in files"
      :key="f.id"
      class="btn rounded-b-none"
      role="tab"
      :aria-selected="f.id === activeId"
      :class="{ 'btn-on': f.id === activeId }"
      @click="$emit('select', f.id)"
    >
      {{ f.name }}
    </button>
  </div>
</template>
