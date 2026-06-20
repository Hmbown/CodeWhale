// Cursor Tab 风格内联补全：设置持久化（localStorage）

/** Tab 补全配置 */
export interface TabCompletionSettings {
  /** 主开关：上下文感知多行补全 */
  enabled: boolean;
  /** Ctrl+Right 逐词接受 */
  partialAccept: boolean;
  /** 注释区域内也触发 */
  suggestInComments: boolean;
  /** 允许仅空白/换行类建议 */
  whitespaceOnly: boolean;
  /** 自动 import（TypeScript，预留） */
  autoImportTs: boolean;
  /** 自动 import（Python Beta，预留） */
  autoImportPy: boolean;
  /** 忽略文件 glob，逗号分隔 */
  ignoredGlobs: string;
}

export const TAB_SETTINGS_STORAGE_KEY = "ds_tab_completion_settings";
export const TAB_SETTINGS_CHANGE_EVENT = "ds-tab-settings-change";

const DEFAULTS: TabCompletionSettings = {
  enabled: true,
  partialAccept: false,
  suggestInComments: true,
  whitespaceOnly: false,
  autoImportTs: true,
  autoImportPy: false,
  ignoredGlobs: "*.md, **/generated/**",
};

/** 读取 Tab 补全设置 */
export function loadTabCompletionSettings(): TabCompletionSettings {
  try {
    const raw = localStorage.getItem(TAB_SETTINGS_STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<TabCompletionSettings>;
    return { ...DEFAULTS, ...parsed };
  } catch {
    return { ...DEFAULTS };
  }
}

/** 保存并广播变更 */
export function saveTabCompletionSettings(next: TabCompletionSettings): void {
  localStorage.setItem(TAB_SETTINGS_STORAGE_KEY, JSON.stringify(next));
  window.dispatchEvent(new CustomEvent(TAB_SETTINGS_CHANGE_EVENT));
}

/** 简单 glob 匹配（*, **, ?） */
export function pathMatchesIgnoredGlobs(filePath: string, globs: string): boolean {
  const patterns = globs
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  if (patterns.length === 0) return false;
  const normalized = filePath.replace(/\\/g, "/");
  const base = normalized.split("/").pop() ?? normalized;
  return patterns.some((pat) => simpleGlobMatch(normalized, base, pat));
}

/** 单条 glob 匹配 */
function simpleGlobMatch(fullPath: string, baseName: string, pattern: string): boolean {
  const p = pattern.replace(/\\/g, "/");
  if (p.includes("/")) {
    return globToRegExp(p).test(fullPath);
  }
  return globToRegExp(p).test(baseName);
}

/** glob → RegExp（支持 * ? **） */
function globToRegExp(glob: string): RegExp {
  let re = "^";
  let i = 0;
  while (i < glob.length) {
    if (glob[i] === "*" && glob[i + 1] === "*") {
      re += ".*";
      i += 2;
      if (glob[i] === "/") i += 1;
    } else if (glob[i] === "*") {
      re += "[^/]*";
      i += 1;
    } else if (glob[i] === "?") {
      re += ".";
      i += 1;
    } else {
      re += glob[i]!.replace(/[.+^${}()|[\]\\]/g, "\\$&");
      i += 1;
    }
  }
  re += "$";
  return new RegExp(re, "i");
}

/** 光标是否在注释区（启发式） */
export function cursorInComment(lineText: string, col: number): boolean {
  const before = lineText.slice(0, col);
  if (before.includes("//")) return true;
  if (/^\s*#/.test(lineText) && !before.includes('"') && !before.includes("'")) return true;
  if (/^\s*\*/.test(lineText.trimStart()) || before.trimStart().startsWith("*")) return true;
  return false;
}
