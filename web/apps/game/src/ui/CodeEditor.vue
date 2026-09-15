<script setup lang="ts">
import type { EditorState } from "@codemirror/state";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import * as api from "../api";
import { store } from "../store";
import { DEMO_SCRIPTS } from "../scripts";
import type { Language } from "../scripts";
import { createCodeEditor, type CodeEditorHandle } from "../editor/setup";
import { fileNameFor, makeMainFile, type EditorFile } from "../editor/files";
import EditorTabs from "./EditorTabs.vue";

// 代码编辑器（CodeMirror 6，docs/architecture/05 §代码编辑器）。
// 「保存并重载」= 热重载：当前 tick 结束后暂停 → 保存即重建执行环境。
// 语言切换走宿主重启（docs 03：同一存档同一时间只运行一种语言），
// 已提交 Game.memory 保留。行为与 B2 textarea 版一致：示例载入、
// 语言下拉、失败提示、草稿保存时才提交 store。
const host = ref<HTMLElement | null>(null);
const demoSel = ref("");
const busy = ref(false);
const saveError = ref("");

// 多文件预留：当前恒为 1 个主文件（宿主协议单字符串），tab 条仅在 >1 时渲染。
const files = ref<EditorFile[]>([makeMainFile(store.code, store.language as Language)]);
const activeId = ref(files.value[0]!.id);
const stateCache = new Map<string, EditorState>();

const activeFile = computed(() => files.value.find((f) => f.id === activeId.value));
const langSel = computed<string>({
  get: () => activeFile.value?.language ?? "js",
  set: (v) => {
    const file = activeFile.value;
    if (!file) return;
    file.language = v as Language;
    file.name = fileNameFor(file.language);
    ed?.setLanguage(file.language);
  },
});

let ed: CodeEditorHandle | null = null;

function placeholderFor(language: Language): string {
  return language === "py" ? "def loop(): …" : "export function loop() { … }";
}

onMounted(() => {
  if (!host.value) return;
  const file = files.value[0]!;
  ed = createCodeEditor(host.value, {
    doc: file.code,
    language: file.language,
    placeholderFor,
    onSave: () => void save(),
  });
  // dev-only 自动化测试钩子（生产构建剔除）：自动化环境注入不了受信
  // 键盘事件，经 handle.typeAtEnd 以真实打字路径驱动补全等行为。
  if (import.meta.env.DEV) {
    (window as unknown as { __ztwEditor?: CodeEditorHandle }).__ztwEditor = ed;
  }
  // 全局 Cmd/Ctrl+S：焦点不在编辑器内（如按钮刚点过）也能保存。
  // 编辑器内 keymap 先消费一次，这里会再触发一次——双触发由 save()
  // 的 busy 排队去重（见 pendingSave），不会双发 hotReload。
  window.addEventListener("keydown", onGlobalKey);
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onGlobalKey);
  if (import.meta.env.DEV) {
    delete (window as unknown as { __ztwEditor?: unknown }).__ztwEditor;
  }
  ed?.destroy();
  ed = null;
  stateCache.clear();
});

function onGlobalKey(e: KeyboardEvent) {
  if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "s") {
    if (!e.defaultPrevented) e.preventDefault();
    void save();
  }
}

let pendingSave = false;

async function save() {
  if (!ed) return;
  if (busy.value) {
    // 重载进行中的再次保存（⌘S 连按）不静默丢弃，完成后补一次
    pendingSave = true;
    return;
  }
  busy.value = true;
  saveError.value = "";
  try {
    const code = ed.getDoc();
    const language = langSel.value;
    const file = activeFile.value;
    if (file) {
      file.code = code;
      file.language = language as Language;
      file.name = fileNameFor(file.language);
    }
    store.code = code;
    store.language = language;
    await api.hotReload(code, language);
  } catch (e) {
    saveError.value = `重载失败：${String(e)}`;
  } finally {
    busy.value = false;
    if (pendingSave) {
      pendingSave = false;
      void save();
    }
  }
}

function loadDemo() {
  const d = DEMO_SCRIPTS.find((s) => s.id === demoSel.value);
  if (!d || !ed) return;
  const file = activeFile.value;
  // 多文件预留：草稿同步进文件模型，切 tab 重建 state 时才有正确内容
  if (file) file.code = d.code;
  ed.setDoc(d.code);
  langSel.value = d.language;
  demoSel.value = "";
}

// 多文件切换（预留路径）：活动文件状态留在缓存（doc 与撤销历史都在
// state 里），目标文件无缓存则按其内容新建。语言隔间随 state 各自
// 持有，缓存态语言若与文件声明漂移则就地重配。
function selectTab(id: string) {
  if (!ed || id === activeId.value) return;
  stateCache.set(activeId.value, ed.view.state);
  const file = files.value.find((f) => f.id === id);
  if (!file) return;
  const cached = stateCache.get(id);
  ed.switchState(cached ?? ed.newState(file.code, file.language));
  if (ed.currentLanguage() !== file.language) ed.setLanguage(file.language);
  activeId.value = id;
}
</script>

<template>
  <section class="flex min-h-0 flex-col overflow-hidden rounded-md border border-line bg-panel">
    <div class="flex flex-wrap items-center gap-2 px-2 py-1.5">
      <select v-model="demoSel" class="field" @change="loadDemo">
        <option value="" disabled>载入示例…</option>
        <option v-for="d in DEMO_SCRIPTS" :key="d.id" :value="d.id">{{ d.name }}</option>
      </select>
      <select v-model="langSel" class="field" aria-label="语言">
        <option value="js">JavaScript</option>
        <option value="py">Python</option>
      </select>
      <button class="btn" :disabled="busy" @click="save">保存并重载</button>
      <span class="text-2xs text-dim"
        >⌘/Ctrl+S 保存并重载；重载会暂停世界并重建执行环境，Game.memory 保留，普通全局变量重置</span
      >
      <span v-if="saveError" class="text-warn">{{ saveError }}</span>
    </div>
    <EditorTabs v-if="files.length > 1" :files="files" :active-id="activeId" @select="selectTab" />
    <div
      ref="host"
      class="mx-2 mb-2 min-h-0 flex-1 overflow-hidden rounded-sm border border-line bg-panel"
    />
  </section>
</template>
