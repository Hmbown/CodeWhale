// 本地绝对路径 → LSP file:// URI（Windows 盘符兼容）

/** 将绝对路径转为 LSP 使用的 file URI */
export function pathToFileUri(absPath: string): string {
  const normalized = absPath.replace(/\\/g, "/");
  if (/^[a-zA-Z]:/.test(normalized)) {
    return `file:///${normalized}`;
  }
  if (normalized.startsWith("/")) {
    return `file://${normalized}`;
  }
  return `file:///${normalized}`;
}
