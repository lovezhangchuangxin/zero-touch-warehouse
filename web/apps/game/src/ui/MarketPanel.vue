<script setup lang="ts">
import { computed } from "vue";
import { fmtMilli } from "../format";
import { vFade } from "./fade";
import { store } from "../store";

// 抽屉页签内容。金币 / 欠款读数在顶栏（DebugBar），这里只展示挂单与已接订单。
const sells = computed(() => store.snapshot?.sell_orders ?? []);
const buys = computed(() => store.snapshot?.buy_orders ?? []);
const mine = computed(() => store.snapshot?.my_orders ?? []);
</script>

<template>
  <div class="flex min-h-0 flex-col">
    <h3 class="panel-h px-2.5 pt-2">
      {{ "市场 "
      }}<span class="font-normal"
        >挂单 {{ sells.length + buys.length }} · 已接 {{ mine.length }}</span
      >
    </h3>
    <div v-fade class="min-h-0 flex-1 overflow-auto px-2.5 pb-2 pt-1.5">
      <div class="grid grid-cols-2 gap-x-4 gap-y-1">
        <section>
          <h4 class="text-2xs font-semibold text-dim">卖单（买入）</h4>
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
        </section>
        <section>
          <h4 class="text-2xs font-semibold text-dim">买单（卖出）</h4>
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
        </section>
      </div>
      <h4 class="mt-2 text-2xs font-semibold text-dim">已接订单</h4>
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
  </div>
</template>
