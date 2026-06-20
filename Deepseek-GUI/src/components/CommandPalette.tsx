// 命令面板（Ctrl+K）：集中检索并执行常用命令与视图切换。
// 纯前端组件，键盘可导航（上/下选择，Enter 执行，Esc 关闭）。

import { useEffect, useMemo, useRef, useState } from "react";

/** 单条命令定义 */
export interface PaletteCommand {
  id: string;
  /** 显示标题 */
  title: string;
  /** 副标题/提示（如对应的斜杠命令） */
  hint?: string;
  /** 执行动作 */
  run: () => void;
  /** 是否禁用（如无活动会话时） */
  disabled?: boolean;
}

interface CommandPaletteProps {
  commands: PaletteCommand[];
  onClose: () => void;
}

/**
 * 命令面板组件。
 * @param commands 可执行命令列表
 * @param onClose 关闭回调
 */
export function CommandPalette({ commands, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  // 打开即聚焦输入框
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // 模糊过滤（标题 + 提示）
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = commands.filter((c) => !c.disabled);
    if (!q) return list;
    return list.filter(
      (c) =>
        c.title.toLowerCase().includes(q) || (c.hint ?? "").toLowerCase().includes(q),
    );
  }, [commands, query]);

  // 过滤变化时重置选中项
  useEffect(() => {
    setSel(0);
  }, [query]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const cmd = filtered[sel];
      if (cmd) {
        onClose();
        cmd.run();
      }
    }
  };

  return (
    <div className="modal-overlay palette-overlay" onClick={onClose}>
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="输入命令… (Esc 关闭)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <div className="palette-list">
          {filtered.length === 0 && <div className="palette-empty">无匹配命令</div>}
          {filtered.map((c, i) => (
            <div
              key={c.id}
              className={`palette-item ${i === sel ? "active" : ""}`}
              onMouseEnter={() => setSel(i)}
              onClick={() => {
                onClose();
                c.run();
              }}
            >
              <span className="palette-title">{c.title}</span>
              {c.hint && <code className="palette-hint">{c.hint}</code>}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
