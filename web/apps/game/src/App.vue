<script setup lang="ts">
import SettingsPanel from "./screens/SettingsPanel.vue";
import GameScreen from "./screens/GameScreen.vue";
import MainMenu from "./screens/MainMenu.vue";
import * as api from "./api";
import { store, applyDrafts } from "./store";
import { onBeforeUnmount, onMounted, ref, watch } from "vue";

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

/** 读档（主菜单继续 / 读档入口）：草稿段回填编辑器（存档内草稿优先）。 */
async function loadSave(id: string) {
  menuOpen.value = false;
  store.inGame = true;
  try {
    const drafts = await api.loadGame(id);
    if (drafts) applyDrafts(drafts);
  } catch (e) {
    console.warn("读档失败", e);
    // 失败整体回滚到主菜单语义：世界未被破坏（临时会话初始化失败不换
    // 入），inGame 一并回退，避免主菜单因 inGame 卡真而不再跑吸引模式。
    store.inGame = false;
    menuOpen.value = true;
  }
}

// ---------------------------------------------------------------------------
// 自动存档（docs/architecture/06：定时 + 返回主菜单，同一安全点与写入
// 路径；草稿段取编辑器去抖刷新的整包缓存）。吸引模式（未进对局）不写
// 自动档——主菜单"继续"只指向真实对局。
// ---------------------------------------------------------------------------

const AUTOSAVE_MS = 5 * 60_000;
let autosaveTimer = 0;

/** 定时路径：对局中且无浮层才写自动档（menuOpen 门只服务定时器——
 *  回主菜单的即席存档经 saveNow 直达，否则 watch 触发时门必挡）。 */
function autoSave() {
  if (!store.inGame || menuOpen.value || settingsOpen.value) return;
  if (!store.snapshot?.loaded) return;
  saveNow();
}

function saveNow() {
  void api.saveGame(null, store.draftsCache).catch((e) => {
    console.warn("自动存档失败", e);
  });
}

onMounted(() => {
  autosaveTimer = window.setInterval(autoSave, AUTOSAVE_MS);
});
onBeforeUnmount(() => clearInterval(autosaveTimer));
// 回主菜单（暂停中）顺手存一次：最自然的"离开即保存"点。
watch(menuOpen, (open) => {
  if (open && store.inGame && store.snapshot?.loaded) saveNow();
});
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
      <MainMenu
        v-if="menuOpen"
        @start="onStart"
        @open-settings="settingsOpen = true"
        @resume-save="loadSave"
        @load-save="loadSave"
      />
    </Transition>
    <SettingsPanel v-if="settingsOpen" @close="settingsOpen = false" />
  </div>
</template>
