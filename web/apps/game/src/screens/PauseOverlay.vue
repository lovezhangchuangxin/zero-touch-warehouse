<script setup lang="ts">
// ESC 暂停浮层：与主菜单同一右书脊视觉。世界已在打开时暂停（父级处理），
// "继续"按打开时的速度恢复；"回主菜单"保持暂停——冷启动吸引模式由
// 主菜单自行接管，对局中回主菜单则背景保持玩家的暂停仓库。
defineProps<{ tps: number }>();
const emit = defineEmits<{ close: []; openSettings: []; openMenu: [] }>();
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
        <button class="menu-item" @click="emit('openSettings')"><span>设置</span></button>
        <button class="menu-item" @click="emit('openMenu')"><span>回主菜单</span></button>
      </nav>
      <footer class="absolute bottom-5 text-2xs text-dim/70">ESC 继续</footer>
    </div>
  </div>
</template>
