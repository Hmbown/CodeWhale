// Tab 补全：调用 Tauri 后端请求 DeepSeek 内联补全

import { isTauri, tabComplete as invokeTabComplete } from "../api/tauri";

/** 内联补全请求参数 */
export interface TabCompleteRequest {
  filePath: string;
  prefix: string;
  suffix: string;
  languageId?: string;
  /** 是否让补全自动补上缺失的 import 语句 */
  autoImport?: boolean;
}

/** 请求 AI 内联补全文本；失败或非 Tauri 返回空串 */
export async function fetchTabCompletion(req: TabCompleteRequest): Promise<string> {
  if (!isTauri()) return "";
  if (!req.prefix.trim() && !req.suffix.trim()) return "";
  try {
    const text = await invokeTabComplete({
      filePath: req.filePath,
      prefix: req.prefix,
      suffix: req.suffix,
      languageId: req.languageId ?? null,
      autoImport: req.autoImport ?? false,
    });
    return typeof text === "string" ? text : "";
  } catch {
    return "";
  }
}
