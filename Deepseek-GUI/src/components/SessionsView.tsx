// 会话历史浏览器：列出后端持久化的历史会话，支持搜索、恢复为新线程、删除。
// 对接 GET /v1/sessions、POST /v1/sessions/{id}/resume-thread、DELETE /v1/sessions/{id}。

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { SessionMetadata } from "../api/types";

interface SessionsViewProps {
  client: RuntimeClient;
  /** 恢复会话成功后回调，传出新线程 id（由上层切换到该线程并刷新） */
  onResumed: (threadId: string) => void;
  onBack: () => void;
}

/**
 * 历史会话主界面。
 * @param client 运行时 API 客户端
 * @param onResumed 恢复成功回调（传出新线程 id）
 * @param onBack 返回回调
 */
export function SessionsView({ client, onResumed, onBack }: SessionsViewProps) {
  const [sessions, setSessions] = useState<SessionMetadata[]>([]);
  const [search, setSearch] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  /** 拉取会话列表（带搜索词） */
  const refresh = useCallback(
    async (q: string) => {
      setLoading(true);
      try {
        const res = await client.listSessions(q || undefined, 100);
        setSessions(res.sessions);
        setError(null);
      } catch (e) {
        setError((e as Error).message);
      } finally {
        setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    refresh("");
  }, [refresh]);

  /** 恢复会话为新线程 */
  const onResume = useCallback(
    async (id: string) => {
      setBusy(id);
      try {
        const res = await client.resumeSession(id);
        onResumed(res.thread_id);
      } catch (e) {
        alert(`恢复失败：${(e as Error).message}`);
      } finally {
        setBusy(null);
      }
    },
    [client, onResumed],
  );

  /** 删除会话 */
  const onDelete = useCallback(
    async (id: string, e: React.MouseEvent) => {
      e.stopPropagation();
      if (!window.confirm("确定删除该历史会话吗？此操作不可恢复。")) return;
      try {
        await client.deleteSession(id);
        await refresh(search);
      } catch (err) {
        alert(`删除失败：${(err as Error).message}`);
      }
    },
    [client, refresh, search],
  );

  return (
    <div className="tasks-view">
      <div className="tasks-head">
        <span className="pane-title">历史会话</span>
        <button className="btn-mini" onClick={onBack} title="返回聊天">
          ← 返回
        </button>
      </div>

      <div className="task-form-row">
        <input
          className="task-input"
          placeholder="搜索历史会话标题 / 内容…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") refresh(search);
          }}
        />
        <button className="btn primary" onClick={() => refresh(search)}>
          搜索
        </button>
      </div>

      <div className="task-list">
        {error && <div className="banner banner-warn">加载失败：{error}</div>}
        {!error && loading && <div className="pane-placeholder">加载中…</div>}
        {!error && !loading && sessions.length === 0 && (
          <div className="pane-placeholder">没有历史会话。</div>
        )}
        {sessions.map((s) => (
          <div
            key={s.id}
            className="task-card"
            style={{ cursor: "pointer" }}
            onClick={() => onResume(s.id)}
            title="点击恢复为新会话"
          >
            <div className="task-card-top">
              <span className="task-model">{s.model}</span>
              {s.mode && <span className="task-mode">{s.mode}</span>}
              <span className="task-dur">{s.message_count} 条消息</span>
              <button
                className="btn-mini task-cancel"
                onClick={(e) => onDelete(s.id, e)}
                title="删除会话"
              >
                删除
              </button>
            </div>
            <div className="task-prompt-text">
              {busy === s.id ? "恢复中…" : s.title || "(无标题会话)"}
            </div>
            <div className="task-meta">
              <span>{new Date(s.updated_at).toLocaleString()}</span>
              <span className="task-id">{s.id.slice(0, 8)}</span>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
