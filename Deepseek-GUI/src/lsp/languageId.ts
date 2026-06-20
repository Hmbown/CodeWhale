// LSP languageId：与 Rust registry 对齐

/** 从文件路径解析 LSP languageId；无 LSP 支持时返回 null */
export function languageIdFromPath(path: string): string | null {
  const base = path.split(/[\\/]/).pop()?.toLowerCase() ?? "";
  const ext = base.includes(".") ? (base.split(".").pop() ?? "") : base;
  switch (ext) {
    case "rs":
      return "rust";
    case "go":
      return "go";
    case "py":
    case "pyi":
      return "python";
    case "ts":
    case "tsx":
      return "typescript";
    case "js":
    case "jsx":
    case "mjs":
    case "cjs":
      return "javascript";
    case "c":
    case "h":
      return "c";
    case "cpp":
    case "cc":
    case "cxx":
    case "hpp":
    case "hxx":
    case "hh":
      return "cpp";
    case "java":
      return "java";
    default:
      return null;
  }
}

/** 是否可能有内置 LSP server（与 Rust server_for 一致） */
export function hasLspServer(languageId: string | null): boolean {
  if (!languageId) return false;
  return languageId !== "java" && languageId !== "plaintext";
}
