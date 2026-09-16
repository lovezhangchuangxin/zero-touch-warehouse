import "./styles/main.css";
import { createApp } from "vue";
import App from "./App.vue";
import { installScrollGuards } from "./pageScroll";
import { bootstrap } from "./store";
import { loadSettings } from "./settings";

void (async () => {
  // 设置先行：布局 / 默认语言就位后再挂载，避免默认值闪变。
  await loadSettings();
  createApp(App).mount("#app");
  installScrollGuards();
  await bootstrap();
})();
