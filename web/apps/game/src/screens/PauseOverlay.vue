<script setup lang="ts">
import { ref } from "vue";
import SaveSlots from "../ui/SaveSlots.vue";
import type { SaveSummary } from "../types";

// ESC 暂停浮层：与主菜单同一右书脊视觉。世界已在打开时暂停（父级处理），
// "继续"按打开时的速度恢复；"回主菜单"保持暂停——冷启动吸引模式由
// 主菜单自行接管，对局中回主菜单则背景保持玩家的暂停仓库。
// 保存进度：暂停点即安全点（docs/architecture/06），手动档可选名字。
defineProps<{
  tps: number;
  saves: SaveSummary[];
  scenarioNames?: Record<string, string>;
  note: string;
}>();
const emit = defineEmits<{
  close: [];
  openSettings: [];
  openMenu: [];
  save: [name: string | null];
  load: [id: string];
}>();

const nameInput = ref("");
const loadOpen = ref(false);

function onSave() {
  const name = nameInput.value.trim();
  emit("save", name || null);
  nameInput.value = "";
}
</script>

<template>
  <div class="absolute inset-0 z-20">
    <div class="absolute inset-0 bg-bg/70" @click="emit('close')" />
    <div class="absolute inset-0 flex flex-col items-center justify-center pb-14">
      <header class="text-center">
        <h2 class="text-3xl font-bold tracking-widest text-fg">已暂停</h2>
        <p class="mt-1 text-sm text-dim">世界停在你离开的那一刻</p>
      </header>
      <nav class="mt-10 flex w-72 flex-col items-stretch gap-1.5">
        <button class="menu-item" @click="emit('close')">
          <span>继续</span>
          <span class="menu-hint font-mono text-2xs">{{ tps }}× </span>
        </button>
        <!-- 保存进度：安全点整档写盘（世界 + memory + 程序 + 草稿） -->
        <div class="flex flex-col gap-1.5">
          <div class="flex gap-1.5">
            <input
              v-model="nameInput"
              class="min-w-0 flex-1 rounded-sm border border-line bg-bg px-2 py-1.5 text-sm text-fg placeholder:text-dim focus:border-accent focus:outline-none"
              placeholder="存档名（可空）"
              maxlength="32"
              @keydown.enter.prevent="onSave"
            />
            <button class="btn shrink-0" @click="onSave">保存进度</button>
          </div>
          <p v-if="note" class="min-h-[1.25rem] px-1 text-2xs text-dim">{{ note }}</p>
        </div>
        <button class="menu-item" :class="loadOpen && 'menu-item-on'" @click="loadOpen = !loadOpen">
          <span>读取存档</span>
          <span v-if="loadOpen" class="menu-hint">收起</span>
        </button>
        <div v-if="loadOpen" class="w-[22rem] max-w-[70vw] pb-2">
          <SaveSlots
            :saves="saves"
            :scenario-names="scenarioNames"
            empty-hint="暂无存档"
            @load="(id) => emit('load', id)"
          />
        </div>
        <button class="menu-item" @click="emit('openSettings')"><span>设置</span></button>
        <button class="menu-item" @click="emit('openMenu')"><span>回主菜单</span></button>
      </nav>
      <footer class="absolute bottom-5 text-2xs text-dim/70">ESC 继续</footer>
    </div>
  </div>
</template>
