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
  <section class="panel">
    <h3>
      市场
      <span class="money"
        >金币 <b>{{ gold }}</b></span
      >
      <span v-if="debt !== '0.00'" class="debt">欠款 {{ debt }}</span>
    </h3>
    <div class="cols">
      <div>
        <h4>卖单（买入）</h4>
        <div v-for="o in sells" :key="o.id" class="order">
          <span class="mono dim">#{{ o.id }}</span>
          <span class="goods">{{ o.goods_type }}</span>
          <span>×{{ o.qty }}</span>
          <span class="mono">@{{ fmtMilli(o.unit_price_milli) }}</span>
        </div>
        <div v-if="sells.length === 0" class="dim empty">（空）</div>
      </div>
      <div>
        <h4>买单（卖出）</h4>
        <div v-for="o in buys" :key="o.id" class="order">
          <span class="mono dim">#{{ o.id }}</span>
          <span class="goods">{{ o.goods_type }}</span>
          <span>×{{ o.qty }}</span>
          <span class="mono">@{{ fmtMilli(o.unit_price_milli) }}</span>
        </div>
        <div v-if="buys.length === 0" class="dim empty">（空）</div>
      </div>
    </div>
    <h4 class="mine-title">已接订单</h4>
    <div class="mine">
      <div v-for="o in mine" :key="o.id" class="order">
        <span class="mono dim">#{{ o.id }}</span>
        <span class="goods">{{ o.goods_type }}</span>
        <span>×{{ o.qty }}</span>
        <span class="mono">@{{ fmtMilli(o.unit_price_milli) }}</span>
        <span v-if="o.vehicle" class="dim">在场车辆 #{{ o.vehicle }}</span>
        <span v-else class="dim">车辆待到场</span>
      </div>
      <div v-if="mine.length === 0" class="dim empty">（空）</div>
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
  grid-template-rows: auto auto auto minmax(0, 1fr);
  gap: 4px;
  min-height: 0;
  overflow: auto;
}
h3 {
  margin: 0;
  font-size: 13px;
  display: flex;
  gap: 12px;
  align-items: baseline;
}
h4 {
  margin: 2px 0 0;
  font-size: 12px;
  color: var(--dim);
}
.money b {
  color: var(--warn);
}
.debt {
  color: var(--bad);
}
.cols {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}
.order {
  display: flex;
  gap: 8px;
  font-size: 12px;
  padding: 1px 0;
}
.goods {
  color: var(--accent);
}
.mine {
  overflow: auto;
}
.empty {
  font-size: 12px;
}
.dim {
  color: var(--dim);
}
</style>
