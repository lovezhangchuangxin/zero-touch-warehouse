<script setup lang="ts">
import { computed } from "vue";
import { fmtMilli } from "../format";
import { store } from "../store";

const gold = computed(() => fmtMilli(store.snapshot?.gold_milli ?? "0"));
const debt = computed(() => fmtMilli(store.snapshot?.debt_milli ?? "0"));
const sells = computed(() => store.snapshot?.sell_orders ?? []);
const buys = computed(() => store.snapshot?.buy_orders ?? []);
const mine = computed(() => store.snapshot?.my_orders ?? []);
</script>

<template>
  <section
    class="grid min-h-0 grid-rows-[auto_auto_auto_minmax(0,1fr)] gap-1 overflow-auto rounded-md border border-line bg-panel px-2.5 py-2"
  >
    <h3 class="flex items-baseline gap-3 text-sm font-semibold">
      市场
      <span class="font-normal"
        >金币 <b class="text-warn">{{ gold }}</b></span
      >
      <span v-if="debt !== '0.00'" class="text-bad">欠款 {{ debt }}</span>
    </h3>
    <div class="grid grid-cols-2 gap-2">
      <div>
        <h4 class="text-xs font-semibold text-dim">卖单（买入）</h4>
        <div
          v-for="o in sells"
          :key="o.id"
          class="flex gap-2 rounded-sm px-1 py-px text-xs hover:bg-panel-2/70"
        >
          <span class="font-mono text-dim">#{{ o.id }}</span>
          <span class="text-accent">{{ o.goods_type }}</span>
          <span>×{{ o.qty }}</span>
          <span class="font-mono">@{{ fmtMilli(o.unit_price_milli) }}</span>
        </div>
        <div v-if="sells.length === 0" class="px-1 py-1 text-xs text-dim">（空）</div>
      </div>
      <div>
        <h4 class="text-xs font-semibold text-dim">买单（卖出）</h4>
        <div
          v-for="o in buys"
          :key="o.id"
          class="flex gap-2 rounded-sm px-1 py-px text-xs hover:bg-panel-2/70"
        >
          <span class="font-mono text-dim">#{{ o.id }}</span>
          <span class="text-accent">{{ o.goods_type }}</span>
          <span>×{{ o.qty }}</span>
          <span class="font-mono">@{{ fmtMilli(o.unit_price_milli) }}</span>
        </div>
        <div v-if="buys.length === 0" class="px-1 py-1 text-xs text-dim">（空）</div>
      </div>
    </div>
    <h4 class="mt-0.5 text-xs font-semibold text-dim">已接订单</h4>
    <div class="min-h-0 overflow-auto">
      <div
        v-for="o in mine"
        :key="o.id"
        class="flex gap-2 rounded-sm px-1 py-px text-xs hover:bg-panel-2/70"
      >
        <span class="font-mono text-dim">#{{ o.id }}</span>
        <span class="text-accent">{{ o.goods_type }}</span>
        <span>×{{ o.qty }}</span>
        <span class="font-mono">@{{ fmtMilli(o.unit_price_milli) }}</span>
        <span v-if="o.vehicle" class="text-dim">在场车辆 #{{ o.vehicle }}</span>
        <span v-else class="text-dim">车辆待到场</span>
      </div>
      <div v-if="mine.length === 0" class="px-1 py-1 text-xs text-dim">（空）</div>
    </div>
  </section>
</template>
