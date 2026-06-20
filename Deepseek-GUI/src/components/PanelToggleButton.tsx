// 侧栏收起/展开按钮（仿 Cursor）：左侧栏 / 右侧聊天各一枚 SVG 图标

import type { Locale } from "../i18n";

/** 面板方向：left=左侧资源管理器，right=右侧聊天 */
export type PanelSide = "left" | "right";

interface PanelToggleButtonProps {
  side: PanelSide;
  /** 当前是否展开（展开时按钮高亮） */
  open: boolean;
  locale: Locale;
  onClick: () => void;
  /** 额外 class，便于嵌入 pane-head / chat-header 等容器 */
  className?: string;
}

/** 根据语言返回 tooltip */
function tooltip(side: PanelSide, open: boolean, locale: Locale): string {
  if (side === "left") {
    return locale === "zh"
      ? open
        ? "隐藏资源管理器 (Ctrl+B)"
        : "显示资源管理器 (Ctrl+B)"
      : open
        ? "Hide Explorer (Ctrl+B)"
        : "Show Explorer (Ctrl+B)";
  }
  return locale === "zh"
    ? open
      ? "隐藏聊天面板"
      : "显示聊天面板"
    : open
      ? "Hide chat panel"
      : "Show chat panel";
}

/**
 * Cursor 风格侧栏开关：left 图标竖条在左，right 图标竖条在右。
 */
export function PanelToggleButton({
  side,
  open,
  locale,
  onClick,
  className = "",
}: PanelToggleButtonProps) {
  const path =
    side === "left"
      ? "M3 4h18a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zm1 2v12h5V6H4zm7 0v12h9V6h-9z"
      : "M3 4h18a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zm1 2v12h11V6H4zm13 0v12h3V6h-3z";

  return (
    <button
      type="button"
      className={`panel-toggle-btn${open ? " active" : ""}${className ? ` ${className}` : ""}`}
      onClick={onClick}
      title={tooltip(side, open, locale)}
      aria-label={tooltip(side, open, locale)}
    >
      <svg viewBox="0 0 24 24" aria-hidden className="panel-toggle-svg">
        <path d={path} />
      </svg>
    </button>
  );
}
