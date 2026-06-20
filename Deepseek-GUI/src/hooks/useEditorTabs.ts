// 编辑器多标签状态：打开/关闭/切换文件，支持批量打开与批量关闭

import { useCallback, useState } from "react";

/** 多标签编辑器 Hook 返回值 */
export interface EditorTabsState {
  /** 已打开文件路径列表（标签顺序） */
  openFiles: string[];
  /** 当前激活的文件路径 */
  activeFile: string | null;
  /** 打开单个文件并激活 */
  openFile: (path: string) => void;
  /** 批量打开多个文件，激活最后一个 */
  openFilesBatch: (paths: string[]) => void;
  /** 关闭指定标签 */
  closeFile: (path: string) => void;
  /** 关闭全部标签 */
  closeAll: () => void;
  /** 关闭除指定文件外的其他标签 */
  closeOthers: (keep: string) => void;
  /** 关闭指定文件右侧的标签 */
  closeToRight: (anchor: string) => void;
  /** 关闭指定文件左侧的标签 */
  closeToLeft: (anchor: string) => void;
  /** 重命名已打开的文件标签路径 */
  renameFile: (from: string, to: string) => void;
  /** 仅切换激活标签 */
  setActiveFile: (path: string) => void;
}

/** 根据关闭集合计算下一个激活标签 */
function nextActiveAfterClose(prev: string[], toClose: Set<string>, cur: string | null): string | null {
  if (!cur || !toClose.has(cur)) return cur;
  const next = prev.filter((p) => !toClose.has(p));
  if (next.length === 0) return null;
  const oldIdx = prev.indexOf(cur);
  return next[Math.min(oldIdx, next.length - 1)] ?? next[0];
}

/**
 * 管理代码编辑器多标签状态。
 * 关闭当前激活标签时自动切到相邻标签。
 */
export function useEditorTabs(): EditorTabsState {
  const [openFiles, setOpenFiles] = useState<string[]>([]);
  const [activeFile, setActiveFile] = useState<string | null>(null);

  /** 打开单个文件：若已存在则仅激活 */
  const openFile = useCallback((path: string) => {
    setOpenFiles((prev) => (prev.includes(path) ? prev : [...prev, path]));
    setActiveFile(path);
  }, []);

  /** 批量打开：去重追加，激活列表中最后一个 */
  const openFilesBatch = useCallback((paths: string[]) => {
    if (paths.length === 0) return;
    setOpenFiles((prev) => {
      const next = [...prev];
      for (const p of paths) {
        if (!next.includes(p)) next.push(p);
      }
      return next;
    });
    setActiveFile(paths[paths.length - 1]);
  }, []);

  /** 按路径集合批量关闭 */
  const closePaths = useCallback((toClose: Set<string>) => {
    if (toClose.size === 0) return;
    setOpenFiles((prev) => {
      const next = prev.filter((p) => !toClose.has(p));
      setActiveFile((cur) => nextActiveAfterClose(prev, toClose, cur));
      return next;
    });
  }, []);

  /** 关闭标签；若关闭的是当前激活项则切到相邻标签 */
  const closeFile = useCallback(
    (path: string) => {
      closePaths(new Set([path]));
    },
    [closePaths],
  );

  /** 清空所有打开的文件 */
  const closeAll = useCallback(() => {
    setOpenFiles([]);
    setActiveFile(null);
  }, []);

  /** 关闭除 keep 外的所有标签 */
  const closeOthers = useCallback(
    (keep: string) => {
      setOpenFiles((prev) => {
        const toClose = new Set(prev.filter((p) => p !== keep));
        if (toClose.size === 0) return prev;
        const next = [keep];
        setActiveFile(keep);
        return next;
      });
    },
    [],
  );

  /** 关闭 anchor 右侧的标签 */
  const closeToRight = useCallback(
    (anchor: string) => {
      setOpenFiles((prev) => {
        const idx = prev.indexOf(anchor);
        if (idx < 0 || idx >= prev.length - 1) return prev;
        const toClose = new Set(prev.slice(idx + 1));
        const next = prev.filter((p) => !toClose.has(p));
        setActiveFile((cur) => nextActiveAfterClose(prev, toClose, cur));
        return next;
      });
    },
    [],
  );

  /** 关闭 anchor 左侧的标签 */
  const closeToLeft = useCallback(
    (anchor: string) => {
      setOpenFiles((prev) => {
        const idx = prev.indexOf(anchor);
        if (idx <= 0) return prev;
        const toClose = new Set(prev.slice(0, idx));
        const next = prev.filter((p) => !toClose.has(p));
        setActiveFile((cur) => nextActiveAfterClose(prev, toClose, cur) ?? anchor);
        return next;
      });
    },
    [],
  );

  /** 同步重命名后的标签路径 */
  const renameFile = useCallback((from: string, to: string) => {
    setOpenFiles((prev) => prev.map((p) => (p === from ? to : p)));
    setActiveFile((cur) => (cur === from ? to : cur));
  }, []);

  return {
    openFiles,
    activeFile,
    openFile,
    openFilesBatch,
    closeFile,
    closeAll,
    closeOthers,
    closeToRight,
    closeToLeft,
    renameFile,
    setActiveFile,
  };
}
