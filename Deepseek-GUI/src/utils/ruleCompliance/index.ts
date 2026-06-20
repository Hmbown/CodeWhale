// 规则合规：注册表入口 — 加载规则、批量检测缺口

import { isTauri, listDir, readFile } from "../../api/tauri";
import { parseMdcFile, rulesDirAbs, type CursorRule } from "../cursorRules";
import { COMPLIANCE_CHECKERS, buildCombinedCompliancePrompt } from "./checkers";
import type { Locale } from "../../i18n";
import type { RuleComplianceResult } from "./types";

export type { RuleComplianceGap, RuleComplianceResult, ComplianceTypeMeta } from "./types";
export { COMPLIANCE_TYPES } from "./types";
export { buildCombinedCompliancePrompt } from "./checkers";
export { getChecker } from "./checkers";

/** 从工作区加载全部 Cursor 规则（含 compliance 字段） */
export async function loadCursorRulesFromWorkspace(rootPath: string): Promise<CursorRule[]> {
  if (!isTauri() || !rootPath.trim()) return [];
  const dir = rulesDirAbs(rootPath);
  let entries: Awaited<ReturnType<typeof listDir>> = [];
  try {
    entries = await listDir(dir);
  } catch {
    return [];
  }
  const files = entries
    .filter((e) => !e.is_dir && /\.(mdc|md)$/i.test(e.name))
    .sort((a, b) => a.name.localeCompare(b.name));

  const rules: CursorRule[] = [];
  for (const f of files) {
    try {
      const fc = await readFile(f.path);
      const parsed = parseMdcFile(fc.content);
      const id = f.name.replace(/\.(mdc|md)$/i, "");
      rules.push({
        id,
        path: f.path,
        description: parsed.description,
        globs: parsed.globs,
        alwaysApply: parsed.alwaysApply,
        compliance: parsed.compliance,
        body: parsed.body,
      });
    } catch {
      /* 单条读取失败不影响其余规则 */
    }
  }
  return rules;
}

/**
 * 检测上一回合的全部合规缺口（按检查器 priority 排序）。
 * 同类型缺口合并 ruleIds。
 */
export function detectAllComplianceGaps(
  rules: CursorRule[],
  changedPaths: string[],
): RuleComplianceResult[] {
  const gaps: RuleComplianceResult[] = [];
  for (const checker of COMPLIANCE_CHECKERS) {
    const hit = checker.detect(rules, changedPaths);
    if (hit) gaps.push(hit);
  }
  return gaps;
}

/** 兼容旧 API：仅检测 README 缺口 */
export function detectReadmeChangelogGap(
  rules: CursorRule[],
  changedPaths: string[],
): RuleComplianceResult | null {
  return detectAllComplianceGaps(rules, changedPaths).find((g) => g.gap === "readme_changelog") ?? null;
}

/** 生成跟进提示词（单条或多条缺口） */
export function buildCompliancePrompt(gaps: RuleComplianceResult[], locale: Locale): string {
  return buildCombinedCompliancePrompt(gaps, locale);
}

// 路径工具 re-export（供测试或外部使用）
export {
  isReadmePath,
  isLikelyCodeOrConfigChange,
  isSourceCodePath,
  isTestPath,
} from "./pathUtils";
export { ruleHasCompliance } from "./ruleMatch";
