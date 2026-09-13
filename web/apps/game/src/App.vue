<script setup lang="ts">
import CodeEditor from "./ui/CodeEditor.vue";
import DebugBar from "./ui/DebugBar.vue";
import EventPanel from "./ui/EventPanel.vue";
import FaultBanner from "./ui/FaultBanner.vue";
import GameCanvas from "./ui/GameCanvas.vue";
import LogPanel from "./ui/LogPanel.vue";
import MarketPanel from "./ui/MarketPanel.vue";
import RobotPanel from "./ui/RobotPanel.vue";
import { store } from "./store";
import { computed, ref } from "vue";

const tab = ref<"events" | "logs">("events");
const fault = computed(() => store.snapshot?.fault ?? null);
</script>

<template>
  <div class="app">
    <DebugBar />
    <main class="main">
      <section class="stage">
        <GameCanvas />
        <FaultBanner v-if="fault" :fault="fault" />
      </section>
      <aside class="side">
        <RobotPanel />
        <MarketPanel />
      </aside>
    </main>
    <section class="bottom">
      <CodeEditor class="editor" />
      <div class="reports">
        <div class="tabs">
          <button :class="{ on: tab === 'events' }" @click="tab = 'events'">事件</button>
          <button :class="{ on: tab === 'logs' }" @click="tab = 'logs'">日志</button>
        </div>
        <EventPanel v-show="tab === 'events'" class="panel" />
        <LogPanel v-show="tab === 'logs'" class="panel" />
      </div>
    </section>
  </div>
</template>

<style>
:root {
  --bg: #14161a;
  --panel: #1d2026;
  --panel-2: #242830;
  --line: #323743;
  --fg: #d7dbe2;
  --dim: #8b93a1;
  --ok: #56c271;
  --bad: #e2606b;
  --warn: #e0b34c;
  --accent: #4da3e0;
}
* {
  box-sizing: border-box;
}
html,
body,
#app {
  height: 100%;
  margin: 0;
}
body {
  background: var(--bg);
  color: var(--fg);
  font:
    13px/1.45 "PingFang SC",
    "Microsoft YaHei",
    system-ui,
    sans-serif;
}
button,
select,
textarea {
  font: inherit;
  color: inherit;
}
button {
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 4px;
  padding: 3px 10px;
  cursor: pointer;
}
button:hover {
  border-color: var(--accent);
}
button.on {
  background: #2c3a4e;
  border-color: var(--accent);
}
code,
.mono {
  font-family: "SF Mono", ui-monospace, Menlo, Consolas, monospace;
  font-size: 12px;
}
.app {
  height: 100%;
  display: grid;
  grid-template-rows: auto 1fr 300px;
  gap: 8px;
  padding: 8px;
}
.main {
  display: grid;
  grid-template-columns: 1fr 320px;
  gap: 8px;
  min-height: 0;
}
.stage {
  position: relative;
  display: grid;
  place-items: stretch;
  min-height: 0;
}
.side {
  display: grid;
  grid-template-rows: minmax(0, 1.2fr) minmax(0, 1fr);
  gap: 8px;
  min-height: 0;
}
.bottom {
  display: grid;
  grid-template-columns: minmax(360px, 2fr) minmax(360px, 3fr);
  gap: 8px;
  min-height: 0;
}
.reports {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 6px;
  min-height: 0;
}
.tabs {
  display: flex;
  gap: 6px;
  padding: 6px 8px 0;
}
.panel {
  min-height: 0;
}
</style>
