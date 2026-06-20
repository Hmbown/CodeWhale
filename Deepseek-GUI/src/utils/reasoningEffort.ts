// 推理强度（reasoning_effort）可选项，与 config.toml / TUI 对齐

/** 推理强度枚举（写入 config.toml） */
export const REASONING_EFFORT_OPTIONS = ["max", "high", "medium", "low", "off"] as const;

export type ReasoningEffort = (typeof REASONING_EFFORT_OPTIONS)[number];

/** 循环切换到下一档 */
export function cycleReasoningEffort(current: string): ReasoningEffort {
  const list = REASONING_EFFORT_OPTIONS as readonly string[];
  const idx = list.indexOf(current);
  const next = idx < 0 ? 0 : (idx + 1) % list.length;
  return list[next]! as ReasoningEffort;
}

/** 显示标签（中英文） */
export function reasoningEffortLabel(effort: string, locale: "zh" | "en"): string {
  const mapZh: Record<string, string> = {
    max: "推理·Max",
    high: "推理·High",
    medium: "推理·Med",
    low: "推理·Low",
    off: "推理·Off",
  };
  const mapEn: Record<string, string> = {
    max: "Reason·Max",
    high: "Reason·High",
    medium: "Reason·Med",
    low: "Reason·Low",
    off: "Reason·Off",
  };
  return (locale === "zh" ? mapZh : mapEn)[effort] ?? effort;
}
