// 可拖拽分栏：通过 CSS 变量 --sidebar-w / --chat-w 调整左栏与 Chat 宽度
// 启动与窗口缩放时会 clamp，避免 Chat 占满导致编辑器/文件树不可见

import { useCallback, useEffect, useRef, useState } from "react";

const SIDEBAR_KEY = "ds_sidebar_w";
const CHAT_KEY = "ds_chat_w";
const SIDEBAR_MIN = 160;
const SIDEBAR_MAX = 480;
/** Chat 面板最小宽度：保证 Composer 底栏（pill + 发送）不被裁切 */
export const CHAT_MIN = 380;
const CHAT_MAX = 720;
/** 活动栏已移除，保留变量兼容旧 clamp 逻辑 */
const ACT_W = 0;
/** 中间编辑器至少保留的宽度（仿 Cursor：缩小时优先压中间，不压侧栏控件） */
export const CENTER_MIN = 280;

/** 分栏尺寸 Hook 返回值 */
export interface ResizablePanelsState {
  sidebarW: number;
  chatW: number;
  /** 开始拖拽左栏分隔条 */
  startSidebarDrag: (e: React.MouseEvent) => void;
  /** 开始拖拽 Chat 分隔条 */
  startChatDrag: (e: React.MouseEvent) => void;
  /** 恢复默认分栏宽度（布局被挤没时使用） */
  resetLayout: () => void;
}

/** 从 localStorage 读取持久化的分栏宽度 */
function loadWidth(key: string, fallback: number): number {
  try {
    const n = Number(localStorage.getItem(key));
    if (Number.isFinite(n) && n > 0) return n;
  } catch {
    /* 忽略 */
  }
  return fallback;
}

/**
 * 根据窗口宽度限制 sidebar/chat，保证中间编辑器至少 CENTER_MIN 像素可见。
 * @param sidebarW 资源管理器宽度
 * @param chatW Chat 宽度
 * @param sidebarOpen 侧栏是否展开（折叠时 sidebar 占 0）
 */
export function clampPanelWidths(
  sidebarW: number,
  chatW: number,
  sidebarOpen: boolean,
): { sidebarW: number; chatW: number } {
  const winW = typeof window !== "undefined" ? window.innerWidth : 1200;
  let side = sidebarOpen ? Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, sidebarW)) : 0;
  let chat = Math.min(CHAT_MAX, Math.max(CHAT_MIN, chatW));

  // 固定侧栏 + 中间最小编辑区 + Chat 最小宽度不得超过窗口
  const maxChat = winW - ACT_W - side - CENTER_MIN;
  if (maxChat >= CHAT_MIN) {
    chat = Math.min(chat, maxChat);
    chat = Math.max(CHAT_MIN, chat);
  } else {
    // 窗口过窄：Chat 保持 CHAT_MIN，优先压缩侧栏
    chat = CHAT_MIN;
    if (sidebarOpen) {
      const sideBudget = winW - ACT_W - CHAT_MIN - CENTER_MIN;
      side = Math.max(SIDEBAR_MIN, Math.min(side, sideBudget));
    }
  }

  const maxSide = winW - ACT_W - chat - CENTER_MIN;
  if (sidebarOpen && side > maxSide) {
    side = Math.max(SIDEBAR_MIN, maxSide);
  }

  // 仍超出时只压中间列（1fr），Chat/侧栏不再低于最小值
  if (ACT_W + side + chat > winW) {
    chat = CHAT_MIN;
    if (sidebarOpen) {
      side = Math.max(SIDEBAR_MIN, Math.min(side, winW - ACT_W - CHAT_MIN));
    }
  }

  return { sidebarW: sidebarOpen ? side : sidebarW, chatW: chat };
}

/**
 * 管理 IDE 左栏与 Chat 面板宽度，写入 CSS 变量并持久化。
 */
export function useResizablePanels(sidebarOpen: boolean): ResizablePanelsState {
  const [sidebarW, setSidebarW] = useState(() => {
    const raw = loadWidth(SIDEBAR_KEY, 260);
    return clampPanelWidths(raw, loadWidth(CHAT_KEY, 400), true).sidebarW;
  });
  const [chatW, setChatW] = useState(() => {
    const raw = loadWidth(CHAT_KEY, 400);
    return clampPanelWidths(loadWidth(SIDEBAR_KEY, 260), raw, true).chatW;
  });
  const widthsRef = useRef({ sidebarW, chatW });
  widthsRef.current = { sidebarW, chatW };

  /** 窗口尺寸变化或侧栏开关变化时重新 clamp，防止「只剩 Chat」 */
  useEffect(() => {
    const apply = () => {
      const { sidebarW: sw, chatW: cw } = widthsRef.current;
      const c = clampPanelWidths(sw, cw, sidebarOpen);
      if (c.sidebarW !== sw) setSidebarW(c.sidebarW);
      if (c.chatW !== cw) setChatW(c.chatW);
    };
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [sidebarOpen]);

  // 同步到 CSS 变量与 localStorage
  useEffect(() => {
    document.documentElement.style.setProperty("--sidebar-w", `${sidebarW}px`);
    localStorage.setItem(SIDEBAR_KEY, String(sidebarW));
  }, [sidebarW]);

  useEffect(() => {
    document.documentElement.style.setProperty("--chat-w", `${chatW}px`);
    localStorage.setItem(CHAT_KEY, String(chatW));
  }, [chatW]);

  /** 通用水平拖拽逻辑 */
  const bindDrag = useCallback((onDelta: (dx: number) => void) => {
    return (e: React.MouseEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const move = (ev: MouseEvent) => onDelta(ev.clientX - startX);
      const up = () => {
        window.removeEventListener("mousemove", move);
        window.removeEventListener("mouseup", up);
      };
      window.addEventListener("mousemove", move);
      window.addEventListener("mouseup", up);
    };
  }, []);

  const startSidebarDrag = useCallback(
    (e: React.MouseEvent) => {
      const startW = sidebarW;
      bindDrag((dx) => {
        const next = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, startW + dx));
        const clamped = clampPanelWidths(next, chatW, sidebarOpen);
        setSidebarW(clamped.sidebarW);
        setChatW(clamped.chatW);
      })(e);
    },
    [sidebarW, chatW, sidebarOpen, bindDrag],
  );

  const startChatDrag = useCallback(
    (e: React.MouseEvent) => {
      const startW = chatW;
      bindDrag((dx) => {
        const next = Math.min(CHAT_MAX, Math.max(CHAT_MIN, startW - dx));
        const clamped = clampPanelWidths(sidebarW, next, sidebarOpen);
        setSidebarW(clamped.sidebarW);
        setChatW(clamped.chatW);
      })(e);
    },
    [sidebarW, chatW, sidebarOpen, bindDrag],
  );

  /** 恢复默认三栏比例 */
  const resetLayout = useCallback(() => {
    const def = clampPanelWidths(260, 400, sidebarOpen);
    setSidebarW(def.sidebarW);
    setChatW(def.chatW);
  }, [sidebarOpen]);

  return { sidebarW, chatW, startSidebarDrag, startChatDrag, resetLayout };
}
