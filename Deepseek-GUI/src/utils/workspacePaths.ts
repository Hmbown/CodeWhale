// 工作区路径解析与从工具/ diff 文本中提取文件路径

/** 规范化工作区路径以便比较（Windows 忽略大小写与尾部分隔符） */
export function normalizeWorkspacePath(p: string): string {
  return p.trim().replace(/\//g, "\\").replace(/[\\/]+$/, "").toLowerCase();
}

/** 判断两个工作区路径是否指向同一目录 */
export function workspacePathsEqual(a: string, b: string): boolean {
  return normalizeWorkspacePath(a) === normalizeWorkspacePath(b);
}

/** 将相对路径解析为绝对路径（已绝对则原样返回） */
export function resolveWorkspacePath(root: string | null, p: string): string | null {
  if (!p.trim()) return null;
  const normalized = p.trim().replace(/\//g, "\\");
  // Windows 盘符或 Unix 根路径
  if (/^[a-zA-Z]:\\/.test(normalized) || normalized.startsWith("\\\\") || normalized.startsWith("/")) {
    return normalized;
  }
  if (!root) return normalized;
  const base = root.replace(/[\\/]+$/, "");
  const rel = normalized.replace(/^[./\\]+/, "");
  return `${base}\\${rel}`;
}

/** 从工具 input 对象中提取可能修改的文件路径 */
export function extractPathsFromToolInput(input: unknown): string[] {
  if (!input || typeof input !== "object") return [];
  const obj = input as Record<string, unknown>;
  const paths: string[] = [];
  if (typeof obj.path === "string" && obj.path) paths.push(obj.path);
  // 兼容部分模型误用的 file_path 字段名
  if (typeof obj.file_path === "string" && obj.file_path) paths.push(obj.file_path);
  if (Array.isArray(obj.files)) {
    for (const entry of obj.files) {
      if (entry && typeof entry === "object" && typeof (entry as Record<string, unknown>).path === "string") {
        paths.push((entry as Record<string, unknown>).path as string);
      }
    }
  }
  if (typeof obj.patch === "string" && obj.patch) {
    paths.push(...parsePathsFromDiff(obj.patch));
  }
  return [...new Set(paths)];
}

/** 从 SSE item.started 的 tool 载荷提取路径 */
export function extractPathsFromToolPayload(tool: unknown): string[] {
  if (!tool || typeof tool !== "object") return [];
  const t = tool as Record<string, unknown>;
  return extractPathsFromToolInput(t.input);
}

/** 从 unified diff 文本解析被修改的文件路径 */
export function parsePathsFromDiff(text: string): string[] {
  const paths: string[] = [];
  for (const line of text.split("\n")) {
    // +++ b/path 或 diff --git a/x b/y
    if (line.startsWith("+++ b/")) {
      paths.push(line.slice("+++ b/".length).trim());
    } else if (line.startsWith("diff --git ")) {
      const parts = line.split(" ");
      const b = parts.find((p) => p.startsWith("b/"));
      if (b) paths.push(b.slice(2));
    }
  }
  return [...new Set(paths.filter(Boolean))];
}
