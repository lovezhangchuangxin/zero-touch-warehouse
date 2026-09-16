<script setup lang="ts">
import SettingsPanel from "./screens/SettingsPanel.vue";
import GameScreen from "./screens/GameScreen.vue";
import MainMenu from "./screens/MainMenu.vue";
import * as api from "./api";
import { store } from "./store";
import { ref } from "vue";

// 应用宿主：游戏屏常驻，主菜单 / 设置是对局世界之上的浮层。
// 冷启动主菜单跑吸引模式（演示脚本慢速空转，见 MainMenu）；"开始"
// 整包重开所选场景并丢弃演示程序，落进未加载代码的干净初态；
// 对局中"回主菜单"保持暂停，渐晕之下就是玩家自己的仓库。
const menuOpen = ref(true);
const settingsOpen = ref(false);

function onStart(id: string) {
  menuOpen.value = false;
  store.inGame = true;
  void api.resetScenario(id, false);
}
</script>

<template>
  <div class="relative h-full overflow-hidden">
    <GameScreen
      :overlay-open="menuOpen || settingsOpen"
      :menu-open="menuOpen"
      @open-menu="menuOpen = true"
      @open-settings="settingsOpen = true"
    />
    <Transition name="menu-fade">
      <MainMenu v-if="menuOpen" @start="onStart" @open-settings="settingsOpen = true" />
    </Transition>
    <SettingsPanel v-if="settingsOpen" @close="settingsOpen = false" />
  </div>
</template>
