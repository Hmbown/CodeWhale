// 规则合规检测：类型与检查器接口定义

import type { CursorRule } from "../cursorRules";
import type { Locale } from "../../i18n";

/** 内置合规类型 id（与 .mdc frontmatter `compliance:` 对齐） */
export type RuleComplianceGap =
  | "readme_changelog"
  | "code_comments"
  | "database_schema"
  | "tests_required"
  | "api_docs";

/** 合规类型元数据（UI / 模板用） */
export interface ComplianceTypeMeta {
  id: RuleComplianceGap;
  /** 中文展示名 */
  labelZh: string;
  labelEn: string;
}

/** 全部内置合规类型 */
export const COMPLIANCE_TYPES: ComplianceTypeMeta[] = [
  { id: "readme_changelog", labelZh: "README 变更记录", labelEn: "README changelog" },
  { id: "code_comments", labelZh: "代码注释", labelEn: "Code comments" },
  { id: "database_schema", labelZh: "数据库建表/迁移", labelEn: "Database schema" },
  { id: "tests_required", labelZh: "单元/集成测试", labelEn: "Tests" },
  { id: "api_docs", labelZh: "API 接口文档", labelEn: "API docs" },
];

/** 单条合规缺口 */
export interface RuleComplianceResult {
  gap: RuleComplianceGap;
  /** 触发该检测的规则 id */
  ruleIds: string[];
}

/** 合规检查器：每种规则类型实现一套触发/满足/跟进逻辑 */
export interface ComplianceChecker {
  id: RuleComplianceGap;
  /** 数值越小越优先处理 */
  priority: number;
  /** 规则是否启用此检查（explicit compliance 或关键词回退） */
  bindsRule(rule: CursorRule): boolean;
  /** 检测缺口；无缺口返回 null */
  detect(rules: CursorRule[], changedPaths: string[]): RuleComplianceResult | null;
  /** 生成自动跟进提示词 */
  buildPrompt(locale: Locale, ruleIds: string[]): string;
  /** i18n 横幅详情 key */
  detailI18nKey: `rule.${string}`;
  /** i18n 重试按钮 key */
  retryI18nKey: `rule.${string}`;
}
