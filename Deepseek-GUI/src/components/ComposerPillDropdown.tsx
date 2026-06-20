// Composer 底栏自定义下拉（替代原生 select，避免 WebView2 白底低对比选项）

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

/** 下拉选项 */
export interface PillDropdownOption {
  value: string;
  label: string;
  /** 副标题（如模式说明） */
  hint?: string;
}

interface ComposerPillDropdownProps {
  /** 左侧图标字符 */
  icon?: string;
  value: string;
  options: PillDropdownOption[];
  disabled?: boolean;
  onChange: (value: string) => void;
  title?: string;
  /** 触发按钮最大宽度 */
  maxWidth?: number;
  /** 菜单最小宽度 */
  menuMinWidth?: number;
}

/** 浮动菜单定位（portal 到 body，避免 chat-main overflow 裁剪） */
interface MenuPos {
  left: number;
  bottom: number;
  minWidth: number;
}

/** Cursor 风格 pill 下拉：深色菜单、向上展开 */
export function ComposerPillDropdown({
  icon,
  value,
  options,
  disabled,
  onChange,
  title,
  maxWidth = 160,
  menuMinWidth = 200,
}: ComposerPillDropdownProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [menuPos, setMenuPos] = useState<MenuPos | null>(null);

  const current = options.find((o) => o.value === value) ?? options[0];

  /** 根据触发按钮位置计算 portal 菜单坐标（向上展开） */
  const updateMenuPos = useCallback(() => {
    const el = triggerRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setMenuPos({
      left: Math.max(4, Math.min(r.left, window.innerWidth - menuMinWidth - 8)),
      bottom: window.innerHeight - r.top + 6,
      minWidth: menuMinWidth,
    });
  }, [menuMinWidth]);

  /** 打开/关闭下拉 */
  const toggleOpen = useCallback(() => {
    if (disabled) return;
    setOpen((v) => !v);
  }, [disabled]);

  useLayoutEffect(() => {
    if (!open) {
      setMenuPos(null);
      return;
    }
    updateMenuPos();
  }, [open, updateMenuPos, value]);

  /** 缩放/窗口变化时重算菜单位置 */
  useEffect(() => {
    if (!open) return;
    const onRelayout = () => updateMenuPos();
    window.addEventListener("resize", onRelayout);
    window.addEventListener("scroll", onRelayout, true);
    window.addEventListener("ds-ui-zoom", onRelayout);
    return () => {
      window.removeEventListener("resize", onRelayout);
      window.removeEventListener("scroll", onRelayout, true);
      window.removeEventListener("ds-ui-zoom", onRelayout);
    };
  }, [open, updateMenuPos]);

  /** 点击外部关闭（含 portal 菜单内部） */
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      const target = e.target as Node;
      if (wrapRef.current?.contains(target)) return;
      if ((target as HTMLElement).closest?.(".composer-pill-menu-float")) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const pick = useCallback(
    (v: string) => {
      onChange(v);
      setOpen(false);
    },
    [onChange],
  );

  return (
    <div
      className={`composer-pill-dropdown${open ? " open" : ""}`}
      ref={wrapRef}
      title={title}
    >
      <button
        ref={triggerRef}
        type="button"
        className="composer-pill composer-pill-trigger"
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        onMouseDown={(e) => {
          // WebView2 / 无边框窗口：避免 mousedown 被拖拽区或 textarea 抢走
          e.preventDefault();
          e.stopPropagation();
          toggleOpen();
        }}
        style={{ maxWidth }}
      >
        {icon && (
          <span className="composer-pill-icon" aria-hidden>
            {icon}
          </span>
        )}
        <span className="composer-pill-label">{current?.label ?? value}</span>
        <span className="composer-pill-chevron" aria-hidden>
          ▾
        </span>
      </button>

      {open &&
        menuPos &&
        createPortal(
          <div
            className="composer-pill-menu composer-pill-menu-float"
            role="listbox"
            style={{
              position: "fixed",
              left: menuPos.left,
              bottom: menuPos.bottom,
              minWidth: menuPos.minWidth,
            }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            {options.map((opt) => {
              const active = opt.value === value;
              return (
                <button
                  key={opt.value}
                  type="button"
                  role="option"
                  aria-selected={active}
                  className={`composer-pill-menu-item${active ? " active" : ""}`}
                  onClick={() => pick(opt.value)}
                >
                  <span className="composer-pill-menu-main">
                    <span className="composer-pill-menu-label">{opt.label}</span>
                    {opt.hint && <span className="composer-pill-menu-hint">{opt.hint}</span>}
                  </span>
                  {active && <span className="composer-pill-menu-check">✓</span>}
                </button>
              );
            })}
          </div>,
          document.body,
        )}
    </div>
  );
}
