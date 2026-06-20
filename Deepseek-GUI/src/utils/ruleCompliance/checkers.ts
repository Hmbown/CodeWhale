// 内置合规检查器实现

import type { Locale } from "../../i18n";
import { CURSOR_RULES_REL } from "../cursorRules";
import type { CursorRule } from "../cursorRules";
import {
  isApiDocPath,
  isApiLayerPath,
  isDatabaseDocPath,
  isEntityOrModelPath,
  isLikelyCodeOrConfigChange,
  isReadmePath,
  isSourceCodePath,
  isSqlOrMigrationPath,
  isTestPath,
} from "./pathUtils";
import { ruleHasCompliance, rulesForGap } from "./ruleMatch";
import type { ComplianceChecker, RuleComplianceGap, RuleComplianceResult } from "./types";

function result(gap: RuleComplianceGap, rules: CursorRule[]): RuleComplianceResult {
  return { gap, ruleIds: rules.map((r) => r.id) };
}

/** README 变更记录：改了代码但未改 README */
const readmeChangelogChecker: ComplianceChecker = {
  id: "readme_changelog",
  priority: 10,
  bindsRule: (r) => ruleHasCompliance(r, "readme_changelog"),
  detect(rules, changedPaths) {
    const matched = rulesForGap(rules, "readme_changelog");
    if (matched.length === 0) return null;
    if (!changedPaths.some(isLikelyCodeOrConfigChange)) return null;
    if (changedPaths.some(isReadmePath)) return null;
    return result("readme_changelog", matched);
  },
  buildPrompt(locale, ruleIds) {
    const ids = ruleIds.join(", ");
    if (locale === "zh") {
      return `[规则合规·README] 本轮修改了代码/配置，但未更新 README（规则：${ids}）。请根据 ${CURSOR_RULES_REL} 要求在 README「更新记录」追加完整条目（序号、时间、模块、功能变更、实现说明），完成后简要确认。`;
    }
    return `[Rule compliance·README] Code/config changed but README was not updated (rules: ${ids}). Append a full changelog entry per ${CURSOR_RULES_REL}, then confirm briefly.`;
  },
  detailI18nKey: "rule.readmeDetail",
  retryI18nKey: "rule.retryReadme",
};

/** 代码注释：改了源码后提醒按规范补注释（无法静态验证注释密度，仅触发跟进） */
const codeCommentsChecker: ComplianceChecker = {
  id: "code_comments",
  priority: 20,
  bindsRule: (r) => ruleHasCompliance(r, "code_comments"),
  detect(rules, changedPaths) {
    const matched = rulesForGap(rules, "code_comments");
    if (matched.length === 0) return null;
    if (!changedPaths.some(isSourceCodePath)) return null;
    // 注释规范无法从路径判断是否已满足，每轮源码变更需 Agent 自检
    return result("code_comments", matched);
  },
  buildPrompt(locale, ruleIds) {
    const ids = ruleIds.join(", ");
    if (locale === "zh") {
      return `[规则合规·注释] 本轮修改了源代码（规则：${ids}）。请检查新增/修改的类、函数、复杂逻辑是否已按项目注释规范补充中文注释（说明用途与关键业务规则），补全后简要列出改动文件。`;
    }
    return `[Rule compliance·Comments] Source files changed (rules: ${ids}). Review new/edited code and add required comments per project rules, then list touched files.`;
  },
  detailI18nKey: "rule.commentsDetail",
  retryI18nKey: "rule.retryComments",
};

/** 数据库 schema：涉及 SQL/Entity 变更时须同步迁移或建表文档 */
const databaseSchemaChecker: ComplianceChecker = {
  id: "database_schema",
  priority: 15,
  bindsRule: (r) => ruleHasCompliance(r, "database_schema"),
  detect(rules, changedPaths) {
    const matched = rulesForGap(rules, "database_schema");
    if (matched.length === 0) return null;
    const dbTrigger = changedPaths.some(
      (p) => isSqlOrMigrationPath(p) || isEntityOrModelPath(p) || isLikelyCodeOrConfigChange(p),
    );
    if (!dbTrigger) return null;
    const dbSatisfied = changedPaths.some(isDatabaseDocPath);
    if (dbSatisfied) return null;
    // 仅改普通业务代码且规则要求「凡涉及数据库」—— 保守：有 entity/sql 变更才强制
    const strongTrigger = changedPaths.some(
      (p) => isSqlOrMigrationPath(p) || isEntityOrModelPath(p),
    );
    if (!strongTrigger) return null;
    return result("database_schema", matched);
  },
  buildPrompt(locale, ruleIds) {
    const ids = ruleIds.join(", ");
    if (locale === "zh") {
      return `[规则合规·数据库] 本轮涉及表结构/Entity 变更（规则：${ids}）。请按数据库规范输出或更新建表/迁移 SQL（含表注释、字段注释、索引说明），并说明影响模块；完成后简要确认。`;
    }
    return `[Rule compliance·DB] Schema/entity changes detected (rules: ${ids}). Provide migration DDL with table/column comments and index notes per project DB rules, then confirm.`;
  },
  detailI18nKey: "rule.databaseDetail",
  retryI18nKey: "rule.retryDatabase",
};

/** 测试：改代码但未改测试文件 */
const testsRequiredChecker: ComplianceChecker = {
  id: "tests_required",
  priority: 25,
  bindsRule: (r) => ruleHasCompliance(r, "tests_required"),
  detect(rules, changedPaths) {
    const matched = rulesForGap(rules, "tests_required");
    if (matched.length === 0) return null;
    const codeChanged = changedPaths.some(
      (p) => isLikelyCodeOrConfigChange(p) && !isTestPath(p),
    );
    if (!codeChanged) return null;
    if (changedPaths.some(isTestPath)) return null;
    return result("tests_required", matched);
  },
  buildPrompt(locale, ruleIds) {
    const ids = ruleIds.join(", ");
    if (locale === "zh") {
      return `[规则合规·测试] 本轮修改了代码但未更新测试（规则：${ids}）。请为本次变更补充或更新相关单元/集成测试，并简要说明覆盖场景。`;
    }
    return `[Rule compliance·Tests] Code changed without test updates (rules: ${ids}). Add or update tests for this change and summarize coverage.`;
  },
  detailI18nKey: "rule.testsDetail",
  retryI18nKey: "rule.retryTests",
};

/** API 文档：改 API 层但未改文档 */
const apiDocsChecker: ComplianceChecker = {
  id: "api_docs",
  priority: 18,
  bindsRule: (r) => ruleHasCompliance(r, "api_docs"),
  detect(rules, changedPaths) {
    const matched = rulesForGap(rules, "api_docs");
    if (matched.length === 0) return null;
    if (!changedPaths.some(isApiLayerPath)) return null;
    if (changedPaths.some(isApiDocPath)) return null;
    return result("api_docs", matched);
  },
  buildPrompt(locale, ruleIds) {
    const ids = ruleIds.join(", ");
    if (locale === "zh") {
      return `[规则合规·API] 本轮修改了 API/路由层（规则：${ids}）。请同步更新 OpenAPI/Swagger 或接口文档，说明新增/变更的端点与参数，完成后简要确认。`;
    }
    return `[Rule compliance·API] API/routes changed (rules: ${ids}). Update OpenAPI/Swagger or API docs for new/changed endpoints, then confirm.`;
  },
  detailI18nKey: "rule.apiDetail",
  retryI18nKey: "rule.retryApi",
};

/** 全部检查器（按 priority 排序） */
export const COMPLIANCE_CHECKERS: ComplianceChecker[] = [
  readmeChangelogChecker,
  databaseSchemaChecker,
  apiDocsChecker,
  codeCommentsChecker,
  testsRequiredChecker,
].sort((a, b) => a.priority - b.priority);

/** 根据 id 查找检查器 */
export function getChecker(gap: RuleComplianceGap): ComplianceChecker | undefined {
  return COMPLIANCE_CHECKERS.find((c) => c.id === gap);
}

/** 生成多缺口合并跟进提示（按优先级拼接） */
export function buildCombinedCompliancePrompt(
  gaps: RuleComplianceResult[],
  locale: Locale,
): string {
  if (gaps.length === 1) {
    const c = getChecker(gaps[0].gap);
    return c?.buildPrompt(locale, gaps[0].ruleIds) ?? "";
  }
  const parts = gaps
    .map((g) => getChecker(g.gap)?.buildPrompt(locale, g.ruleIds))
    .filter(Boolean);
  const header =
    locale === "zh"
      ? `[规则合规·多项] 本轮存在 ${gaps.length} 项未完成的项目规则，请依次处理：\n\n`
      : `[Rule compliance·multiple] ${gaps.length} project rules pending:\n\n`;
  return header + parts.map((p, i) => `${i + 1}. ${p}`).join("\n\n");
}
