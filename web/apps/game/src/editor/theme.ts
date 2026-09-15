// 编辑器主题：EditorView.theme + HighlightStyle 直接消费游戏 CSS 变量
// （App.vue :root 的 --panel / --panel-2 / --line / --fg / --dim / --ok /
// --bad / --warn / --accent），游戏换皮即编辑器换皮。
// 少量半透明叠色用 color-mix 从变量派生（WKWebView 16.2+），不引入
// 固定色值。选中底色沿用 App.vue button.on 的 #2c3a4e（面板内既定选中色）。

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

export const gameTheme = EditorView.theme(
  {
    "&": {
      height: "100%",
      fontSize: "12px",
      color: "var(--fg)",
      backgroundColor: "transparent",
    },
    "&.cm-focused": {
      outline: "none",
    },
    ".cm-scroller": {
      fontFamily: '"SF Mono", ui-monospace, Menlo, Consolas, monospace',
      lineHeight: "1.5",
      overflow: "auto",
    },
    ".cm-content": {
      caretColor: "var(--accent)",
    },
    ".cm-line": {
      padding: "0 8px",
    },
    ".cm-cursor, .cm-dropCursor": {
      borderLeftColor: "var(--accent)",
    },
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
      {
        backgroundColor: "#2c3a4e",
      },
    ".cm-gutters": {
      backgroundColor: "var(--panel-2)",
      color: "var(--dim)",
      border: "none",
      borderRight: "1px solid var(--line)",
    },
    ".cm-activeLine": {
      backgroundColor: "color-mix(in srgb, var(--panel-2) 60%, transparent)",
    },
    ".cm-activeLineGutter": {
      backgroundColor: "transparent",
      color: "var(--fg)",
    },
    ".cm-placeholder": {
      color: "var(--dim)",
    },
    ".cm-matchingBracket, &.cm-focused .cm-matchingBracket": {
      backgroundColor: "transparent",
      outline: "1px solid color-mix(in srgb, var(--accent) 60%, transparent)",
      color: "inherit",
    },
    ".cm-selectionMatch": {
      backgroundColor: "color-mix(in srgb, var(--warn) 25%, transparent)",
    },
    ".cm-searchMatch": {
      backgroundColor: "color-mix(in srgb, var(--warn) 25%, transparent)",
    },
    ".cm-searchMatch.cm-searchMatch-selected": {
      backgroundColor: "color-mix(in srgb, var(--warn) 50%, transparent)",
    },
    // 搜索面板（@codemirror/search，search({top: true}) 顶置）
    ".cm-panels": {
      backgroundColor: "var(--panel-2)",
      color: "var(--fg)",
      borderBottom: "1px solid var(--line)",
    },
    ".cm-panel.cm-search": {
      padding: "4px 8px",
      fontSize: "12px",
    },
    ".cm-panel.cm-search label": {
      fontSize: "11px",
    },
    ".cm-panel.cm-search input, .cm-textfield": {
      backgroundColor: "var(--panel)",
      border: "1px solid var(--line)",
      borderRadius: "4px",
      color: "var(--fg)",
      font: "inherit",
      padding: "2px 6px",
    },
    ".cm-panel.cm-search button, .cm-button": {
      backgroundColor: "var(--panel)",
      border: "1px solid var(--line)",
      borderRadius: "4px",
      color: "var(--fg)",
      font: "inherit",
      padding: "2px 8px",
      cursor: "pointer",
    },
    ".cm-panel.cm-search button:hover, .cm-button:hover": {
      borderColor: "var(--accent)",
    },
    ".cm-panel.cm-search input[type=checkbox]": {
      accentColor: "var(--accent)",
    },
    // 补全下拉与 info 面板
    ".cm-tooltip": {
      backgroundColor: "var(--panel-2)",
      border: "1px solid var(--line)",
      borderRadius: "6px",
      overflow: "hidden",
    },
    ".cm-tooltip.cm-tooltip-autocomplete > ul": {
      fontFamily: '"SF Mono", ui-monospace, Menlo, Consolas, monospace',
      maxHeight: "220px",
    },
    ".cm-tooltip.cm-tooltip-autocomplete > ul > li": {
      padding: "2px 8px",
    },
    ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": {
      backgroundColor: "var(--accent)",
      color: "#14161a",
    },
    ".cm-completionDetail": {
      color: "var(--dim)",
      fontStyle: "normal",
    },
    ".cm-completionInfo": {
      backgroundColor: "var(--panel-2)",
      border: "1px solid var(--line)",
      borderRadius: "6px",
      maxWidth: "340px",
      padding: "8px 10px",
      whiteSpace: "pre-wrap",
      lineHeight: "1.5",
      color: "var(--fg)",
    },
    ".cm-completionInfo.cm-completionInfo-left": {
      borderRight: "none",
    },
    // Game API 悬浮文档（completion.ts 注入 .cm-game-doc）
    ".cm-game-doc": {
      maxWidth: "340px",
      padding: "6px 10px",
    },
    ".cm-game-doc-sig": {
      fontFamily: '"SF Mono", ui-monospace, Menlo, Consolas, monospace',
      color: "var(--accent)",
      marginBottom: "4px",
    },
    ".cm-game-doc-text": {
      color: "var(--fg)",
      lineHeight: "1.5",
      whiteSpace: "pre-wrap",
    },
  },
  { dark: true },
);

export const gameHighlight = HighlightStyle.define([
  { tag: [t.comment], color: "var(--dim)", fontStyle: "italic" },
  {
    tag: [t.keyword, t.moduleKeyword, t.controlKeyword, t.operatorKeyword, t.self],
    color: "var(--accent)",
  },
  { tag: [t.string, t.special(t.string)], color: "var(--ok)" },
  { tag: [t.number, t.bool, t.null], color: "var(--warn)" },
  { tag: [t.regexp], color: "var(--warn)" },
  { tag: [t.definition(t.variableName), t.definition(t.propertyName)], fontWeight: "600" },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName],
    color: "color-mix(in srgb, var(--accent) 70%, var(--fg))",
  },
  {
    tag: [t.typeName, t.className, t.namespace],
    color: "color-mix(in srgb, var(--warn) 55%, var(--fg))",
  },
  { tag: [t.propertyName], color: "var(--fg)" },
  { tag: [t.variableName], color: "var(--fg)" },
  { tag: [t.invalid], color: "var(--bad)" },
  { tag: [t.meta, t.processingInstruction], color: "var(--dim)" },
]);

export const gameThemeExtension = [gameTheme, syntaxHighlighting(gameHighlight)];
