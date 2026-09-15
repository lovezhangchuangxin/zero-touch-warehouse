<script setup lang="ts">
import { computed } from "vue";
import { codeLabel, isOkCode } from "../codes";
import { fmtPos } from "../format";
import { vFade } from "./fade";
import { store } from "../store";

const robots = computed(() => store.snapshot?.robots ?? []);
function select(id: number) {
  store.selectedRobot = store.selectedRobot === id ? null : id;
}
</script>

<template>
  <!-- 抽屉页签内容：外框由抽屉提供，这里只负责头部与滚动列表 -->
  <div class="flex min-h-0 flex-col">
    <h3 class="panel-h px-2.5 pt-2">
      {{ "机器人 " }}<span class="font-normal">{{ robots.length }} 台</span>
    </h3>
    <!-- v-fade 挂外层 block：内层 flex 列表不与 sticky 哨兵互相干扰 -->
    <div v-fade class="min-h-0 flex-1 overflow-auto px-2.5 pb-2 pt-1.5">
      <div class="flex flex-col gap-1.5">
        <div
          v-for="r in robots"
          :key="r.id"
          class="cursor-pointer rounded-sm border bg-panel-2 px-2 py-1.5"
          :class="
            store.selectedRobot === r.id ? 'border-accent' : 'border-line hover:border-accent/60'
          "
          @click="select(r.id)"
        >
          <div class="flex items-baseline gap-2.5">
            <span class="font-semibold">#{{ r.id }}</span>
            <span class="font-mono text-xs text-dim">{{ fmtPos(r.x, r.y) }}</span>
            <span v-if="r.carry" class="text-accent">携带 {{ r.carry.goods_type }}</span>
            <span v-else class="text-dim">空载</span>
          </div>
          <div class="mt-0.5 flex items-center gap-2">
            <div class="h-1.5 flex-1 overflow-hidden rounded-full bg-black/30">
              <div
                class="h-full min-w-0.5 rounded-full"
                :class="r.energy / r.energy_max < 0.3 ? 'bg-bad' : 'bg-ok'"
                :style="{ width: `${(100 * r.energy) / r.energy_max}%` }"
              />
            </div>
            <span class="font-mono text-2xs text-dim">{{ r.energy }}/{{ r.energy_max }}</span>
          </div>
          <div
            v-if="r.last_result"
            class="mt-0.5 text-xs"
            :class="isOkCode(r.last_result.code) ? 'text-ok' : 'text-bad'"
          >
            <code>{{ r.last_result.action }}</code>
            <code class="text-dim">{{ r.last_result.arg }}</code>
            →
            <code>{{ r.last_result.code }}</code>
            <span class="text-dim">{{ codeLabel(r.last_result.code) }}</span>
          </div>
          <div v-else class="mt-0.5 text-xs text-dim">上 tick 未受理动作</div>
        </div>
      </div>
      <div v-if="robots.length === 0" class="px-1 py-2 text-xs text-dim">（无机器人）</div>
    </div>
  </div>
</template>
