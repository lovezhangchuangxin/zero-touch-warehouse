// 编辑器组装：按需组合 CM6 扩展（不引入 basicSetup，控制体积），
// 语言经 Compartment 热切换（不重建 EditorView），缩进 js=2 / py=4。
// 多文件切换用 view.setState()（撤销历史随 state，见 CodeEditor.vue），
// 由此暴露 newState / switchState。

import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import {
  bracketMatching,
  indentOnInput,
  indentUnit,
  language as languageFacet,
} from "@codemirror/language";
import { search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, Transaction, type Extension } from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  keymap,
  lineNumbers,
  placeholder,
} from "@codemirror/view";
import { javascript } from "@codemirror/lang-javascript";
import { python, pythonLanguage } from "@codemirror/lang-python";
import type { Language } from "../scripts";
import { gameCompletionSource, gameDocTooltip } from "./completion";
import { gameThemeExtension } from "./theme";

export interface CodeEditorOptions {
  doc: string;
  language: Language;
  placeholderFor: (language: Language) => string;
  /** Cmd/Ctrl+S（编辑器聚焦时）。全局捕获在 CodeEditor.vue。 */
  onSave: () => void;
}

export interface CodeEditorHandle {
  view: EditorView;
  getDoc(): string;
  setDoc(code: string): void;
  setLanguage(language: Language): void;
  currentLanguage(): Language;
  newState(doc: string, language: Language): EditorState;
  switchState(state: EditorState): void;
  focus(): void;
  destroy(): void;
  /** dev-only：模拟文末连续输入（带 input.type 注解，补全按真实打字激活）。 */
  typeAtEnd?(text: string): void;
}

function languageSupport(lang: Language): Extension {
  return lang === "py" ? python() : javascript();
}

function indentFor(lang: Language): string {
  return lang === "py" ? "    " : "  ";
}

function stateLanguage(state: EditorState): Language {
  return state.facet(languageFacet) === pythonLanguage ? "py" : "js";
}

export function createCodeEditor(host: HTMLElement, opts: CodeEditorOptions): CodeEditorHandle {
  const langCompartment = new Compartment();
  const placeholderCompartment = new Compartment();

  const buildExtensions = (language: Language): Extension[] => [
    keymap.of([
      // Mod-s 全部默认 keymap 均无此绑定，放在首位仅求直观；
      // Enter/Esc 等补全键实际由 autocompletion() 自带的
      // Prec.highest keymap 接管，此处 completionKeymap 属冗余兜底。
      {
        key: "Mod-s",
        run: () => {
          opts.onSave();
          return true;
        },
      },
      ...closeBracketsKeymap,
      ...defaultKeymap,
      ...searchKeymap,
      ...historyKeymap,
      ...completionKeymap,
      indentWithTab,
    ]),
    lineNumbers(),
    highlightActiveLine(),
    history(),
    drawSelection(),
    indentOnInput(),
    bracketMatching(),
    closeBrackets(),
    // 不 override：与语言包自带补全（py 的 global/local、js 的 local）
    // 并存，Game 源经 languageData 全局注入（双语言一致）。
    autocompletion(),
    EditorState.languageData.of(() => [{ autocomplete: gameCompletionSource }]),
    gameDocTooltip,
    search({ top: true }),
    ...gameThemeExtension,
    langCompartment.of([languageSupport(language), indentUnit.of(indentFor(language))]),
    placeholderCompartment.of(placeholder(opts.placeholderFor(language))),
  ];

  const view = new EditorView({
    state: EditorState.create({ doc: opts.doc, extensions: buildExtensions(opts.language) }),
    parent: host,
  });

  const handle: CodeEditorHandle = {
    view,
    getDoc: () => view.state.doc.toString(),
    setDoc(code) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: code } });
    },
    setLanguage(language) {
      view.dispatch({
        effects: [
          langCompartment.reconfigure([
            languageSupport(language),
            indentUnit.of(indentFor(language)),
          ]),
          placeholderCompartment.reconfigure(placeholder(opts.placeholderFor(language))),
        ],
      });
    },
    currentLanguage: () => stateLanguage(view.state),
    newState: (doc, language) => EditorState.create({ doc, extensions: buildExtensions(language) }),
    switchState: (state) => {
      view.setState(state);
    },
    focus: () => view.focus(),
    destroy: () => view.destroy(),
  };

  if (import.meta.env.DEV) {
    // 测试钩子：自动化环境注入不了受信键盘事件，dev 下经事务模拟文末
    // 打字（input.type 注解使补全按真实路径激活）。生产构建剔除。
    handle.typeAtEnd = (text) => {
      const at = view.state.doc.length;
      view.dispatch({
        changes: { from: at, insert: text },
        selection: { anchor: at + text.length },
        annotations: Transaction.userEvent.of("input.type"),
        scrollIntoView: true,
      });
    };
  }

  return handle;
}
