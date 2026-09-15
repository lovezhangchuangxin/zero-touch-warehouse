// 多文件数据模型：文件集整包提交给宿主（协议 v3：文件名 → 源码 + 入口，
// docs/architecture/03）。入口固定 main.js / main.py 随语言走，其余文件
// 平铺（首版约定无子目录）。语言切换在 js / py 两套文件集之间整体切换，
// 各自草稿互不丢失；文件 id 全局唯一且跨改名稳定——撤销历史缓存
// （stateCache）与恢复都按 id 键控。

import type { Language } from "../scripts";

export interface EditorFile {
  /** 全局唯一、跨改名稳定。 */
  id: string;
  /** 相对文件名（含扩展名；平铺，无路径分隔符）。 */
  name: string;
  language: Language;
  code: string;
}

export function extFor(language: Language): string {
  return language === "py" ? ".py" : ".js";
}

/** 入口文件名随语言走（编辑器与未来日志面板源码定位共用该命名）。 */
export function fileNameFor(language: Language): string {
  return language === "py" ? "main.py" : "main.js";
}

export function makeMainFile(code: string, language: Language): EditorFile {
  return { id: `main:${language}`, name: fileNameFor(language), language, code };
}

export function isMain(file: EditorFile): boolean {
  return file.id.startsWith("main:");
}

let nextFileId = 1;

export function makeFile(name: string, language: Language, code = ""): EditorFile {
  return { id: `f${nextFileId++}`, name, language, code };
}

/** 新建文件的默认名：scriptN，取当前文件集中未占用的最小 N。 */
export function nextFileName(files: EditorFile[], language: Language): string {
  const taken = new Set(files.map((f) => f.name));
  let n = 1;
  while (taken.has(`script${n}${extFor(language)}`)) {
    n++;
  }
  return `script${n}${extFor(language)}`;
}

/** 文件名合法性（平铺单段 + 语言对应扩展名，入口名由调用方排除）。
 *  py 侧与宿主同源：stem 必须是合法 Python 标识符——runtime-py 会拒绝
 *  非标识符与 stdlib / 关键字冲突名，宽松正则会产出保存必炸的名字。 */
export function isValidFileName(name: string, language: Language): boolean {
  return language === "py" ? /^[A-Za-z_][A-Za-z0-9_]*\.py$/.test(name) : /^[\w-]+\.js$/.test(name);
}
