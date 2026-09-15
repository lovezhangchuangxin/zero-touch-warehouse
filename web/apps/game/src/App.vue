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
  <div class="grid h-full grid-rows-[auto_1fr_300px] gap-2 p-2">
    <DebugBar />
    <main class="grid min-h-0 grid-cols-[1fr_320px] gap-2">
      <section class="relative grid min-h-0">
        <GameCanvas />
        <FaultBanner v-if="fault" :fault="fault" />
      </section>
      <aside class="grid min-h-0 grid-rows-[minmax(0,1.2fr)_minmax(0,1fr)] gap-2">
        <RobotPanel />
        <MarketPanel />
      </aside>
    </main>
    <section class="grid min-h-0 grid-cols-[minmax(360px,2fr)_minmax(360px,3fr)] gap-2">
      <CodeEditor />
      <div
        class="grid min-h-0 grid-rows-[auto_minmax(0,1fr)] rounded-md border border-line bg-panel"
      >
        <div class="flex gap-1.5 px-2 pt-1.5">
          <button class="btn" :class="{ 'btn-on': tab === 'events' }" @click="tab = 'events'">
            事件
          </button>
          <button class="btn" :class="{ 'btn-on': tab === 'logs' }" @click="tab = 'logs'">
            日志
          </button>
        </div>
        <EventPanel v-show="tab === 'events'" class="min-h-0" />
        <LogPanel v-show="tab === 'logs'" class="min-h-0" />
      </div>
    </section>
  </div>
</template>
