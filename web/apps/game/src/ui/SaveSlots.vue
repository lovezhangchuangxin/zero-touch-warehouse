<script setup lang="ts">
import { computed } from "vue";
import type { SaveSummary } from "../types";

// 存档列表（主菜单读档 / 暂停浮层存读档 / 设置面板存档管理三处复用）。
// 损坏档（ok=false）灰条展示原因、不可载入；删除仅管理场景开放。
const props = defineProps<{
  saves: SaveSummary[];
  /** 场景 id → 显示名（StaticInfo.scenarios；缺省回退 id）。 */
  scenarioNames?: Record<string, string>;
  deletable?: boolean;
  emptyHint?: string;
}>();
const emit = defineEmits<{ load: [id: string]; delete: [id: string] }>();

/** 新→旧；自动档与手动档混排，自动徽标区分。 */
const ordered = computed(() =>
  [...props.saves].sort((a, b) => Number(b.created_at_ms) - Number(a.created_at_ms)),
);

function timeOf(s: SaveSummary): string {
  const n = Number(s.created_at_ms);
  if (!Number.isFinite(n) || n <= 0) return "—";
  return new Date(n).toLocaleString();
}

function titleOf(s: SaveSummary): string {
  return s.name || (s.auto ? "自动存档" : timeOf(s));
}

function scenarioOf(s: SaveSummary): string {
  return props.scenarioNames?.[s.scenario] ?? s.scenario;
}
</script>

<template>
  <div class="flex flex-col gap-1" role="list" aria-label="存档列表">
    <p v-if="ordered.length === 0" class="px-3 py-2 text-sm text-dim">
      {{ emptyHint ?? "暂无存档" }}
    </p>
    <div
      v-for="s in ordered"
      :key="s.id"
      class="flex items-center gap-2 rounded-sm border px-3 py-2 transition-colors"
      :class="
        s.ok
          ? 'border-line bg-panel/80 hover:border-accent hover:bg-panel-2'
          : 'border-warn/40 bg-panel/40 opacity-60'
      "
      role="listitem"
    >
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="truncate text-sm font-semibold text-fg">{{ titleOf(s) }}</span>
          <span v-if="s.auto" class="shrink-0 rounded-sm border border-line px-1 text-2xs text-dim"
            >自动</span
          >
          <span
            v-if="!s.ok"
            class="shrink-0 rounded-sm border border-warn/50 px-1 text-2xs text-warn"
            >损坏</span
          >
        </div>
        <div class="mt-0.5 truncate text-2xs text-dim">
          {{ scenarioOf(s) }} · tick {{ s.tick }} · {{ timeOf(s) }}
        </div>
        <div v-if="!s.ok && s.error" class="mt-0.5 truncate text-2xs text-warn" :title="s.error">
          {{ s.error }}
        </div>
      </div>
      <button class="btn shrink-0 px-2 py-1 text-xs" :disabled="!s.ok" @click="emit('load', s.id)">
        载入
      </button>
      <button v-if="deletable" class="btn shrink-0 px-2 py-1 text-xs" @click="emit('delete', s.id)">
        删除
      </button>
    </div>
  </div>
</template>
