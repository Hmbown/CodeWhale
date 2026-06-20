// 规则合规：工作区路径分类工具

/** 路径是否为 README 类文档 */
export function isReadmePath(path: string): boolean {
  const norm = path.replace(/\\/g, "/").toLowerCase();
  const base = norm.split("/").pop() ?? "";
  return base === "readme.md" || base === "readme.bak.md" || base.startsWith("readme_");
}

/** 路径是否为测试文件 */
export function isTestPath(path: string): boolean {
  const norm = path.replace(/\\/g, "/").toLowerCase();
  const base = norm.split("/").pop() ?? "";
  if (/\/tests?\//.test(norm) || /\/__tests__\//.test(norm)) return true;
  return (
    /\.(test|spec)\.(tsx?|jsx?)$/i.test(base) ||
    /_test\.(rs|go)$/i.test(base) ||
    /^test_.*\.py$/i.test(base) ||
    /Test\.(java|kt)$/i.test(base)
  );
}

/** 路径是否为 SQL / 迁移 / schema 相关 */
export function isSqlOrMigrationPath(path: string): boolean {
  const norm = path.replace(/\\/g, "/").toLowerCase();
  if (/\.(sql|ddl)$/i.test(norm)) return true;
  return (
    /\/migrations?\//.test(norm) ||
    /\/schema\//.test(norm) ||
    /\/db\/(migrate|migration)/.test(norm)
  );
}

/** 路径是否为数据库文档或设计说明 */
export function isDatabaseDocPath(path: string): boolean {
  const norm = path.replace(/\\/g, "/").toLowerCase();
  return (
    isSqlOrMigrationPath(path) ||
    /\/docs?\/(db|database|sql)\//.test(norm) ||
    /database\.md$/i.test(norm) ||
    /schema\.md$/i.test(norm)
  );
}

/** 路径是否为 API 层代码 */
export function isApiLayerPath(path: string): boolean {
  const norm = path.replace(/\\/g, "/").toLowerCase();
  const base = norm.split("/").pop() ?? "";
  return (
    /\/(routes?|controllers?|handlers?|api)\//.test(norm) ||
    /controller\.(java|kt|cs)$/i.test(base) ||
    /routes?\.(tsx?|jsx?|rs|py|go)$/i.test(base) ||
    /openapi\.(yaml|yml|json)$/i.test(base)
  );
}

/** 路径是否为 API 文档 */
export function isApiDocPath(path: string): boolean {
  const norm = path.replace(/\\/g, "/").toLowerCase();
  const base = norm.split("/").pop() ?? "";
  return (
    /openapi\.(yaml|yml|json)$/i.test(base) ||
    /swagger\.(yaml|yml|json)$/i.test(base) ||
    /\/docs?\/api\//.test(norm) ||
    /api\.md$/i.test(base) ||
    isReadmePath(path)
  );
}

/** 路径是否为源代码（非测试、非纯配置） */
export function isSourceCodePath(path: string): boolean {
  const norm = path.replace(/\\/g, "/");
  const lower = norm.toLowerCase();
  if (lower.includes("/.cursor/rules/")) return false;
  if (isReadmePath(path) || isTestPath(path)) return false;
  if (isSqlOrMigrationPath(path)) return false;

  const base = lower.split("/").pop() ?? "";
  const sourceExt =
    /\.(tsx?|jsx?|rs|py|go|java|kt|cs|cpp|c|h|vue|svelte|php|rb|swift)$/i;
  return sourceExt.test(base);
}

/** 路径是否像代码/配置变更（触发 README、测试等检查的广义「开发变更」） */
export function isLikelyCodeOrConfigChange(path: string): boolean {
  const norm = path.replace(/\\/g, "/");
  const lower = norm.toLowerCase();
  if (lower.includes("/.cursor/rules/")) return false;
  if (isReadmePath(path)) return false;
  if (/\/docs?\//i.test(lower) && /\.(md|mdx)$/i.test(lower) && !isDatabaseDocPath(path)) {
    return false;
  }

  const base = lower.split("/").pop() ?? "";
  const codeExt =
    /\.(tsx?|jsx?|rs|py|go|java|kt|cs|cpp|c|h|css|scss|html|vue|svelte|sql|toml|yaml|yml|json|xml|gradle|properties)$/i;
  const configNames = new Set([
    "cargo.toml",
    "package.json",
    "dockerfile",
    "makefile",
  ]);
  return (
    codeExt.test(base) ||
    configNames.has(base) ||
    isSourceCodePath(path) ||
    isApiLayerPath(path)
  );
}

/** 路径是否像 ORM / Entity / Model 层（可能涉及数据库结构） */
export function isEntityOrModelPath(path: string): boolean {
  const norm = path.replace(/\\/g, "/").toLowerCase();
  return (
    /\/(models?|entities|entity|domain|schemas?)\//.test(norm) ||
    /entity\.(java|kt|cs)$/i.test(norm) ||
    /model\.(rs|py|go)$/i.test(norm)
  );
}
