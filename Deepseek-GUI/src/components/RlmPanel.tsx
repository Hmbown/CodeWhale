// RLM 会话面板：GET /v1/rlm/sessions

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { RlmSessionSummary } from "../api/types";
import type { Locale } from "../i18n";

interface RlmPanelProps {
  client: RuntimeClient;
  locale: Locale;
}

/** RLM 持久 Python 会话列表 */
export function RlmPanel({ client, locale }: RlmPanelProps) {
  const zh = locale === "zh";
  const [sessions, setSessions] = useState<RlmSessionSummary[]>([]);
  const [openCount, setOpenCount] = useState(0);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [selected, setSelected] = useState<RlmSessionSummary | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await client.listRlmSessions();
      setSessions(res.sessions);
      setOpenCount(res.open_count);
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), 4000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const onShow = async (name: string) => {
    try {
      const s = await client.getRlmSession(name);
      setSelected(s);
    } catch (e) {
      setErr((e as Error).message);
    }
  };

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">RLM</h3>
      <p className="settings-section-desc">
        {zh
          ? "持久 RLM Python 会话（rlm_open / rlm_eval），每 4 秒刷新。"
          : "Persistent RLM sessions (rlm_open / rlm_eval)."}
      </p>
      {err && <p className="settings-hint settings-hint-error">{err}</p>}

      <div className="adv-toolbar">
        <button type="button" className="btn btn-mini" onClick={() => void refresh()}>
          {zh ? "刷新" : "Refresh"}
        </button>
        <span className="adv-list-meta">
          {zh ? "打开" : "Open"}: {openCount} / {sessions.length}
        </span>
      </div>

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : sessions.length === 0 ? (
        <p className="adv-empty">{zh ? "暂无 RLM 会话" : "No RLM sessions"}</p>
      ) : (
        <ul className="adv-list">
          {sessions.map((s) => (
            <li key={s.name} className="adv-list-item">
              <div className="adv-list-main">
                <strong>{s.name}</strong>
                <span className="adv-list-meta">
                  {s.id} · {s.is_open ? (zh ? "打开" : "open") : zh ? "已关闭" : "closed"} · rpc{" "}
                  {s.rpc_count}
                </span>
                <span className="adv-list-meta">
                  {s.context_meta.type} · {s.context_meta.length} chars
                </span>
                <span className="adv-list-meta">{s.context_meta.preview_500.slice(0, 160)}…</span>
              </div>
              <button type="button" className="btn btn-mini" onClick={() => void onShow(s.name)}>
                {zh ? "详情" : "Detail"}
              </button>
            </li>
          ))}
        </ul>
      )}

      {selected && (
        <pre className="adv-json-preview">{JSON.stringify(selected, null, 2)}</pre>
      )}
    </div>
  );
}
