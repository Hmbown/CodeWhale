// 后端运行时 status 消息中文化（引擎事件为英文，GUI 按 locale 翻译展示）

import type { Locale } from "./index";

/** 精确匹配的英文 → 中文 */
const EXACT_ZH: Record<string, string> = {
  "Executing tools sequentially (writes, approvals, or non-parallel tools detected)":
    "检测到写入、审批或非并行工具，正在串行执行",
  "Session context synced": "会话上下文已同步",
  "Request cancelled": "请求已取消",
  "Reached maximum steps": "已达到最大步数",
  "Auto-compacting context...": "正在自动压缩上下文…",
  "Stripped non-API tool-call wrapper from model output (use the API tool channel)":
    "已从模型输出中移除非 API 工具调用包装（请使用 API 工具通道）",
};

/** 正则模式匹配（按顺序尝试） */
const PATTERN_ZH: Array<{ re: RegExp; repl: string | ((m: RegExpMatchArray) => string) }> = [
  {
    re: /^Executing (\d+) read-only tools in (\d+) parallel chunk\(s\)$/,
    repl: "正在并行执行 $1 个只读工具（$2 个批次）",
  },
  {
    re: /^Steer input accepted: (.+)$/,
    repl: (_m) => `转向输入已接受：${_m[1]}`,
  },
  {
    re: /^Steer input queued: (.+)$/,
    repl: (_m) => `转向输入已排队：${_m[1]}`,
  },
  {
    re: /^Approved tool call: (.+)$/,
    repl: (_m) => `已批准工具调用：${_m[1]}`,
  },
  {
    re: /^Denied tool call: (.+)$/,
    repl: (_m) => `已拒绝工具调用：${_m[1]}`,
  },
  {
    re: /^Mode changed to: (.+)$/,
    repl: (_m) => `模式已切换为：${_m[1]}`,
  },
  {
    re: /^REPL init failed: (.+)$/,
    repl: (_m) => `REPL 初始化失败：${_m[1]}`,
  },
];

/**
 * 将后端运行时 status 文本翻译为当前语言；未知文案原样返回。
 */
export function translateRuntimeStatus(text: string, locale: Locale): string {
  if (!text || locale !== "zh") return text;

  const exact = EXACT_ZH[text.trim()];
  if (exact) return exact;

  for (const { re, repl } of PATTERN_ZH) {
    const m = text.match(re);
    if (!m) continue;
    return typeof repl === "string" ? text.replace(re, repl) : repl(m);
  }

  return text;
}
