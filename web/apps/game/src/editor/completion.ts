// 双语言共享的 Game 补全源与悬浮文档（docs/architecture/05「编辑器只做
// 展示」）：JS 与 Python 同一份实现——两种语言的成员访问拼写一致
// （Game.market.take），故用同一条点链解析即可。
//
// 叠加策略：
//   - 本源经 EditorState.languageData 全局注入，与语言包自带补全并存：
//     Python 由 @codemirror/lang-python 的 python() 自动挂接
//     globalCompletion / localCompletionSource；
//     JS 由 @codemirror/lang-javascript 自带 localCompletionSource
//     （基于 syntaxTree 的作用域声明收集，覆盖玩家自己的变量/函数）
//     与关键字补全。不重写收集器。
//   - 悬浮文档用 hoverTooltip，按同一条点链查 apiData。

import {
  snippetCompletion,
  type Completion,
  type CompletionContext,
  type CompletionSource,
} from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";
import { hoverTooltip } from "@codemirror/view";
import { CODE_LABELS } from "../codes";
import { GAME_MEMBERS, GAME_ROOT_DOC, RESULT_CODE_NOTE, type ApiMember } from "./apiData";

// ---- 点链解析 --------------------------------------------------------------

interface Chain {
  /** "Game" 之后的段（不含 Game），如 Game.market.take → ["market", "take"]。 */
  segments: string[];
  /** 点号之后正在输入的部分词（afterDot 时为 ""）。 */
  word: string;
  /** word 的起点（补全替换起点）。 */
  from: number;
  /** 光标前是否紧跟点号。 */
  afterDot: boolean;
}

const CHAIN_RE = /([A-Za-z_][\w$]*(?:\.[A-Za-z_][\w$]*)*)\.?$/;
const IDENT_RE = /^[A-Za-z_][\w$]*$/;
/** "Game" 的前缀（g/ga/gam/game，大小写任意）——根对象发现性补全用。 */
const GAME_PREFIX_RE = /^[Gg](?:[Aa](?:[Mm](?:[Ee])?)?)?$/;

function chainBefore(state: EditorState, pos: number): Chain | null {
  const line = state.doc.lineAt(pos);
  const before = line.text.slice(0, pos - line.from);
  const m = CHAIN_RE.exec(before);
  if (!m) return null;
  const segs = m[1].split(".");
  if (segs[0] !== "Game") {
    // 首段还不是 Game：仅当它是 Game 的前缀（正输入 Ga…）时保留链，
    // 供 GAME_SELF 补全根对象；其余标识符不归本源管。
    if (segs.length === 1 && GAME_PREFIX_RE.test(segs[0])) {
      return { segments: [], word: segs[0], from: pos - segs[0].length, afterDot: false };
    }
    return null;
  }
  const afterDot = before.endsWith(".");
  if (afterDot) return { segments: segs.slice(1), word: "", from: pos, afterDot: true };
  const word = segs[segs.length - 1];
  return {
    segments: segs.slice(1),
    word,
    from: pos - word.length,
    afterDot: false,
  };
}

/** 字符串 / 注释内不触发补全与悬浮（JS 与 Python 的节点名并集）；
 * 模板插值 ${...} / f-string {...} 内是合法代码，命中即放行。 */
const NO_COMPLETION_NODE =
  /^(String|TemplateString|FormatString|LineComment|BlockComment|Comment|RegExp)$/;
const INTERPOLATION_NODE = /^(Interpolation|FormatReplacement)$/;

function inStringOrComment(state: EditorState, pos: number): boolean {
  let node: SyntaxNode | null = syntaxTree(state).resolveInner(pos, -1);
  for (; node; node = node.parent) {
    if (INTERPOLATION_NODE.test(node.name)) break;
    if (NO_COMPLETION_NODE.test(node.name)) return true;
  }
  return false;
}

// ---- 补全选项 ---------------------------------------------------------------

const GAME_SELF: Completion = {
  label: "Game",
  type: "namespace",
  detail: "全局对象",
  info: GAME_ROOT_DOC,
  boost: 40,
};

function memberOption(m: ApiMember): Completion {
  const base: Completion = {
    label: m.name,
    type: m.kind,
    detail: m.signature,
    info: m.doc,
  };
  return m.template ? snippetCompletion(m.template, base) : base;
}

const RESULT_CODES: Completion[] = Object.entries(CODE_LABELS).map(([code, label]) => ({
  label: code,
  type: "constant",
  detail: label,
  info: `${label}。${RESULT_CODE_NOTE}`,
}));

/** 点链末段对应的成员列表；不可补全的路径返回 null。 */
function membersFor(path: string[]): Completion[] | null {
  if (path.length === 0) return GAME_MEMBERS.map(memberOption);
  const [head, ...rest] = path;
  if (head === "E") return rest.length === 0 ? RESULT_CODES : null;
  let members = GAME_MEMBERS;
  let current: ApiMember | undefined;
  for (const seg of path) {
    current = members.find((m) => m.name === seg);
    if (!current) return null;
    members = current.members ?? [];
  }
  return current ? (current.members ?? []).map(memberOption) : null;
}

// ---- 补全源 -----------------------------------------------------------------

export const gameCompletionSource: CompletionSource = (context: CompletionContext) => {
  if (inStringOrComment(context.state, context.pos)) return null;
  const chain = chainBefore(context.state, context.pos);
  if (!chain) return null;
  if (!chain.afterDot) {
    // 标识符位置：仅当它本身是「Game」的前缀时补 Game 本体
    if (
      chain.segments.length === 0 &&
      chain.word.length > 0 &&
      "game".startsWith(chain.word.toLowerCase())
    ) {
      return { from: chain.from, options: [GAME_SELF], validFor: IDENT_RE };
    }
    if (chain.segments.length === 0) return null;
    // 末段是正在输入的部分词，按去掉末段的路径取成员表（词由
    // validFor 过滤）；完整词时显式触发同样落到该词所属的成员表。
    const options = membersFor(chain.segments.slice(0, -1));
    if (!options) return null;
    return { from: chain.from, options, validFor: IDENT_RE };
  }
  const options = membersFor(chain.segments);
  if (!options) return null;
  return { from: chain.from, options, validFor: IDENT_RE };
};

// ---- 悬浮文档 ---------------------------------------------------------------

interface DocEntry {
  title: string;
  body: string;
}

function docFor(segments: string[]): DocEntry | null {
  if (segments.length === 0) return { title: "Game", body: GAME_ROOT_DOC };
  const [head, ...rest] = segments;
  if (head === "E") {
    if (rest.length === 0) {
      // 与补全 info 同文：直接取 apiData 中 E 的 doc
      const e = GAME_MEMBERS.find((m) => m.name === "E");
      return e ? { title: "Game.E", body: e.doc } : null;
    }
    if (rest.length > 1) return null;
    const label = CODE_LABELS[rest[0]];
    return label ? { title: `Game.E.${rest[0]}`, body: `${label}。${RESULT_CODE_NOTE}` } : null;
  }
  let members = GAME_MEMBERS;
  let current: ApiMember | undefined;
  for (const seg of segments) {
    current = members.find((m) => m.name === seg);
    if (!current) return null;
    members = current.members ?? [];
  }
  if (!current) return null;
  return { title: `Game.${segments.join(".")}`, body: current.doc };
}

function docDom(entry: DocEntry): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "cm-game-doc";
  const sig = document.createElement("div");
  sig.className = "cm-game-doc-sig";
  sig.textContent = entry.title;
  const body = document.createElement("div");
  body.className = "cm-game-doc-text";
  body.textContent = entry.body;
  wrap.append(sig, body);
  return wrap;
}

/** 悬停解析：从光标两侧扩展出完整标识符，再向左收集 `.word` 点链。
 * 返回 Game 之后的段（根对象返回 []）；不在 Game 链上返回 null。
 * 不复用 chainBefore——悬停点可在词内任意位置（含左右边缘）。 */
function hoverChain(state: EditorState, pos: number): string[] | null {
  const line = state.doc.lineAt(pos);
  const text = line.text;
  const col = pos - line.from;
  let start = col;
  let end = col;
  while (start > 0 && /[\w$]/.test(text[start - 1])) start--;
  while (end < text.length && /[\w$]/.test(text[end])) end++;
  if (start === end) return null;
  const segs: string[] = [text.slice(start, end)];
  for (;;) {
    if (start === 0 || text[start - 1] !== ".") break;
    const segEnd = start - 1;
    let ws = segEnd;
    while (ws > 0 && /[\w$]/.test(text[ws - 1])) ws--;
    if (ws === segEnd) break; // 点前不是标识符
    segs.push(text.slice(ws, segEnd));
    start = ws;
  }
  segs.reverse();
  if (segs[0] !== "Game") return null;
  return segs.slice(1);
}

export const gameDocTooltip = hoverTooltip((view, pos) => {
  if (inStringOrComment(view.state, pos)) return null;
  const segments = hoverChain(view.state, pos);
  if (!segments) return null;
  const entry = docFor(segments);
  if (!entry) return null;
  return {
    pos,
    create() {
      return { dom: docDom(entry) };
    },
  };
});
