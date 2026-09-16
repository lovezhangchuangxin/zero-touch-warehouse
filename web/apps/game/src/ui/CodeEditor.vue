<script setup lang="ts">
import type { EditorState } from "@codemirror/state";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as api from "../api";
import { DEMO_SCRIPTS, LANG_OPTIONS } from "../scripts";
import type { Language } from "../scripts";
import type { DraftsPayload } from "../types";
import { settings } from "../settings";
import { store } from "../store";
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
import Select from "./Select.vue";

// 多文件代码编辑器（CodeMirror 6，docs/architecture/05 §代码编辑器）。
// 「保存并重载」= 热重载：当前 tick 结束后暂停 → 文件集整包提交并重建
// 执行环境。语言切换走宿主重启（docs 03），js / py 两套文件集草稿各自
// 保留；已提交 Game.memory 跨语言保留。
//
// 草稿持久化（C1）：输入去抖 2s 整包落 drafts.json 防丢 + 刷新
// store.draftsCache（自动存档的草稿段来源）；读档经 store.pendingDrafts
// 回填（存档内草稿优先）。
const host = ref<HTMLElement | null>(null);
const demoSel = ref("");
const busy = ref(false);
const saveError = ref("");
// 示例下拉是静态集合，整表一次成型
const demoOptions = DEMO_SCRIPTS.map((d) => ({ value: d.id, label: d.name }));

const sets = ref<Record<Language, EditorFile[]>>({
  js: [makeMainFile("", "js")],
  py: [makeMainFile("", "py")],
});
const activeLang = ref<Language>(settings.lang);
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
    onChange: scheduleFlush,
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
  // 启动恢复防丢草稿（读档回填若先到则跳过——存档内草稿优先）。
  void api
    .loadDrafts()
    .then((d) => {
      if (d && !store.pendingDrafts) applyDrafts(d);
    })
    .catch(() => {
      // 无桥或读取失败：空草稿起步
    });
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onGlobalKey);
  if (import.meta.env.DEV) {
    delete (window as unknown as { __ztwEditor?: unknown }).__ztwEditor;
  }
  flushDrafts(); // 卸载前尽力把草稿落盘
  // 清掉在途去抖定时器：滞后触发时 ed 已销毁，docOf 会回落陈旧的
  // f.code 覆盖刚写的内容。
  clearTimeout(flushTimer);
  flushTimer = 0;
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

// ---------------------------------------------------------------------------
// 草稿整包：收集（供存档段 / 防丢文件）、去抖落盘、回填
// ---------------------------------------------------------------------------

/** 两语言文件集整包（文件名 → 源码；入口文件随语言固定）。 */
function collectDrafts(): DraftsPayload {
  const map = (lang: Language): Record<string, string> => {
    const out: Record<string, string> = {};
    for (const f of sets.value[lang]) {
      out[f.name] = docOf(f);
    }
    return out;
  };
  return { files: { js: map("js"), py: map("py") }, active: activeLang.value };
}

const FLUSH_MS = 2000;
let flushTimer = 0;
let flushing = false;

/** 内容或结构变化 → 去抖落盘（重排为立即刷新也不加倍写）。 */
function scheduleFlush() {
  if (flushTimer) return;
  flushTimer = window.setTimeout(() => {
    flushTimer = 0;
    flushDrafts();
  }, FLUSH_MS);
}

function flushDrafts() {
  if (flushing) {
    // 上一次 IPC 写入仍在途：尾部编辑补排一次，否则去抖到期被吞后
    // 无限期待在内存。
    scheduleFlush();
    return;
  }
  flushing = true;
  const drafts = collectDrafts();
  store.draftsCache = drafts;
  void api
    .saveDrafts(drafts)
    .catch(() => {
      // 落盘失败：缓存仍在，自动存档 / 下次去抖重试
    })
    .finally(() => {
      flushing = false;
    });
}

/** 读档回填（存档内草稿段或启动恢复）：整包替换两套文件集，文件 id
 *  重新分配（id 不属于草稿公开语义），编辑器切到活动语言入口文件。
 *  载荷形状守卫：drafts.json 被手改成合法 JSON 但形状不对时静默跳过
 *  （与 settings 的 coerce 同款宽容），不抛进 watch 回调。 */
function applyDrafts(d: DraftsPayload) {
  if (!ed || !d || typeof d.files !== "object") return;
  for (const lang of ["js", "py"] as const) {
    const map = d.files[lang] ?? {};
    const list: EditorFile[] = [];
    const entryName = fileNameFor(lang);
    if (map[entryName] !== undefined) {
      list.push(makeMainFile(map[entryName], lang));
    }
    for (const name of Object.keys(map).sort()) {
      if (name === entryName) continue;
      if (isValidFileName(name, lang)) list.push(makeFile(name, lang, map[name]));
    }
    if (list.length === 0) list.push(makeMainFile("", lang));
    for (const f of sets.value[lang]) stateCache.delete(f.id);
    sets.value[lang] = list;
  }
  const targetLang: Language = d.active === "py" ? "py" : "js";
  const main = sets.value[targetLang][0]!;
  // 旧对局的编辑器状态随整包替换一并丢弃：入口文件 id（main:js/py）跨档
  // 复用，若按旧 activeId 塞回缓存，非活动语言会被旧内容顶替并在随后的
  // flushDrafts 里固化进 drafts.json。
  activeLang.value = targetLang;
  ed.switchState(ed.newState(main.code, main.language));
  if (ed.currentLanguage() !== main.language) ed.setLanguage(main.language);
  activeId.value = main.id;
  flushDrafts();
}

// 读档草稿回填（watch token：同引用重复设置也能触发）。
watch(
  () => store.pendingDrafts?.token,
  (tok) => {
    if (tok && store.pendingDrafts) applyDrafts(store.pendingDrafts.drafts);
  },
);

defineExpose({ collectDrafts, applyDrafts });

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
    flushDrafts(); // 已提交内容与草稿模型对齐，顺带整包落盘
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
  scheduleFlush();
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
  scheduleFlush();
}

function renameFile(id: string, name: string) {
  const file = files.value.find((f) => f.id === id);
  if (!file || isMain(file) || !isValidFileName(name, file.language)) return;
  if (files.value.some((f) => f.name === name)) return;
  file.name = name;
  scheduleFlush();
}

// 载入示例 = 目标语言整套文件替换为单文件示例（示例自包含）。
// 跨语言载入时当前语言集是幸存方：先把 live state 写入缓存再切，
// 否则未保存草稿随 switchState 静默丢失；同语言整集替换属预期内丢弃。
function pickDemo(v: string) {
  demoSel.value = v; // 先落值再载入，loadDemo 按它找目标示例
  loadDemo();
}
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
  scheduleFlush();
}

// 语言切换 = 整体切换到另一套文件集（宿主重启在保存时发生，docs 03）。
function switchLang(v: string) {
  const lang: Language = v === "py" ? "py" : "js";
  if (!ed || lang === activeLang.value) return;
  stateCache.set(activeId.value, ed.view.state);
  activeLang.value = lang;
  const main = sets.value[lang][0]!;
  const cached = stateCache.get(main.id);
  ed.switchState(cached ?? ed.newState(main.code, lang));
  if (ed.currentLanguage() !== lang) ed.setLanguage(lang);
  activeId.value = main.id;
  scheduleFlush(); // 活动语言是草稿段的一部分
}
</script>

<template>
  <section class="flex min-h-0 flex-col overflow-hidden rounded-md border border-line bg-panel">
    <div
      class="flex items-center gap-2 overflow-hidden whitespace-nowrap border-b border-line px-2 py-1.5"
    >
      <Select
        :model-value="demoSel"
        class="min-w-0 max-w-[9.5rem]"
        :options="demoOptions"
        placeholder="载入示例…"
        aria-label="载入示例"
        @update:model-value="pickDemo"
      />
      <Select
        :model-value="activeLang"
        class="max-w-[7rem]"
        :options="LANG_OPTIONS"
        aria-label="语言"
        @update:model-value="switchLang"
      />
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
