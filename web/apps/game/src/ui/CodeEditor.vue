<script setup lang="ts">
import { ref } from "vue";
import * as api from "../api";
import { store } from "../store";
import { DEMO_SCRIPTS } from "../scripts";

// 代码输入（B2 为纯 textarea；Monaco / CodeMirror 选型属后续里程碑）。
// “保存并重载”= 热重载：当前 tick 结束后暂停 → 保存即重建执行环境
// （docs/architecture/05 §代码编辑器）。语言切换走宿主重启（docs 03：
// 同一存档同一时间只运行一种语言），已提交 Game.memory 保留。
const local = ref(store.code);
const langSel = ref(store.language);
const demoSel = ref("");
const busy = ref(false);

const saveError = ref("");
async function save() {
  busy.value = true;
  saveError.value = "";
  try {
    store.code = local.value;
    store.language = langSel.value;
    await api.hotReload(local.value, langSel.value);
  } catch (e) {
    saveError.value = `重载失败：${String(e)}`;
  } finally {
    busy.value = false;
  }
}
function loadDemo() {
  const d = DEMO_SCRIPTS.find((s) => s.id === demoSel.value);
  if (d) {
    local.value = d.code;
    langSel.value = d.language;
    demoSel.value = "";
  }
}
</script>

<template>
  <section class="editor">
    <div class="tools">
      <select v-model="demoSel" @change="loadDemo">
        <option value="" disabled>载入示例…</option>
        <option v-for="d in DEMO_SCRIPTS" :key="d.id" :value="d.id">{{ d.name }}</option>
      </select>
      <select v-model="langSel" aria-label="语言">
        <option value="js">JavaScript</option>
        <option value="py">Python</option>
      </select>
      <button :disabled="busy" @click="save">保存并重载</button>
      <span class="dim hint">重载会暂停世界并重建执行环境；Game.memory 保留，普通全局变量重置</span>
      <span v-if="saveError" class="warn">{{ saveError }}</span>
    </div>
    <textarea
      v-model="local"
      spellcheck="false"
      :placeholder="langSel === 'py' ? 'def loop(): …' : 'function loop() { … }'"
      class="code mono"
    />
  </section>
</template>

<style scoped>
.warn {
  color: #e8a24a;
}
.editor {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 6px;
  min-height: 0;
}
.tools {
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 6px 8px;
  flex-wrap: wrap;
}
.tools select {
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 4px;
  padding: 2px 4px;
}
.hint {
  font-size: 11px;
}
.code {
  margin: 0 8px 8px;
  background: #171a1f;
  border: 1px solid var(--line);
  border-radius: 5px;
  padding: 8px;
  resize: none;
  tab-size: 2;
  min-height: 0;
}
.dim {
  color: var(--dim);
}
</style>
