// 自定义标题栏窗口控制按钮（最小化 / 最大化 / 关闭），仅 Tauri 桌面环境显示

import { useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "../api/tauri";

/** Windows 风格标题栏右侧窗口按钮 */
export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  /** 同步最大化状态（用于切换图标） */
  useEffect(() => {
    if (!isTauri()) return;
    const win = getCurrentWindow();
    let alive = true;
    void win.isMaximized().then((v) => {
      if (alive) setMaximized(v);
    });
    const setup = async () => {
      const unlisten = await win.onResized(async () => {
        if (alive) setMaximized(await win.isMaximized());
      });
      return unlisten;
    };
    let unlisten: (() => void) | undefined;
    void setup().then((fn) => {
      unlisten = fn;
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  const minimize = useCallback(() => {
    void getCurrentWindow().minimize();
  }, []);

  const toggleMaximize = useCallback(() => {
    void getCurrentWindow().toggleMaximize();
  }, []);

  const close = useCallback(() => {
    void getCurrentWindow().close();
  }, []);

  if (!isTauri()) return null;

  return (
    <div className="window-controls" data-tauri-drag-region="false">
      <button
        type="button"
        className="window-control-btn"
        title="最小化"
        aria-label="Minimize"
        onClick={minimize}
      >
        <svg viewBox="0 0 12 12" aria-hidden>
          <rect x="1" y="5.5" width="10" height="1" fill="currentColor" />
        </svg>
      </button>
      <button
        type="button"
        className="window-control-btn"
        title={maximized ? "还原" : "最大化"}
        aria-label={maximized ? "Restore" : "Maximize"}
        onClick={toggleMaximize}
      >
        {maximized ? (
          <svg viewBox="0 0 12 12" aria-hidden>
            <path
              d="M3 3h6v6H3V3zm1 1v4h4V4H4zm2-2h5v5h-1V3H4V2h2z"
              fill="currentColor"
            />
          </svg>
        ) : (
          <svg viewBox="0 0 12 12" aria-hidden>
            <rect x="1.5" y="1.5" width="9" height="9" fill="none" stroke="currentColor" />
          </svg>
        )}
      </button>
      <button
        type="button"
        className="window-control-btn window-control-close"
        title="关闭"
        aria-label="Close"
        onClick={close}
      >
        <svg viewBox="0 0 12 12" aria-hidden>
          <path d="M2 2l8 8M10 2L2 10" stroke="currentColor" strokeWidth="1.2" />
        </svg>
      </button>
    </div>
  );
}
