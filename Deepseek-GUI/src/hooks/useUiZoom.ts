// 全局 UI 缩放 Hook：快捷键 Ctrl+/−/0、Ctrl+滚轮，步进仿 Cursor / VS Code

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "../api/tauri";
import {
  UI_ZOOM_LEVEL_DEFAULT,
  applyUiZoomLevel,
  clampUiZoomLevel,
  formatUiZoomLabel,
  loadUiZoomLevel,
  scaleFromLevel,
} from "../utils/uiZoom";

/** 返回当前缩放与操作方法 */
export function useUiZoom() {
  const [level, setLevel] = useState(loadUiZoomLevel);
  const levelRef = useRef(level);
  levelRef.current = level;

  const zoom = scaleFromLevel(level);

  /** 应用 level 并更新 state */
  const commitLevel = useCallback((nextLevel: number) => {
    const clamped = clampUiZoomLevel(nextLevel);
    setLevel(clamped);
    void applyUiZoomLevel(clamped).then(() => {
      window.dispatchEvent(new CustomEvent("ds-ui-zoom"));
    });
  }, []);

  const zoomIn = useCallback(() => {
    commitLevel(levelRef.current + 1);
  }, [commitLevel]);

  const zoomOut = useCallback(() => {
    commitLevel(levelRef.current - 1);
  }, [commitLevel]);

  const zoomReset = useCallback(() => {
    commitLevel(UI_ZOOM_LEVEL_DEFAULT);
  }, [commitLevel]);

  /** 启动时恢复上次缩放 */
  useEffect(() => {
    void applyUiZoomLevel(loadUiZoomLevel()).then(() => {
      window.dispatchEvent(new CustomEvent("ds-ui-zoom"));
    });
  }, []);

  /** 系统 DPI 变化后重新应用用户缩放（WebView2 可能重置 zoom） */
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("tauri://scale-change", () => {
      if (disposed) return;
      void applyUiZoomLevel(levelRef.current);
    }).then((fn) => {
      if (disposed) {
        fn();
        return;
      }
      unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** 全局快捷键：Ctrl/Cmd + = / - / 0 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const key = e.key;
      if (key === "=" || key === "+") {
        e.preventDefault();
        zoomIn();
      } else if (key === "-" || key === "_") {
        e.preventDefault();
        zoomOut();
      } else if (key === "0") {
        e.preventDefault();
        zoomReset();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [zoomIn, zoomOut, zoomReset]);

  /** 全局 Ctrl+滚轮缩放（任意区域生效） */
  useEffect(() => {
    const onWheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      if (e.deltaY < 0) zoomIn();
      else zoomOut();
    };
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => window.removeEventListener("wheel", onWheel);
  }, [zoomIn, zoomOut]);

  return {
    zoom,
    zoomLevel: level,
    zoomIn,
    zoomOut,
    zoomReset,
    zoomLabel: formatUiZoomLabel(zoom),
  };
}
