// 会话标签展示与命名（仿 Cursor：首条消息首行截断，未命名时显示「新对话」）

import { t, type Locale } from "../i18n";

/** 与后端 THREAD_TITLE_MAX_CHARS / TUI derive_session_title 一致 */
const TITLE_MAX_CHARS = 32;

/** 从用户消息首行推导标签标题 */
export function deriveThreadTitleFromMessage(text: string): string | null {
  const firstLine = text.trim().split(/\r?\n/)[0]?.trim() ?? "";
  if (!firstLine) return null;
  const chars = [...firstLine];
  if (chars.length > TITLE_MAX_CHARS) {
    return `${chars.slice(0, TITLE_MAX_CHARS).join("")}…`;
  }
  return firstLine;
}

/** 判断是否为未命名会话 */
export function isUntitledThread(thread: { title?: string | null }): boolean {
  return !thread.title?.trim();
}

/** 标签栏/列表展示名：有 title 用 title，否则「新对话」/ New chat */
export function formatThreadTabTitle(
  thread: { id: string; title?: string | null },
  locale: Locale,
): string {
  const title = thread.title?.trim();
  if (title) return title;
  return t("thread.defaultTitle", locale);
}
