// 会话列表侧栏：展示线程、新建、切换

import type { ThreadRecord } from "../api/types";
import type { Locale } from "../i18n";
import { formatThreadTabTitle } from "../utils/threadTitle";

interface ThreadListProps {
  threads: ThreadRecord[];
  activeId: string | null;
  connected: boolean;
  locale?: Locale;
  onSelect: (id: string) => void;
  onCreate: () => void;
  /** 关闭（归档）会话 */
  onClose: (id: string) => void;
  /** 新建会话的默认模型/模式 */
  newModel: string;
  newMode: string;
  models: string[];
  modes: string[];
  onNewModel: (v: string) => void;
  onNewMode: (v: string) => void;
}

export function ThreadList({
  threads,
  activeId,
  connected,
  locale = "zh",
  onSelect,
  onCreate,
  onClose,
  newModel,
  newMode,
  models,
  modes,
  onNewModel,
  onNewMode,
}: ThreadListProps) {
  return (
    <aside className="sidebar">
      <div className="sidebar-head">
        <span className="brand">DeepSeek GUI</span>
        <span className={connected ? "dot dot-on" : "dot dot-off"} title={connected ? "已连接" : "未连接"} />
      </div>
      <button className="btn btn-primary btn-block" onClick={onCreate}>
        + 新建会话
      </button>
      <div className="new-opts">
        <select
          className="mini-select"
          value={newModel}
          title="新会话默认模型"
          onChange={(e) => onNewModel(e.target.value)}
        >
          {models.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <select
          className="mini-select"
          value={newMode}
          title="新会话默认模式"
          onChange={(e) => onNewMode(e.target.value)}
        >
          {modes.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
      </div>
      <div className="thread-list">
        {threads.length === 0 && <div className="thread-empty">暂无会话</div>}
        {threads.map((t) => (
          <div
            key={t.id}
            className={t.id === activeId ? "thread-item active" : "thread-item"}
            onClick={() => onSelect(t.id)}
          >
            <div className="thread-main">
              <div className="thread-title">{formatThreadTabTitle(t, locale)}</div>
              <div className="thread-sub">
                {t.model} · {t.mode}
              </div>
            </div>
            <button
              className="thread-close"
              title="关闭会话"
              onClick={(e) => {
                // 阻止冒泡，避免触发选中
                e.stopPropagation();
                onClose(t.id);
              }}
            >
              ×
            </button>
          </div>
        ))}
      </div>
    </aside>
  );
}
