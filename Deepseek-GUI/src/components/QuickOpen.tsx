// 快速打开文件（Ctrl+P）：递归索引工作区文件并模糊搜索

import { useCallback, useEffect, useMemo, useState } from "react";
import { listDir } from "../api/tauri";

interface QuickOpenProps {
  rootPath: string | null;
  onOpen: (path: string) => void;
  onClose: () => void;
}

/** 索引条目 */
interface FileEntry {
  path: string;
  rel: string;
  name: string;
}

/** 跳过的目录名（减少索引体积） */
const SKIP_DIRS = new Set([
  "node_modules",
  "target",
  ".git",
  "dist",
  "build",
  ".deepseek",
  "__pycache__",
]);

/**
 * 递归收集工作区内的文件（有数量上限，避免超大仓库卡死）。
 */
async function collectFiles(root: string, max = 800): Promise<FileEntry[]> {
  const out: FileEntry[] = [];
  const sep = root.includes("\\") ? "\\" : "/";

  async function walk(dir: string, depth: number): Promise<void> {
    if (out.length >= max || depth > 10) return;
    let entries;
    try {
      entries = await listDir(dir);
    } catch {
      return;
    }
    for (const e of entries) {
      if (out.length >= max) break;
      if (e.name.startsWith(".")) continue;
      if (e.is_dir) {
        if (SKIP_DIRS.has(e.name)) continue;
        await walk(e.path, depth + 1);
      } else {
        const rel = e.path.startsWith(root)
          ? e.path.slice(root.length).replace(/^[\\/]+/, "")
          : e.name;
        out.push({ path: e.path, rel: rel.replace(/\\/g, sep), name: e.name });
      }
    }
  }

  await walk(root, 0);
  return out.sort((a, b) => a.rel.localeCompare(b.rel));
}

/** 简单模糊匹配：query 各字符按序出现在 target 中 */
function fuzzyMatch(target: string, query: string): boolean {
  const t = target.toLowerCase();
  const q = query.toLowerCase().trim();
  if (!q) return true;
  let ti = 0;
  for (const ch of q) {
    ti = t.indexOf(ch, ti);
    if (ti < 0) return false;
    ti += 1;
  }
  return true;
}

export function QuickOpen({ rootPath, onOpen, onClose }: QuickOpenProps) {
  const [query, setQuery] = useState("");
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [idx, setIdx] = useState(0);

  // 打开时索引文件
  useEffect(() => {
    if (!rootPath) return;
    let alive = true;
    setLoading(true);
    collectFiles(rootPath)
      .then((f) => alive && setFiles(f))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [rootPath]);

  const filtered = useMemo(() => {
    if (!query.trim()) return files.slice(0, 50);
    return files.filter((f) => fuzzyMatch(f.rel, query)).slice(0, 50);
  }, [files, query]);

  useEffect(() => {
    setIdx(0);
  }, [query]);

  /** 键盘导航与确认 */
  const onKey = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setIdx((i) => Math.min(i + 1, filtered.length - 1));
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setIdx((i) => Math.max(i - 1, 0));
        return;
      }
      if (e.key === "Enter" && filtered[idx]) {
        e.preventDefault();
        onOpen(filtered[idx].path);
        onClose();
      }
    },
    [filtered, idx, onOpen, onClose],
  );

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="quick-open" onClick={(e) => e.stopPropagation()}>
        <input
          className="quick-open-input"
          placeholder="输入文件名快速打开… (Ctrl+P)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKey}
          autoFocus
        />
        <div className="quick-open-list">
          {loading && <div className="quick-open-empty">索引中…</div>}
          {!loading && !rootPath && (
            <div className="quick-open-empty">请先打开项目文件夹</div>
          )}
          {!loading && rootPath && filtered.length === 0 && (
            <div className="quick-open-empty">无匹配文件</div>
          )}
          {filtered.map((f, i) => (
            <button
              key={f.path}
              type="button"
              className={`quick-open-item${i === idx ? " active" : ""}`}
              onClick={() => {
                onOpen(f.path);
                onClose();
              }}
            >
              <span className="quick-open-name">{f.name}</span>
              <span className="quick-open-rel">{f.rel}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
