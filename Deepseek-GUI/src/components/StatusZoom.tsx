// 状态栏全局缩放控件（仿 Cursor / VS Code 右下角缩放）

interface StatusZoomProps {
  label: string;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onReset: () => void;
  locale: "zh" | "en";
}

/** 底部状态栏：缩小 / 百分比 / 放大 */
export function StatusZoom({ label, onZoomIn, onZoomOut, onReset, locale }: StatusZoomProps) {
  const zh = locale === "zh";
  return (
    <div className="status-zoom" title={zh ? "界面缩放 (Ctrl+滚轮)" : "UI zoom (Ctrl+wheel)"}>
      <button
        type="button"
        className="status-zoom-btn"
        onClick={onZoomOut}
        title={zh ? "缩小 (Ctrl+-)" : "Zoom out (Ctrl+-)"}
        aria-label={zh ? "缩小" : "Zoom out"}
      >
        −
      </button>
      <button
        type="button"
        className="status-zoom-label"
        onClick={onReset}
        title={zh ? "重置缩放 (Ctrl+0)" : "Reset zoom (Ctrl+0)"}
      >
        {label}
      </button>
      <button
        type="button"
        className="status-zoom-btn status-zoom-in"
        onClick={onZoomIn}
        title={zh ? "放大 (Ctrl+=)" : "Zoom in (Ctrl+=)"}
        aria-label={zh ? "放大" : "Zoom in"}
      >
        <svg viewBox="0 0 16 16" aria-hidden className="status-zoom-icon">
          <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.2" />
          <path d="M10.5 10.5L14 14" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
          <path d="M7 5v4M5 7h4" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  );
}
