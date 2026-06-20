// Cursor 风格规则文件（.mdc）解析与序列化
// 格式：YAML frontmatter + Markdown 正文，与 .cursor/rules/*.mdc 兼容

/** 单条项目规则 */
export interface CursorRule {
  /** 文件名（不含扩展名），作为 id */
  id: string;
  /** 磁盘完整路径 */
  path: string;
  /** 规则说明（frontmatter description） */
  description: string;
  /** 文件匹配 glob，空表示不限制 */
  globs: string[];
  /** 是否始终注入上下文 */
  alwaysApply: boolean;
  /** 回合结束后自动执行的合规类型（frontmatter compliance） */
  compliance: string[];
  /** 规则正文（frontmatter 之后） */
  body: string;
}

/** 解析 .mdc / .md 文件内容为 frontmatter + body */
export function parseMdcFile(content: string): {
  description: string;
  globs: string[];
  alwaysApply: boolean;
  compliance: string[];
  body: string;
} {
  const trimmed = content.replace(/^\uFEFF/, "");
  const match = trimmed.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/);
  if (!match) {
    return { description: "", globs: [], alwaysApply: false, compliance: [], body: trimmed };
  }
  const yaml = match[1];
  const body = match[2];
  let description = "";
  let globs: string[] = [];
  let alwaysApply = false;
  let compliance: string[] = [];

  for (const line of yaml.split(/\r?\n/)) {
    const d = line.match(/^description:\s*(.+)$/);
    if (d) {
      description = unquoteYaml(d[1].trim());
      continue;
    }
    const a = line.match(/^alwaysApply:\s*(.+)$/i);
    if (a) {
      alwaysApply = /true/i.test(a[1].trim());
      continue;
    }
    const g = line.match(/^globs:\s*(.+)$/);
    if (g) {
      globs = parseGlobsValue(g[1].trim());
      continue;
    }
    const c = line.match(/^compliance:\s*(.+)$/i);
    if (c) {
      compliance = parseComplianceValue(c[1].trim());
    }
  }

  return { description, globs, alwaysApply, compliance, body };
}

/** 序列化为 .mdc 文件内容 */
export function serializeMdcFile(rule: {
  description: string;
  globs: string[];
  alwaysApply: boolean;
  compliance?: string[];
  body: string;
}): string {
  const lines = ["---"];
  if (rule.description.trim()) {
    lines.push(`description: ${quoteYaml(rule.description.trim())}`);
  }
  if (rule.globs.length > 0) {
    lines.push(`globs: ${formatGlobsYaml(rule.globs)}`);
  }
  if (rule.alwaysApply) {
    lines.push("alwaysApply: true");
  }
  if (rule.compliance && rule.compliance.length > 0) {
    lines.push(`compliance: ${formatComplianceYaml(rule.compliance)}`);
  }
  lines.push("---", "");
  return `${lines.join("\n")}${rule.body.trimEnd()}\n`;
}

/** 新建规则默认模板 */
export function defaultRuleTemplate(name: string): string {
  return serializeMdcFile({
    description: `${name} 项目规则`,
    globs: [],
    alwaysApply: true,
    body: `# ${name}\n\n在此编写 Agent 应遵守的项目规范。\n`,
  });
}

/** README 变更记录规则模板（alwaysApply，开发完成后必须更新文档） */
export function readmeChangelogRuleTemplate(): string {
  return serializeMdcFile({
    description: "代码变更后必须更新 README 变更记录",
    globs: [],
    alwaysApply: true,
    compliance: ["readme_changelog"],
    body: `# README 更新要求

凡是对项目代码、配置、接口或页面有任何新增、修改或删除，完成后**必须**同步更新 README 的「更新记录」章节。

每条记录包含：序号、更新时间、涉及模块、功能变更、实现说明。仅在 README 的「更新记录」章节追加，不要改动其他章节结构。
`,
  });
}

/** 代码注释规范模板 */
export function codeCommentsRuleTemplate(): string {
  return serializeMdcFile({
    description: "新增/修改代码必须补充中文注释",
    globs: [],
    alwaysApply: true,
    compliance: ["code_comments"],
    body: `# 代码注释规范

新增类、接口、结构体、枚举、方法、函数时须说明用途；复杂业务逻辑、条件判断、异常处理、数据库操作前须添加中文注释。

不要写无意义注释（如「定义变量」），注释应说明「做什么」和「为什么」。
`,
  });
}

/** 数据库建表规范模板 */
export function databaseSchemaRuleTemplate(): string {
  return serializeMdcFile({
    description: "数据库变更须输出完整建表/迁移 SQL",
    globs: [],
    alwaysApply: true,
    compliance: ["database_schema"],
    body: `# 数据库生成规则

涉及表结构变更时，必须输出含表注释、字段注释、主键与索引说明的 SQL；变更须说明影响模块与是否需要数据迁移。

表名、字段名使用小写下划线；每张表含 id、create_time、update_time（业务需要时可扩展）。
`,
  });
}

/** 测试要求模板 */
export function testsRequiredRuleTemplate(): string {
  return serializeMdcFile({
    description: "代码变更须补充或更新测试",
    globs: [],
    alwaysApply: true,
    compliance: ["tests_required"],
    body: `# 测试要求

对行为有影响的代码变更，须补充或更新相关单元/集成测试，并在完成时说明覆盖的主要场景。
`,
  });
}

/** API 文档模板 */
export function apiDocsRuleTemplate(): string {
  return serializeMdcFile({
    description: "API/路由变更须同步接口文档",
    globs: [],
    alwaysApply: true,
    compliance: ["api_docs"],
    body: `# API 文档要求

修改路由、Controller、Handler 或 OpenAPI 定义时，须同步更新接口文档（OpenAPI/Swagger 或 README API 章节），说明端点、参数与响应变更。
`,
  });
}

/** 内置规则模板清单（供 RulesView 快捷创建） */
export const RULE_TEMPLATES: { id: string; label: string; build: () => string }[] = [
  { id: "readme-changelog", label: "README 规则", build: readmeChangelogRuleTemplate },
  { id: "code-comments", label: "注释规范", build: codeCommentsRuleTemplate },
  { id: "database-schema", label: "数据库规范", build: databaseSchemaRuleTemplate },
  { id: "tests-required", label: "测试要求", build: testsRequiredRuleTemplate },
  { id: "api-docs", label: "API 文档", build: apiDocsRuleTemplate },
];

/** 规则目录相对项目根的路径 */
export const CURSOR_RULES_REL = ".cursor/rules";

/** 拼接规则目录绝对路径 */
export function rulesDirAbs(root: string): string {
  const sep = root.includes("\\") ? "\\" : "/";
  return `${root.replace(/[\\/]+$/, "")}${sep}${CURSOR_RULES_REL.replace(/\//g, sep)}`;
}

function unquoteYaml(v: string): string {
  if (
    (v.startsWith('"') && v.endsWith('"')) ||
    (v.startsWith("'") && v.endsWith("'"))
  ) {
    return v.slice(1, -1);
  }
  return v;
}

function quoteYaml(v: string): string {
  if (/[:#\n"'\\]/.test(v)) return JSON.stringify(v);
  return v;
}

function parseGlobsValue(raw: string): string[] {
  if (raw.startsWith("[")) {
    try {
      const arr = JSON.parse(raw.replace(/'/g, '"')) as unknown;
      if (Array.isArray(arr)) return arr.map(String).filter(Boolean);
    } catch {
      /* 忽略 */
    }
  }
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

function formatGlobsYaml(globs: string[]): string {
  if (globs.length === 1) return globs[0];
  return `[${globs.map((g) => JSON.stringify(g)).join(", ")}]`;
}

/** 解析 compliance frontmatter（逗号分隔或 YAML 数组） */
function parseComplianceValue(raw: string): string[] {
  if (raw.startsWith("[")) {
    try {
      const arr = JSON.parse(raw.replace(/'/g, '"')) as unknown;
      if (Array.isArray(arr)) return arr.map(String).filter(Boolean);
    } catch {
      /* 忽略 */
    }
  }
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

function formatComplianceYaml(items: string[]): string {
  if (items.length === 1) return items[0];
  return `[${items.map((i) => JSON.stringify(i)).join(", ")}]`;
}
