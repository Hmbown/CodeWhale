// Composer 排队 / 暂存条（对齐 TUI /queue 与 /stash）
// - 队列 queue：当前回合进行中时排队，回合结束后自动按序发送。
// - 暂存 stash：把队列整体停泊到本地持久存储，稍后弹回队列。

import type { Locale } from "../i18n";

interface QueueBarProps {
  locale: Locale;
  /** 待发送队列 */
  queued: string[];
  /** 已暂存条目 */
  stash: string[];
  /** 编辑某条队列项 */
  onEditQueued: (index: number, text: string) => void;
  /** 删除某条队列项 */
  onDropQueued: (index: number) => void;
  /** 清空队列 */
  onClearQueue: () => void;
  /** 队列整体停泊到暂存 */
  onStashQueue: () => void;
  /** 暂存整体弹回队列 */
  onPopStash: () => void;
  /** 清空暂存 */
  onClearStash: () => void;
}

/** 队列 / 暂存可视化与管理条 */
export function QueueBar({
  locale,
  queued,
  stash,
  onEditQueued,
  onDropQueued,
  onClearQueue,
  onStashQueue,
  onPopStash,
  onClearStash,
}: QueueBarProps) {
  const zh = locale === "zh";
  if (queued.length === 0 && stash.length === 0) return null;

  /** 截断预览文本，避免过长 */
  const preview = (s: string) => (s.length > 80 ? `${s.slice(0, 80)}…` : s);

  return (
    <div className="queue-bar">
      {queued.length > 0 && (
        <div className="queue-group">
          <div className="queue-head">
            <span className="queue-title">
              {zh ? "排队中" : "Queued"} ({queued.length})
            </span>
            <div className="queue-head-actions">
              <button type="button" className="btn-mini" onClick={onStashQueue}>
                {zh ? "全部暂存" : "Stash all"}
              </button>
              <button type="button" className="btn-mini" onClick={onClearQueue}>
                {zh ? "清空" : "Clear"}
              </button>
            </div>
          </div>
          <ul className="queue-list">
            {queued.map((q, i) => (
              <li key={i} className="queue-item">
                <span className="queue-num">{i + 1}</span>
                <span
                  className="queue-text"
                  title={zh ? "点击编辑" : "Click to edit"}
                  onClick={() => {
                    const next = window.prompt(zh ? "编辑队列项" : "Edit queued message", q);
                    if (next != null) onEditQueued(i, next);
                  }}
                >
                  {preview(q)}
                </span>
                <button
                  type="button"
                  className="queue-del"
                  title={zh ? "删除" : "Drop"}
                  onClick={() => onDropQueued(i)}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      {stash.length > 0 && (
        <div className="queue-group">
          <div className="queue-head">
            <span className="queue-title">
              {zh ? "暂存" : "Stash"} ({stash.length})
            </span>
            <div className="queue-head-actions">
              <button type="button" className="btn-mini" onClick={onPopStash}>
                {zh ? "弹回队列" : "Pop"}
              </button>
              <button type="button" className="btn-mini" onClick={onClearStash}>
                {zh ? "清空" : "Clear"}
              </button>
            </div>
          </div>
          <ul className="queue-list">
            {stash.map((s, i) => (
              <li key={i} className="queue-item queue-item-stash">
                <span className="queue-num">{i + 1}</span>
                <span className="queue-text">{preview(s)}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
