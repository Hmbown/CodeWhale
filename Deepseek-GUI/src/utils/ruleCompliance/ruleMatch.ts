// 规则与合规类型的绑定：frontmatter compliance 字段 + 关键词回退

import type { CursorRule } from "../cursorRules";
import type { RuleComplianceGap } from "./types";

/** 各合规类型的关键词回退（无 explicit compliance 时使用） */
const KEYWORD_FALLBACK: Record<RuleComplianceGap, RegExp> = {
  readme_changelog: /readme|更新记录|变更记录|开发变更|changelog/i,
  code_comments: /注释规范|必须.*注释|comment|docstring|javadoc|kdoc|xml注释/i,
  database_schema: /数据库|建表|表结构|migration|schema|字段注释|表注释|ddl/i,
  tests_required: /单元测试|集成测试|必须.*测试|test coverage|tests required/i,
  api_docs: /api文档|openapi|swagger|接口文档|api doc/i,
};

/** 规则是否绑定某合规类型（须 alwaysApply） */
export function ruleHasCompliance(rule: CursorRule, gap: RuleComplianceGap): boolean {
  if (!rule.alwaysApply) return false;
  if (rule.compliance.includes(gap)) return true;
  const text = `${rule.description}\n${rule.body}`;
  return KEYWORD_FALLBACK[gap].test(text);
}

/** 筛选绑定某合规类型的规则 */
export function rulesForGap(rules: CursorRule[], gap: RuleComplianceGap): CursorRule[] {
  return rules.filter((r) => ruleHasCompliance(r, gap));
}
