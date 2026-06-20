// 右侧 Agent 历史面板（仿 Cursor）：搜索、新建会话、按日期分组列表

import { useCallback, useEffect, useMemo, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { SessionMetadata, ThreadRecord } from "../api/types";
import type { Locale } from "../i18n";
import { formatThreadTabTitle } from "../utils/threadTitle";

interface AgentHistoryPanelProps {
  client: RuntimeClient;
  locale: Locale;
  /** 当前工作区下的线程列表 */
  threads: ThreadRecord[];
  activeId: string | null;
  /** 选中已有线程 */
  onSelectThread: (id: string) => void;
  /** 新建会话 */
  onNewChat: () => void;
  /** 恢复持久化历史会话为新线程 */
  onSessionResumed: (threadId: string) => void;
}

/** 将 ISO 时间戳格式化为「今天 / 昨天 / 日期」分组标签 */
function dateGroupLabel(iso: string, locale: Locale): string {
  const d = new Date(iso);
  const now = new Date();
  const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startThat = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const diffDays = Math.round((startToday.getTime() - startThat.getTime()) / 86400000);
  if (diffDays === 0) return locale === "zh" ? "今天" : "Today";
  if (diffDays === 1) return locale === "zh" ? "昨天" : "Yesterday";
  if (diffDays < 7) return locale === "zh" ? "本周" : "This week";
  return locale === "zh" ? "更早" : "Older";
}

/** 按 updated_at 降序分组线程 */
function groupThreadsByDate(
  threads: ThreadRecord[],
  locale: Locale,
): { label: string; items: ThreadRecord[] }[] {
  const sorted = [...threads].sort(
    (a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
  );
  const map = new Map<string, ThreadRecord[]>();
  for (const t of sorted) {
    const label = dateGroupLabel(t.updated_at, locale);
    if (!map.has(label)) map.set(label, []);
    map.get(label)!.push(t);
  }
  return Array.from(map.entries()).map(([label, items]) => ({ label, items }));
}

/**
 * Cursor 风格 Agent 历史侧栏：顶部搜索 + 新建 + 按日期分组的会话列表。
 */
export function AgentHistoryPanel({
  client,
  locale,
  threads,
  activeId,
  onSelectThread,
  onNewChat,
  onSessionResumed,
}: AgentHistoryPanelProps) {
  const [search, setSearch] = useState("");
  const [sessions, setSessions] = useState<SessionMetadata[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  /** 拉取后端持久化历史（供搜索补充） */
  const refreshSessions = useCallback(
    async (q: string) => {
      setLoading(true);
      try {
        const res = await client.listSessions(q || undefined, 50);
        setSessions(res.sessions);
      } catch {
        setSessions([]);
      } finally {
        setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    const t = window.setTimeout(() => void refreshSessions(search), 200);
    return () => window.clearTimeout(t);
  }, [search, refreshSessions]);

  /** 本地线程 + 远程历史合并搜索过滤 */
  const filteredThreads = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return threads;
    return threads.filter((t) => {
      const title = formatThreadTabTitle(t, locale).toLowerCase();
      return title.includes(q) || t.id.toLowerCase().includes(q);
    });
  }, [threads, search, locale]);

  const groups = useMemo(
    () => groupThreadsByDate(filteredThreads, locale),
    [filteredThreads, locale],
  );

  /** 恢复持久化会话 */
  const onResumeSession = useCallback(
    async (id: string) => {
      setBusy(id);
      try {
        const res = await client.resumeSession(id);
        onSessionResumed(res.thread_id);
      } catch (e) {
        alert(`${locale === "zh" ? "恢复失败" : "Resume failed"}：${(e as Error).message}`);
      } finally {
        setBusy(null);
      }
    },
    [client, locale, onSessionResumed],
  );

  const showPersisted =
    search.trim().length > 0 &&
    sessions.filter((s) => !threads.some((t) => t.id === s.id)).length > 0;

  return (
    <div className="agent-history-panel">
      {/* 顶部搜索（仿 Cursor Search Agents） */}
      <div className="agent-history-search-wrap">
        <input
          className="agent-history-search"
          placeholder={locale === "zh" ? "搜索会话…" : "Search chats…"}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>

      {/* 快捷操作行 */}
      <div className="agent-history-actions">
        <button type="button" className="agent-history-action" onClick={() => void onNewChat()}>
          <span className="agent-history-action-icon">+</span>
          <span className="agent-history-action-label">
            {locale === "zh" ? "新建会话" : "New chat"}
          </span>
          <kbd className="agent-history-kbd">Ctrl+N</kbd>
        </button>
      </div>

      <div className="agent-history-list">
        {loading && groups.length === 0 && (
          <div className="agent-history-empty">{locale === "zh" ? "加载中…" : "Loading…"}</div>
        )}

        {!loading && groups.length === 0 && !showPersisted && (
          <div className="agent-history-empty">
            {locale === "zh" ? "暂无会话" : "No chats yet"}
          </div>
        )}

        {groups.map((g) => (
          <section key={g.label} className="agent-history-group">
            <div className="agent-history-group-title">{g.label}</div>
            {g.items.map((t) => {
              const title = formatThreadTabTitle(t, locale);
              const isActive = t.id === activeId;
              const isDraft = !t.title || title.startsWith("Draft");
              return (
                <button
                  key={t.id}
                  type="button"
                  className={`agent-history-item${isActive ? " active" : ""}`}
                  onClick={() => onSelectThread(t.id)}
                >
                  <span className={`agent-history-item-icon${isDraft ? " draft" : ""}`}>
                    {isDraft ? "○" : "✓"}
                  </span>
                  <span className="agent-history-item-title">{title}</span>
                </button>
              );
            })}
          </section>
        ))}

        {/* 后端持久化历史（搜索时补充） */}
        {showPersisted && (
          <section className="agent-history-group">
            <div className="agent-history-group-title">
              {locale === "zh" ? "历史存档" : "Saved sessions"}
            </div>
            {sessions
              .filter((s) => !threads.some((t) => t.id === s.id))
              .map((s) => (
                <button
                  key={s.id}
                  type="button"
                  className="agent-history-item"
                  onClick={() => void onResumeSession(s.id)}
                  disabled={busy === s.id}
                >
                  <span className="agent-history-item-icon">↺</span>
                  <span className="agent-history-item-title">
                    {busy === s.id
                      ? locale === "zh"
                        ? "恢复中…"
                        : "Resuming…"
                      : s.title || s.id.slice(0, 8)}
                  </span>
                </button>
              ))}
          </section>
        )}
      </div>
    </div>
  );
}
