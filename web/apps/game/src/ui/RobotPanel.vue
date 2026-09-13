<script setup lang="ts">
import { computed } from "vue";
import { codeLabel, isOkCode } from "../codes";
import { fmtPos } from "../format";
import { store } from "../store";

const robots = computed(() => store.snapshot?.robots ?? []);
function select(id: number) {
  store.selectedRobot = store.selectedRobot === id ? null : id;
}
</script>

<template>
  <section class="panel">
    <h3>机器人 <span class="dim">{{ robots.length }} 台</span></h3>
    <div class="list">
      <div
        v-for="r in robots"
        :key="r.id"
        class="robot"
        :class="{ sel: store.selectedRobot === r.id }"
        @click="select(r.id)"
      >
        <div class="row1">
          <span class="id">#{{ r.id }}</span>
          <span class="mono dim">{{ fmtPos(r.x, r.y) }}</span>
          <span v-if="r.carry" class="carry">携带 {{ r.carry.goods_type }}</span>
          <span v-else class="dim">空载</span>
        </div>
        <div class="energy">
          <div
            class="fill"
            :class="{ low: r.energy / r.energy_max < 0.3 }"
            :style="{ width: `${(100 * r.energy) / r.energy_max}%` }"
          />
          <span class="mono">{{ r.energy }}/{{ r.energy_max }}</span>
        </div>
        <div v-if="r.last_result" class="last" :class="isOkCode(r.last_result.code) ? 'ok' : 'bad'">
          <code>{{ r.last_result.action }}</code>
          <code class="arg">{{ r.last_result.arg }}</code>
          →
          <code>{{ r.last_result.code }}</code>
          <span class="dim">{{ codeLabel(r.last_result.code) }}</span>
        </div>
        <div v-else class="last dim">上 tick 未受理动作</div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.panel {
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 8px 10px;
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  min-height: 0;
}
h3 {
  margin: 0 0 6px;
  font-size: 13px;
}
.list {
  overflow: auto;
  display: grid;
  gap: 6px;
  align-content: start;
}
.robot {
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 5px;
  padding: 6px 8px;
  cursor: pointer;
}
.robot.sel {
  border-color: var(--accent);
}
.row1 {
  display: flex;
  gap: 10px;
  align-items: baseline;
}
.id {
  font-weight: 600;
}
.carry {
  color: var(--accent);
}
.energy {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 3px;
}
.energy .fill {
  height: 6px;
  border-radius: 3px;
  background: var(--ok);
  min-width: 2px;
}
.energy .fill.low {
  background: var(--bad);
}
.energy span {
  color: var(--dim);
  font-size: 11px;
}
.last {
  margin-top: 3px;
  font-size: 12px;
}
.last.ok {
  color: var(--ok);
}
.last.bad {
  color: var(--bad);
}
.last .arg {
  color: var(--dim);
}
.dim {
  color: var(--dim);
}
</style>
