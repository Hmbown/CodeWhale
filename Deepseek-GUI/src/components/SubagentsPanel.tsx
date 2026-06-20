// Subagents 面板：GET /v1/subagents（优先 HTTP，离线回退读本地 JSON）

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { SubAgentResult } from "../api/types";
import type { Locale } from "../i18n";
import { getSubagentState, isTauri } from "../api/tauri";

interface SubagentsPanelProps {
  client: RuntimeClient;
  locale: Locale;
  workspace: string;
}

/** 格式化 Subagent 状态 */
function formatStatus(status: SubAgentResult["status"]): string {
  if (typeof status === "string") return status;
  if (status && typeof status === "object") {
    const key = Object.keys(status)[0];
    const val = (status as Record<string, unknown>)[key];
    return val ? `${key}: ${val}` : key;
  }
  return "?";
}

/** Subagent 会话管理面板 */
export function SubagentsPanel({ client, locale, workspace }: SubagentsPanelProps) {
  const zh = locale === "zh";
  const [agents, setAgents] = useState<SubAgentResult[]>([]);
  const [runningCount, setRunningCount] = useState(0);
  const [source, setSource] = useState<"api" | "file">("api");
  const [path, setPath] = useState("");
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [includeArchived, setIncludeArchived] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await client.listSubagents({ includeArchived, limit: 50 });
      setAgents(res.agents);
      setRunningCount(res.running_count);
      setSource("api");
      setErr(null);
    } catch {
      // HTTP 不可用时回退读本地状态文件
      if (isTauri() && workspace.trim()) {
        try {
          const data = await getSubagentState(workspace);
          setPath(data.path);
          setSource("file");
          const raw = data.raw as { agents?: SubAgentResult[] } | null;
          const list = Array.isArray(raw?.agents) ? raw!.agents! : [];
          setAgents(list);
          setRunningCount(list.filter((a) => formatStatus(a.status) === "Running").length);
          setErr(null);
        } catch (e) {
          setErr((e as Error).message);
        }
      } else {
        setErr(zh ? "无法连接后端 /v1/subagents" : "Cannot reach /v1/subagents");
      }
    } finally {
      setLoading(false);
    }
  }, [client, includeArchived, workspace, zh]);

  useEffect(() => {
    setLoading(true);
    void refresh();
    const id = window.setInterval(() => void refresh(), 4000);
    return () => window.clearInterval(id);
  }, [refresh]);

  /** 取消运行中的 Subagent */
  const onCancel = async (agentId: string) => {
    if (!window.confirm(zh ? `取消 ${agentId}？` : `Cancel ${agentId}?`)) return;
    setBusy(agentId);
    try {
      await client.cancelSubagent(agentId);
      await refresh();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">Subagents</h3>
      <p className="settings-section-desc">
        {zh
          ? "Subagent 状态（GET /v1/subagents，每 4 秒刷新）。"
          : "Subagent state via /v1/subagents."}
      </p>
      {path && source === "file" && <p className="cfg-tip">{path}（{zh ? "离线回退" : "offline"}）</p>}
      {err && <p className="settings-hint settings-hint-error">{err}</p>}

      <div className="adv-toolbar">
        <button type="button" className="btn btn-mini" onClick={() => void refresh()}>
          {zh ? "刷新" : "Refresh"}
        </button>
        <label className="cfg-check">
          <input
            type="checkbox"
            checked={includeArchived}
            onChange={(e) => setIncludeArchived(e.target.checked)}
          />
          {zh ? "含归档" : "Archived"}
        </label>
        <span className="adv-list-meta">
          {zh ? "运行中" : "Running"}: {runningCount} · {source === "api" ? "HTTP" : "file"}
        </span>
      </div>

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : agents.length === 0 ? (
        <p className="adv-empty">{zh ? "暂无 Subagent" : "No subagents"}</p>
      ) : (
        <ul className="adv-list">
          {agents.map((a) => (
            <li key={a.agent_id} className="adv-list-item">
              <div className="adv-list-main">
                <strong>{a.nickname || a.name}</strong>
                <span className="adv-list-meta">
                  {a.agent_id} · {formatStatus(a.status)} · {a.agent_type}
                </span>
                <span className="adv-list-meta">{a.assignment?.objective?.slice(0, 120)}</span>
                {a.result && <span className="adv-list-meta">{a.result.slice(0, 200)}</span>}
              </div>
              {source === "api" && formatStatus(a.status) === "Running" && (
                <button
                  type="button"
                  className="btn btn-mini btn-danger"
                  disabled={busy === a.agent_id}
                  onClick={() => void onCancel(a.agent_id)}
                >
                  {zh ? "取消" : "Cancel"}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
