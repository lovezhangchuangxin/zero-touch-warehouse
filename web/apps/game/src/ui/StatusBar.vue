<script setup lang="ts">
import { computed } from "vue";
import { store } from "../store";

// 底部状态栏：持续读数（tick / 运行态 / 场景 / 代码加载）的唯一归宿，
// 顶栏只留操作（docs 讨论定稿：读数与操作分离）。
const tick = computed(() => store.snapshot?.tick ?? store.status?.tick ?? 0);
const running = computed(() => store.snapshot?.running ?? store.status?.running ?? false);
const faulted = computed(() => (store.snapshot?.fault ?? store.status?.fault_class) != null);
const tps = computed(() => store.snapshot?.tps ?? store.status?.tps ?? 0);
const stateText = computed(() =>
  faulted.value ? "故障暂停" : running.value ? `运行中 ${tps.value}/s` : "已暂停",
);
</script>

<template>
  <footer
    class="flex items-center gap-3 rounded-md border border-line bg-panel px-2.5 py-1 text-2xs text-dim"
  >
    <span class="font-mono">tick {{ tick }}</span>
    <span class="inline-flex items-center gap-1.5">
      <span
        class="inline-block h-1.5 w-1.5 rounded-full"
        :class="faulted ? 'bg-bad' : running ? 'bg-ok' : 'bg-dim'"
      />
      <span :class="faulted ? 'text-bad' : running ? 'text-ok' : ''">{{ stateText }}</span>
    </span>
    <span v-if="store.static?.name">{{ store.static.name }}</span>
    <span class="flex-1" />
    <span v-if="store.snapshot && !store.snapshot.loaded" class="text-warn">未加载代码</span>
  </footer>
</template>
