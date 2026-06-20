// 多标签代码编辑器容器：标签栏 + 批量关闭菜单 + CodeView

import { useCallback, useEffect, useRef, useState } from "react";
import { CodeView } from "./CodeView";
import { EditorEmptyState } from "./EditorEmptyState";
import type { Locale } from "../i18n";

interface EditorPanelProps {
  /** 已打开的文件绝对路径列表 */
  openFiles: string[];
  /** 当前激活的文件 */
  activeFile: string | null;
  /** 切换激活标签 */
  onSelectFile: (path: string) => void;
  /** 关闭指定标签 */
  onCloseFile: (path: string) => void;
  /** 关闭除指定文件外的其他标签 */
  onCloseOthers: (keep: string) => void;
  /** 关闭指定文件右侧的标签 */
  onCloseToRight: (anchor: string) => void;
  /** 关闭指定文件左侧的标签 */
  onCloseToLeft: (anchor: string) => void;
  /** 关闭全部标签 */
  onCloseAll: () => void;
  locale: Locale;
  hasFolder: boolean;
  onOpenFolder: () => void;
  onQuickOpen: () => void;
  onCommandPalette: () => void;
  onSearchChats: () => void;
  onOpenSettings: () => void;
  onNewChat: () => void;
  /** 工作区根目录（LSP IntelliSense） */
  workspaceRoot?: string | null;
}

/** 标签关闭菜单项 */
type CloseAction = "close" | "others" | "right" | "left" | "all";

/** 从绝对路径取文件名 */
function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/** 若目标含未保存文件则确认 */
function confirmClose(paths: string[], dirtyMap: Record<string, boolean>, action: string): boolean {
  const dirtyCount = paths.filter((p) => dirtyMap[p]).length;
  if (dirtyCount === 0) return true;
  return window.confirm(`${action}：有 ${dirtyCount} 个文件未保存，确定关闭吗？`);
}

/** 标签右键 / 更多菜单 */
function TabCloseMenu({
  anchor,
  openFiles,
  onPick,
  onDismiss,
}: {
  anchor: string;
  openFiles: string[];
  onPick: (action: CloseAction) => void;
  onDismiss: () => void;
}) {
  const idx = openFiles.indexOf(anchor);
  const hasLeft = idx > 0;
  const hasRight = idx >= 0 && idx < openFiles.length - 1;
  const hasOthers = openFiles.length > 1;

  const items: Array<{ action: CloseAction; label: string; disabled?: boolean }> = [
    { action: "close", label: "关闭" },
    { action: "others", label: "关闭其他", disabled: !hasOthers },
    { action: "right", label: "关闭右侧", disabled: !hasRight },
    { action: "left", label: "关闭左侧", disabled: !hasLeft },
    { action: "all", label: "关闭全部", disabled: openFiles.length === 0 },
  ];

  return (
    <div
      className="editor-tab-menu"
      onMouseDown={(e) => e.stopPropagation()}
      onMouseLeave={onDismiss}
    >
      {items.map((it) => (
        <button
          key={it.action}
          type="button"
          className="editor-tab-menu-item"
          disabled={it.disabled}
          onClick={() => onPick(it.action)}
        >
          {it.label}
        </button>
      ))}
    </div>
  );
}

export function EditorPanel({
  openFiles,
  activeFile,
  onSelectFile,
  onCloseFile,
  onCloseOthers,
  onCloseToRight,
  onCloseToLeft,
  onCloseAll,
  locale,
  hasFolder,
  onOpenFolder,
  onQuickOpen,
  onCommandPalette,
  onSearchChats,
  onOpenSettings,
  onNewChat,
  workspaceRoot = null,
}: EditorPanelProps) {
  const [dirtyMap, setDirtyMap] = useState<Record<string, boolean>>({});
  const [menuAnchor, setMenuAnchor] = useState<string | null>(null);
  const [moreOpen, setMoreOpen] = useState(false);
  const moreRef = useRef<HTMLDivElement>(null);
  /** 标签右键菜单容器：须排除在「点击外部关闭」之外，否则 mousedown 会先销毁菜单导致点击无效 */
  const menuFloatRef = useRef<HTMLDivElement>(null);

  /** 子编辑器 dirty 变化回调 */
  const onDirtyChange = useCallback((path: string, dirty: boolean) => {
    setDirtyMap((prev) => (prev[path] === dirty ? prev : { ...prev, [path]: dirty }));
  }, []);

  /** 点击外部关闭菜单（排除 ⋯ 下拉与标签右键浮层，避免 mousedown 抢在 click 前关掉菜单） */
  useEffect(() => {
    if (!menuAnchor && !moreOpen) return;
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Node;
      if (moreRef.current?.contains(t)) return;
      if (menuFloatRef.current?.contains(t)) return;
      setMenuAnchor(null);
      setMoreOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menuAnchor, moreOpen]);

  /** 收集将要关闭的路径 */
  const pathsForAction = useCallback(
    (action: CloseAction, anchor: string): string[] => {
      const idx = openFiles.indexOf(anchor);
      switch (action) {
        case "close":
          return [anchor];
        case "others":
          return openFiles.filter((p) => p !== anchor);
        case "right":
          return idx >= 0 ? openFiles.slice(idx + 1) : [];
        case "left":
          return idx > 0 ? openFiles.slice(0, idx) : [];
        case "all":
          return [...openFiles];
        default:
          return [];
      }
    },
    [openFiles],
  );

  /** 执行批量关闭（含未保存确认） */
  const runCloseAction = useCallback(
    (action: CloseAction, anchor: string) => {
      const labels: Record<CloseAction, string> = {
        close: "关闭",
        others: "关闭其他",
        right: "关闭右侧",
        left: "关闭左侧",
        all: "关闭全部",
      };
      const targets = pathsForAction(action, anchor);
      if (targets.length === 0) {
        setMenuAnchor(null);
        setMoreOpen(false);
        return;
      }
      if (!confirmClose(targets, dirtyMap, labels[action])) return;
      switch (action) {
        case "close":
          onCloseFile(anchor);
          break;
        case "others":
          onCloseOthers(anchor);
          break;
        case "right":
          onCloseToRight(anchor);
          break;
        case "left":
          onCloseToLeft(anchor);
          break;
        case "all":
          onCloseAll();
          break;
      }
      setMenuAnchor(null);
      setMoreOpen(false);
      setDirtyMap((prev) => {
        const next = { ...prev };
        for (const p of targets) delete next[p];
        return next;
      });
    },
    [
      dirtyMap,
      onCloseAll,
      onCloseFile,
      onCloseToLeft,
      onCloseOthers,
      onCloseToRight,
      pathsForAction,
    ],
  );

  if (openFiles.length === 0) {
    return (
      <EditorEmptyState
        locale={locale}
        hasFolder={hasFolder}
        onOpenFolder={onOpenFolder}
        onQuickOpen={onQuickOpen}
        onCommandPalette={onCommandPalette}
        onSearchChats={onSearchChats}
        onOpenSettings={onOpenSettings}
        onNewChat={onNewChat}
      />
    );
  }

  return (
    <div className="editor-panel">
      <div className="editor-tabs-wrap">
        <div className="editor-tabs">
          {openFiles.map((path) => {
            const active = path === activeFile;
            const dirty = dirtyMap[path];
            return (
              <div
                key={path}
                className={`editor-tab${active ? " active" : ""}${dirty ? " dirty" : ""}`}
                onClick={() => onSelectFile(path)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  onSelectFile(path);
                  setMenuAnchor(path);
                  setMoreOpen(false);
                }}
                title={path}
              >
                {dirty && <span className="code-dirty" title="未保存">●</span>}
                <span className="editor-tab-name">{baseName(path)}</span>
                <button
                  type="button"
                  className="editor-tab-close"
                  title="关闭"
                  onClick={(e) => {
                    e.stopPropagation();
                    runCloseAction("close", path);
                  }}
                >
                  ×
                </button>
              </div>
            );
          })}
        </div>

        {/* 更多：对当前标签批量关闭（仿 Cursor / VS Code） */}
        <div className="editor-tabs-more-wrap" ref={moreRef}>
          <button
            type="button"
            className={`editor-tabs-more${moreOpen ? " active" : ""}`}
            title="关闭选项"
            onClick={() => {
              setMoreOpen((v) => !v);
              setMenuAnchor(null);
            }}
          >
            ⋯
          </button>
          {moreOpen && activeFile && (
            <TabCloseMenu
              anchor={activeFile}
              openFiles={openFiles}
              onPick={(action) => runCloseAction(action, activeFile)}
              onDismiss={() => setMoreOpen(false)}
            />
          )}
        </div>
      </div>

      {/* 右键菜单（固定定位在标签栏下方） */}
      {menuAnchor && (
        <div className="editor-tab-menu-float" ref={menuFloatRef}>
          <TabCloseMenu
            anchor={menuAnchor}
            openFiles={openFiles}
            onPick={(action) => runCloseAction(action, menuAnchor)}
            onDismiss={() => setMenuAnchor(null)}
          />
        </div>
      )}

      <div className="editor-body">
        {openFiles.map((path) => (
          <CodeView
            key={path}
            path={path}
            workspaceRoot={workspaceRoot}
            visible={path === activeFile}
            embedded
            onDirtyChange={(d) => onDirtyChange(path, d)}
            onClose={() => runCloseAction("close", path)}
          />
        ))}
      </div>
    </div>
  );
}
