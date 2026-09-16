// 本机设置（settings.json，经 get_settings/set_settings 原子落盘；取代
// 早期 localStorage 方案——桌面产品里 webview 存储可能被系统「清除网站
// 数据」误伤，且不入 Cloud 同步集，docs/architecture/06 §文件格式）。
//
// main 在挂载前 loadSettings：布局 / 默认语言就位后再渲染，无默认值闪变。

import { reactive } from "vue";
import * as api from "./api";
import type { SettingsPayload } from "./types";

export const settings = reactive<SettingsPayload>({
  lang: "js",
  layout: null,
});

function coerce(v: SettingsPayload | null): SettingsPayload | null {
  if (!v) return null;
  const lang = v.lang === "py" ? "py" : "js";
  const layout =
    v.layout && typeof v.layout.edW === "number" && typeof v.layout.drH === "number"
      ? v.layout
      : null;
  return { lang, layout };
}

export async function loadSettings(): Promise<void> {
  try {
    const s = coerce(await api.getSettings());
    if (s) {
      settings.lang = s.lang;
      settings.layout = s.layout;
    }
  } catch {
    // 无 Tauri 桥（纯浏览器预览）或读取失败：默认值，仅本次会话生效
  }
}

/** 更新并整包落盘（patch 合入当前值；写失败时本次会话仍生效）。 */
export async function updateSettings(patch: Partial<SettingsPayload>): Promise<void> {
  if (patch.lang) settings.lang = patch.lang;
  if (patch.layout) settings.layout = patch.layout;
  try {
    await api.setSettings({ lang: settings.lang, layout: settings.layout });
  } catch {
    // 落盘失败：保持会话内生效
  }
}
