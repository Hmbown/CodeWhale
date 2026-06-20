// 顶部菜单栏（仿 Cursor / VS Code）：File / Edit / View 等，仅实现常用项

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { t, type Locale } from "../i18n";
import type { SettingsTab } from "./SettingsView";
import { WindowControls } from "./WindowControls";
import { PanelToggleButton } from "./PanelToggleButton";
import { isTauri } from "../api/tauri";
import {
  editUndo,
  editRedo,
  editCut,
  editCopy,
  editPaste,
  editSelectAll,
  editExpandSelection,
  editFind,
} from "../utils/editorCommands";

/** 单个菜单项 */
interface MenuItemDef {
  id: string;
  labelZh: string;
  labelEn: string;
  shortcut?: string;
  disabled?: boolean;
  /** 分隔线（无点击行为） */
  separator?: boolean;
  action?: () => void;
}

/** 菜单组 */
interface MenuDef {
  id: string;
  labelZh: string;
  labelEn: string;
  items: MenuItemDef[];
}

interface TitleMenuBarProps {
  locale: Locale;
  sidebarOpen: boolean;
  hasFolder: boolean;
  /** 设置页是否已打开（顶栏按钮高亮） */
  settingsOpen: boolean;
  onOpenFolder: () => void;
  onQuickOpen: () => void;
  onCommandPalette: () => void;
  onSearchChats: () => void;
  onOpenSettings: (tab?: SettingsTab) => void;
  /** 切换设置页显示 */
  onToggleSettings: () => void;
  onToggleSidebar: () => void;
  /** 切换右侧聊天面板 */
  onToggleChat: () => void;
  /** 右侧聊天是否展开（按钮高亮） */
  chatOpen: boolean;
  onNewChat: () => void;
  onShowDiff: () => void;
  onToggleSessions: () => void;
  /** 切换底部集成终端 */
  onToggleTerminal: () => void;
  /** 全局界面缩放 */
  onZoomIn: () => void;
  onZoomOut: () => void;
  onZoomReset: () => void;
}

/** 根据语言取菜单文案 */
function label(locale: Locale, zh: string, en: string): string {
  return locale === "zh" ? zh : en;
}

export function TitleMenuBar({
  locale,
  sidebarOpen,
  hasFolder,
  onOpenFolder,
  onQuickOpen,
  onCommandPalette,
  onSearchChats,
  onOpenSettings,
  onToggleSettings,
  onToggleSidebar,
  onToggleChat,
  chatOpen,
  onNewChat,
  onShowDiff,
  onToggleSessions,
  onToggleTerminal,
  settingsOpen,
  onZoomIn,
  onZoomOut,
  onZoomReset,
}: TitleMenuBarProps) {
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  /** 当前下拉的屏幕定位（portal 到 body，避免被顶栏 overflow:hidden 裁剪） */
  const [menuPos, setMenuPos] = useState<{ left: number; top: number } | null>(null);
  const barRef = useRef<HTMLElement>(null);
  const leftRef = useRef<HTMLDivElement>(null);
  const navRef = useRef<HTMLElement>(null);
  /** 各菜单触发按钮的 DOM 引用，用于计算下拉定位 */
  const triggerRefs = useRef<Map<string, HTMLButtonElement>>(new Map());
  /** 各菜单组宽度缓存（用于缩放/变窗时计算可见数量） */
  const menuWidthsRef = useRef<number[]>([]);
  /** 左侧能完整显示的菜单数量（右侧放不下的如「终端」「帮助」隐藏） */
  const [visibleMenuCount, setVisibleMenuCount] = useState(99);

  /** 点击外部关闭下拉菜单（下拉已 portal 到 body，需额外放行其内部点击） */
  useEffect(() => {
    if (!openMenu) return;
    const onDoc = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      if (barRef.current?.contains(target)) return;
      if (target.closest?.(".title-menu-dropdown")) return;
      setOpenMenu(null);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [openMenu]);

  /** 打开某个菜单并按其触发按钮位置计算下拉坐标 */
  const openMenuById = useCallback((id: string) => {
    const el = triggerRefs.current.get(id);
    if (el) {
      const r = el.getBoundingClientRect();
      // 防止下拉超出右边界
      const left = Math.min(r.left, window.innerWidth - 248);
      setMenuPos({ left: Math.max(4, left), top: r.bottom + 1 });
    }
    setOpenMenu(id);
  }, []);

  /** 滚动/缩放/窗口变化时关闭下拉，避免定位错位 */
  useEffect(() => {
    if (!openMenu) return;
    const close = () => setOpenMenu(null);
    window.addEventListener("scroll", close, true);
    window.addEventListener("ds-ui-zoom", close);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("ds-ui-zoom", close);
    };
  }, [openMenu]);

  /** 执行菜单项并关闭下拉 */
  const run = useCallback((item: MenuItemDef) => {
    if (item.disabled || item.separator || !item.action) return;
    item.action();
    setOpenMenu(null);
  }, []);

  const menus: MenuDef[] = [
    {
      id: "file",
      labelZh: "文件",
      labelEn: "File",
      items: [
        {
          id: "open-folder",
          labelZh: "打开文件夹…",
          labelEn: "Open Folder…",
          action: onOpenFolder,
        },
        {
          id: "quick-open",
          labelZh: "快速打开文件",
          labelEn: "Quick Open File",
          shortcut: "Ctrl+P",
          disabled: !hasFolder,
          action: onQuickOpen,
        },
        {
          id: "save",
          labelZh: "保存",
          labelEn: "Save",
          shortcut: "Ctrl+S",
          // 通知当前可见编辑器保存（CodeView 监听 ds-editor-save）
          action: () => window.dispatchEvent(new Event("ds-editor-save")),
        },
        { id: "sep1", labelZh: "", labelEn: "", separator: true },
        {
          id: "new-chat",
          labelZh: "新建会话",
          labelEn: "New Chat",
          action: onNewChat,
        },
        { id: "sep2", labelZh: "", labelEn: "", separator: true },
        {
          id: "exit",
          labelZh: "退出",
          labelEn: "Exit",
          disabled: !isTauri(),
          action: () => {
            void getCurrentWindow().close();
          },
        },
      ],
    },
    {
      id: "edit",
      labelZh: "编辑",
      labelEn: "Edit",
      items: [
        {
          id: "palette",
          labelZh: "命令面板",
          labelEn: "Command Palette",
          shortcut: "Ctrl+K",
          action: onCommandPalette,
        },
        { id: "sep1", labelZh: "", labelEn: "", separator: true },
        {
          id: "undo",
          labelZh: "撤销",
          labelEn: "Undo",
          shortcut: "Ctrl+Z",
          action: editUndo,
        },
        {
          id: "redo",
          labelZh: "重做",
          labelEn: "Redo",
          shortcut: "Ctrl+Y",
          action: editRedo,
        },
        { id: "sep2", labelZh: "", labelEn: "", separator: true },
        {
          id: "cut",
          labelZh: "剪切",
          labelEn: "Cut",
          shortcut: "Ctrl+X",
          action: editCut,
        },
        {
          id: "copy",
          labelZh: "复制",
          labelEn: "Copy",
          shortcut: "Ctrl+C",
          action: editCopy,
        },
        {
          id: "paste",
          labelZh: "粘贴",
          labelEn: "Paste",
          shortcut: "Ctrl+V",
          action: () => void editPaste(),
        },
        { id: "sep3", labelZh: "", labelEn: "", separator: true },
        {
          id: "find",
          labelZh: "查找",
          labelEn: "Find",
          shortcut: "Ctrl+F",
          // 在当前代码编辑器中打开搜索面板；无编辑器焦点时提示
          action: () => {
            if (!editFind()) {
              alert(
                locale === "zh"
                  ? "请先点选一个代码编辑器再查找。"
                  : "Focus a code editor first to search.",
              );
            }
          },
        },
      ],
    },
    {
      id: "selection",
      labelZh: "选择",
      labelEn: "Selection",
      items: [
        {
          id: "select-all",
          labelZh: "全选",
          labelEn: "Select All",
          shortcut: "Ctrl+A",
          action: editSelectAll,
        },
        {
          id: "expand",
          labelZh: "展开选择",
          labelEn: "Expand Selection",
          shortcut: "Shift+Alt+→",
          action: editExpandSelection,
        },
      ],
    },
    {
      id: "view",
      labelZh: "视图",
      labelEn: "View",
      items: [
        {
          id: "explorer",
          labelZh: sidebarOpen ? "隐藏资源管理器" : "显示资源管理器",
          labelEn: sidebarOpen ? "Hide Explorer" : "Show Explorer",
          shortcut: "Ctrl+B",
          action: onToggleSidebar,
        },
        {
          id: "chat-panel",
          labelZh: chatOpen ? "隐藏聊天面板" : "显示聊天面板",
          labelEn: chatOpen ? "Hide Chat Panel" : "Show Chat Panel",
          action: onToggleChat,
        },
        {
          id: "settings",
          labelZh: "设置",
          labelEn: "Settings",
          shortcut: "Ctrl+,",
          action: () => onOpenSettings("models"),
        },
        { id: "sep1", labelZh: "", labelEn: "", separator: true },
        {
          id: "sessions",
          labelZh: "历史会话",
          labelEn: "Chat History",
          action: onToggleSessions,
        },
        {
          id: "terminal",
          labelZh: "集成终端",
          labelEn: "Integrated Terminal",
          shortcut: "Ctrl+`",
          action: onToggleTerminal,
        },
        { id: "sep2", labelZh: "", labelEn: "", separator: true },
        {
          id: "zoom-in",
          labelZh: "放大",
          labelEn: "Zoom In",
          shortcut: "Ctrl+=",
          action: onZoomIn,
        },
        {
          id: "zoom-out",
          labelZh: "缩小",
          labelEn: "Zoom Out",
          shortcut: "Ctrl+-",
          action: onZoomOut,
        },
        {
          id: "zoom-reset",
          labelZh: "重置缩放",
          labelEn: "Reset Zoom",
          shortcut: "Ctrl+0",
          action: onZoomReset,
        },
      ],
    },
    {
      id: "go",
      labelZh: "转到",
      labelEn: "Go",
      items: [
        {
          id: "quick-open",
          labelZh: "转到文件…",
          labelEn: "Go to File…",
          shortcut: "Ctrl+P",
          disabled: !hasFolder,
          action: onQuickOpen,
        },
        {
          id: "search-chats",
          labelZh: "搜索会话…",
          labelEn: "Search Chats…",
          shortcut: "Ctrl+Shift+P",
          action: onSearchChats,
        },
      ],
    },
    {
      id: "run",
      labelZh: "运行",
      labelEn: "Run",
      items: [
        {
          id: "tasks",
          labelZh: "任务与自动化",
          labelEn: "Tasks",
          action: () => onOpenSettings("tasks"),
        },
        {
          id: "diff",
          labelZh: "查看工作区变更",
          labelEn: "View Workspace Diff",
          disabled: !hasFolder,
          action: onShowDiff,
        },
      ],
    },
    {
      id: "terminal",
      labelZh: "终端",
      labelEn: "Terminal",
      items: [
        {
          id: "new-terminal",
          labelZh: "新建终端",
          labelEn: "New Terminal",
          shortcut: "Ctrl+`",
          // 打开底部集成终端（PTY + xterm）
          action: onToggleTerminal,
        },
      ],
    },
    {
      id: "help",
      labelZh: "帮助",
      labelEn: "Help",
      items: [
        {
          id: "docs",
          labelZh: "文档与快捷键",
          labelEn: "Docs & Shortcuts",
          action: onCommandPalette,
        },
        {
          id: "about",
          labelZh: "关于 DeepSeek GUI",
          labelEn: "About DeepSeek GUI",
          action: () => {
            alert(
              locale === "zh"
                ? "DeepSeek GUI v0.1.0\n基于 Tauri + React，连接 deepseek serve 运行时。"
                : "DeepSeek GUI v0.1.0\nTauri + React desktop shell for deepseek serve.",
            );
          },
        },
      ],
    },
  ];

  /** 根据左侧可用宽度，从右向左隐藏放不下的菜单（避免与搜索框重叠；不占用侧栏开关宽度） */
  const updateVisibleMenuCount = useCallback(() => {
    const left = leftRef.current;
    const nav = navRef.current;
    if (!left || !nav || menuWidthsRef.current.length === 0) return;

    const brand = left.querySelector(".title-menu-brand") as HTMLElement | null;
    const brandW = brand?.offsetWidth ?? 30;
    // 与中央搜索条留白，缩放时右侧菜单（终端/帮助）优先隐藏
    const gapBeforeSearch = 16;
    const avail = Math.max(0, left.clientWidth - brandW - gapBeforeSearch);

    let used = 0;
    let count = 0;
    for (const w of menuWidthsRef.current) {
      if (count === 0 || used + w <= avail) {
        used += w;
        count += 1;
      } else {
        break;
      }
    }
    setVisibleMenuCount(Math.max(1, count));
  }, []);

  /** 语言切换后重新测量各菜单宽度 */
  useLayoutEffect(() => {
    const groups = navRef.current?.querySelectorAll<HTMLElement>(".title-menu-group");
    if (!groups?.length) return;
    // 测量时临时显示全部菜单，避免 is-overflow-hidden 导致宽度为 0
    for (const g of groups) {
      g.style.display = "flex";
    }
    menuWidthsRef.current = Array.from(groups).map((g) => g.getBoundingClientRect().width);
    for (const g of groups) {
      g.style.display = "";
    }
    updateVisibleMenuCount();
  }, [locale, menus.length, updateVisibleMenuCount]);

  /** 窗口缩放 / 顶栏宽度变化时重算可见菜单 */
  useEffect(() => {
    const left = leftRef.current;
    const bar = barRef.current;
    if (!left) return;

    updateVisibleMenuCount();
    const ro = new ResizeObserver(() => updateVisibleMenuCount());
    ro.observe(left);
    if (bar) ro.observe(bar);

    const onResize = () => updateVisibleMenuCount();
    window.addEventListener("resize", onResize);
    // 全局 UI 缩放（Ctrl+滚轮）后 layout 宽度变化，需同步隐藏溢出菜单
    window.addEventListener("ds-ui-zoom", onResize);

    return () => {
      ro.disconnect();
      window.removeEventListener("resize", onResize);
      window.removeEventListener("ds-ui-zoom", onResize);
    };
  }, [updateVisibleMenuCount]);

  /** 当前打开的菜单被隐藏时自动关闭下拉 */
  useEffect(() => {
    if (!openMenu) return;
    const idx = menus.findIndex((m) => m.id === openMenu);
    if (idx >= visibleMenuCount) setOpenMenu(null);
  }, [visibleMenuCount, openMenu, menus]);

  return (
    <header className="title-menu-bar" ref={barRef} data-tauri-drag-region="false">
      {/* 左侧栏开关：独立固定列，缩放时不被 overflow 裁切（仿 Cursor） */}
      <div className="title-menu-rail-left" data-tauri-drag-region="false">
        <PanelToggleButton
          side="left"
          open={sidebarOpen}
          locale={locale}
          onClick={onToggleSidebar}
          className="title-menu-panel-toggle"
        />
      </div>

      <div className="title-menu-left" ref={leftRef} data-tauri-drag-region="false">
        <div className="title-menu-brand" title="DeepSeek GUI">
          <svg viewBox="0 0 24 24" aria-hidden className="title-menu-logo">
            <path d="M12 2L2 7l10 5 10-5-10-5zm0 7L2 14l10 5 10-5-10-5z" />
          </svg>
        </div>

        <nav
          className="title-menu-nav"
          ref={navRef}
          aria-label={label(locale, "主菜单", "Main menu")}
        >
          {menus.map((menu, index) => (
            <div
              key={menu.id}
              className={`title-menu-group${index >= visibleMenuCount ? " is-overflow-hidden" : ""}`}
            >
              <button
                type="button"
                ref={(el) => {
                  if (el) triggerRefs.current.set(menu.id, el);
                  else triggerRefs.current.delete(menu.id);
                }}
                className={`title-menu-trigger${openMenu === menu.id ? " active" : ""}`}
                onClick={() => (openMenu === menu.id ? setOpenMenu(null) : openMenuById(menu.id))}
                onMouseEnter={() => {
                  if (openMenu) openMenuById(menu.id);
                }}
              >
                {label(locale, menu.labelZh, menu.labelEn)}
              </button>
            </div>
          ))}
        </nav>
      </div>

      {/* 下拉菜单 portal 到 body，避免被顶栏 overflow:hidden 裁剪 */}
      {openMenu &&
        menuPos &&
        createPortal(
          <div
            className="title-menu-dropdown"
            role="menu"
            style={{ position: "fixed", left: menuPos.left, top: menuPos.top }}
          >
            {(menus.find((m) => m.id === openMenu)?.items ?? []).map((item) =>
              item.separator ? (
                <div key={item.id} className="title-menu-sep" role="separator" />
              ) : (
                <button
                  key={item.id}
                  type="button"
                  role="menuitem"
                  className="title-menu-item"
                  disabled={item.disabled}
                  onClick={() => run(item)}
                >
                  <span>{label(locale, item.labelZh, item.labelEn)}</span>
                  {item.shortcut && <kbd className="title-menu-kbd">{item.shortcut}</kbd>}
                </button>
              ),
            )}
          </div>,
          document.body,
        )}

      {/* 中央搜索条（三列网格居中，仿 Cursor） */}
      <div className="title-menu-center" data-tauri-drag-region="false">
        <button
          type="button"
          className="title-menu-search"
          onClick={onCommandPalette}
          title={t("palette.quickOpen", locale)}
        >
          <span className="title-menu-search-icon">⌕</span>
          <span className="title-menu-search-ph">
            {locale === "zh" ? "搜索或运行命令…" : "Search or run a command…"}
          </span>
          <kbd className="title-menu-search-kbd">Ctrl+K</kbd>
        </button>
      </div>

      <div className="title-menu-right" data-tauri-drag-region="false">
        {/* 空白拖拽区：固定最小宽度，避免挤掉右侧开关与窗口控件 */}
        {isTauri() && (
          <div
            className="title-menu-drag"
            data-tauri-drag-region
            title={locale === "zh" ? "拖动窗口 · 双击最大化" : "Drag window · double-click to maximize"}
            onDoubleClick={() => {
              void getCurrentWindow().toggleMaximize();
            }}
          />
        )}

        <div className="title-menu-right-controls" data-tauri-drag-region="false">
          <PanelToggleButton
            side="right"
            open={chatOpen}
            locale={locale}
            onClick={onToggleChat}
            className="title-menu-panel-toggle"
          />

          <button
            type="button"
            className={`title-menu-icon${settingsOpen ? " active" : ""}`}
            onClick={onToggleSettings}
            title={t("status.settings", locale)}
            aria-label={t("status.settings", locale)}
          >
            <svg viewBox="0 0 24 24" aria-hidden className="title-menu-icon-svg">
              <path d="M12 8a4 4 0 1 0 .001 8.001A4 4 0 0 0 12 8zm8.94 3a7.96 7.96 0 0 1 .06.94 7.96 7.96 0 0 1-.06.94l2.03 1.58a.75.75 0 0 1 .18.96l-1.92 3.32a.75.75 0 0 1-.91.33l-2.4-.96a7.12 7.12 0 0 1-1.62.94l-.36 2.54a.75.75 0 0 1-.74.64h-3.84a.75.75 0 0 1-.74-.64l-.36-2.54a7.12 7.12 0 0 1-1.62-.94l-2.4.96a.75.75 0 0 1-.91-.33L2.79 15.4a.75.75 0 0 1 .18-.96L4.99 12.9a7.96 7.96 0 0 1-.06-.94c0-.32.02-.63.06-.94L2.97 9.44a.75.75 0 0 1-.18-.96l1.92-3.32a.75.75 0 0 1 .91-.33l2.4.96c.5-.4 1.05-.72 1.62-.94l.36-2.54A.75.75 0 0 1 10.16 2h3.84c.36 0 .67.26.74.64l.36 2.54c.57.22 1.12.54 1.62.94l2.4-.96a.75.75 0 0 1 .91.33l1.92 3.32a.75.75 0 0 1-.18.96l-2.03 1.58c.04.31.06.62.06.94z" />
            </svg>
          </button>

          <WindowControls />
        </div>
      </div>
    </header>
  );
}
