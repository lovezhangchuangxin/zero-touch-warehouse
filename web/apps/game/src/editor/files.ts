// 多文件数据模型（预留）：宿主 hotReload 协议仍是单字符串（单文件整体
// 重建执行环境，docs/architecture/03），故当前恒为 1 个可编辑主文件；
// tab 条仅在文件数 > 1 时渲染。每文件多字符串的宿主协议（投递粒度、
// 模块解析、存档结构）属后续运行时里程碑，届时扩展本模型即可。

import type { Language } from "../scripts";

export interface EditorFile {
  id: string;
  name: string;
  language: Language;
  code: string;
}

export const MAIN_FILE_ID = "main";

/** 主文件名随语言走（tab 与未来日志面板源码定位共用该命名）。 */
export function fileNameFor(language: Language): string {
  return language === "py" ? "main.py" : "main.js";
}

export function makeMainFile(code: string, language: Language): EditorFile {
  return { id: MAIN_FILE_ID, name: fileNameFor(language), language, code };
}
