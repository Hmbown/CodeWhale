// 会话搜索模态：调用 /v1/threads/summary?search= 模糊检索并切换会话

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { ThreadSummary } from "../api/types";
import { t, type Locale } from "../i18n";

interface ThreadSearchProps {
  client: RuntimeClient;
  locale: Locale;
  onSelect: (threadId: string) => void;
  onClose: () => void;
}

export function ThreadSearch({ client, locale, onSelect, onClose }: ThreadSearchProps) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<ThreadSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [idx, setIdx] = useState(0);

  /** 防抖搜索线程摘要 */
  useEffect(() => {
    let alive = true;
    const timer = setTimeout(async () => {
      setLoading(true);
      try {
        const list = await client.searchThreads(query.trim() || undefined, 30);
        if (alive) {
          setResults(list);
          setIdx(0);
        }
      } catch {
        if (alive) setResults([]);
      } finally {
        if (alive) setLoading(false);
      }
    }, 200);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [client, query]);

  const onKey = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setIdx((i) => Math.min(i + 1, results.length - 1));
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setIdx((i) => Math.max(i - 1, 0));
        return;
      }
      if (e.key === "Enter" && results[idx]) {
        e.preventDefault();
        onSelect(results[idx].id);
        onClose();
      }
    },
    [results, idx, onSelect, onClose],
  );

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="quick-open" onClick={(e) => e.stopPropagation()}>
        <input
          className="quick-open-input"
          placeholder={t("search.threads", locale)}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKey}
          autoFocus
        />
        <div className="quick-open-list">
          {loading && <div className="quick-open-empty">…</div>}
          {!loading && results.length === 0 && (
            <div className="quick-open-empty">{locale === "zh" ? "无匹配会话" : "No chats found"}</div>
          )}
          {results.map((r, i) => (
            <button
              key={r.id}
              type="button"
              className={`quick-open-item${i === idx ? " active" : ""}`}
              onClick={() => {
                onSelect(r.id);
                onClose();
              }}
            >
              <span className="quick-open-name">
                {r.title?.trim() || t("thread.defaultTitle", locale)}
                {r.archived ? " 🗄" : ""}
              </span>
              <span className="quick-open-rel">{r.preview?.slice(0, 60) || r.model}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
