<script setup lang="ts">
import type { EditorState } from "@codemirror/state";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import * as api from "../api";
import { DEMO_SCRIPTS } from "../scripts";
import type { Language } from "../scripts";
import { createCodeEditor, type CodeEditorHandle } from "../editor/setup";
import {
  fileNameFor,
  isMain,
  isValidFileName,
  makeFile,
  makeMainFile,
  nextFileName,
  type EditorFile,
} from "../editor/files";
import FileTree from "./FileTree.vue";

// 多文件代码编辑器（CodeMirror 6，docs/architecture/05 §代码编辑器）。
// 「保存并重载」= 热重载：当前 tick 结束后暂停 → 文件集整包提交并重建
// 执行环境。语言切换走宿主重启（docs 03），js / py 两套文件集草稿各自
// 保留；已提交 Game.memory 跨语言保留。
const host = ref<HTMLElement | null>(null);
const demoSel = ref("");
const busy = ref(false);
const saveError = ref("");

// 两套语言文件集（各自草稿），activeLang 指向当前编辑的那套。
// 初始语言读设置（主菜单"设置 → 默认脚本语言"，localStorage 持久化）。
function initialLanguage(): Language {
  try {
    return localStorage.getItem("ztw.editor.lang") === "py" ? "py" : "js";
  } catch {
    return "js";
  }
}
const sets = ref<Record<Language, EditorFile[]>>({
  js: [makeMainFile("", "js")],
  py: [makeMainFile("", "py")],
});
const activeLang = ref<Language>(initialLanguage());
const activeId = ref(sets.value[activeLang.value][0]!.id);
const stateCache = new Map<string, EditorState>();

const files = computed(() => sets.value[activeLang.value]);

let ed: CodeEditorHandle | null = null;

function placeholderFor(language: Language): string {
  return language === "py" ? "def loop(): …" : "export function loop() { … }";
}

onMounted(() => {
  if (!host.value) return;
  const file = sets.value[activeLang.value][0]!;
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
  // 编辑器内 keymap 消费时会 preventDefault，这里据此去重——同一次
  // 按键只走一条路径，不会双发热重载。
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
    if (e.defaultPrevented) return; // 编辑器内 keymap 已处理
    e.preventDefault();
    void save();
  }
}

/** 活动文件内容取自编辑器，其余文件取撤销缓存 / 文件模型的最新草稿。 */
function docOf(f: EditorFile): string {
  if (f.id === activeId.value) {
    return ed?.getDoc() ?? f.code;
  }
  return stateCache.get(f.id)?.doc.toString() ?? f.code;
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
    const language = activeLang.value;
    const set = sets.value[language];
    const map: Record<string, string> = {};
    for (const f of set) {
      map[f.name] = docOf(f);
    }
    const main = set.find(isMain);
    await api.hotReload(map, main?.name ?? fileNameFor(language), language);
    // 成功才回写草稿模型：f.code 与宿主已提交内容保持一致，
    // 消除「无缓存才准读 f.code」这一无保护的不变量。
    for (const f of set) {
      const code = map[f.name];
      if (code !== undefined) f.code = code;
    }
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

// 多文件切换：活动文件状态留在缓存（doc 与撤销历史都在 state 里），
// 目标文件无缓存则按其内容新建。缓存态语言若与文件声明漂移则就地重配。
function selectFile(id: string) {
  if (!ed || id === activeId.value) return;
  const target = files.value.find((f) => f.id === id);
  if (!target) return;
  stateCache.set(activeId.value, ed.view.state);
  const cached = stateCache.get(id);
  ed.switchState(cached ?? ed.newState(target.code, target.language));
  if (ed.currentLanguage() !== target.language) ed.setLanguage(target.language);
  activeId.value = id;
}

function createFile() {
  if (!ed) return;
  const file = makeFile(nextFileName(files.value, activeLang.value), activeLang.value);
  files.value.push(file);
  selectFile(file.id);
}

function deleteFile(id: string) {
  if (!ed) return;
  const set = files.value;
  const file = set.find((f) => f.id === id);
  if (!file || isMain(file)) return;
  if (id === activeId.value) {
    selectFile(set[0]!.id); // 入口文件恒在，回主文件
  }
  set.splice(set.indexOf(file), 1);
  stateCache.delete(id);
}

function renameFile(id: string, name: string) {
  const file = files.value.find((f) => f.id === id);
  if (!file || isMain(file) || !isValidFileName(name, file.language)) return;
  if (files.value.some((f) => f.name === name)) return;
  file.name = name;
}

// 载入示例 = 目标语言整套文件替换为单文件示例（示例自包含）。
// 跨语言载入时当前语言集是幸存方：先把 live state 写入缓存再切，
// 否则未保存草稿随 switchState 静默丢失；同语言整集替换属预期内丢弃。
function loadDemo() {
  const d = DEMO_SCRIPTS.find((s) => s.id === demoSel.value);
  if (!d || !ed) return;
  if (d.language !== activeLang.value) {
    stateCache.set(activeId.value, ed.view.state);
  }
  const old = sets.value[d.language];
  for (const f of old) {
    stateCache.delete(f.id);
  }
  sets.value[d.language] = [makeMainFile(d.code, d.language)];
  activeLang.value = d.language;
  const main = sets.value[d.language][0]!;
  ed.switchState(ed.newState(main.code, main.language));
  activeId.value = main.id;
  demoSel.value = "";
}

// 语言切换 = 整体切换到另一套文件集（宿主重启在保存时发生，docs 03）。
const langSel = computed<Language>({
  get: () => activeLang.value,
  set: (v) => {
    if (!ed || v === activeLang.value) return;
    stateCache.set(activeId.value, ed.view.state);
    activeLang.value = v;
    const main = sets.value[v][0]!;
    const cached = stateCache.get(main.id);
    ed.switchState(cached ?? ed.newState(main.code, v));
    if (ed.currentLanguage() !== v) ed.setLanguage(v);
    activeId.value = main.id;
  },
});
</script>

<template>
  <section class="flex min-h-0 flex-col overflow-hidden rounded-md border border-line bg-panel">
    <div
      class="flex items-center gap-2 overflow-hidden whitespace-nowrap border-b border-line px-2 py-1.5"
    >
      <select v-model="demoSel" class="field min-w-0 max-w-[9.5rem]" @change="loadDemo">
        <option value="" disabled>载入示例…</option>
        <option v-for="d in DEMO_SCRIPTS" :key="d.id" :value="d.id">{{ d.name }}</option>
      </select>
      <select v-model="langSel" class="field min-w-0 max-w-[7rem]" aria-label="语言">
        <option value="js">JavaScript</option>
        <option value="py">Python</option>
      </select>
      <button class="btn shrink-0" :disabled="busy" @click="save">保存并重载</button>
      <span v-if="saveError" class="min-w-0 truncate text-warn">{{ saveError }}</span>
    </div>
    <div class="flex min-h-0 flex-1">
      <FileTree
        :files="files"
        :active-id="activeId"
        @select="selectFile"
        @create="createFile"
        @delete="deleteFile"
        @rename="renameFile"
      />
      <div ref="host" class="min-h-0 flex-1 overflow-hidden bg-panel" />
    </div>
    <div class="truncate border-t border-line px-2 py-1 text-2xs text-dim">
      ⌘/Ctrl+S 保存并重载（整包提交全部文件）；重载会暂停世界并重建执行环境，Game.memory
      保留，普通全局变量重置
    </div>
  </section>
</template>
