// 定时自动化界面：CRUD + 运行/暂停/恢复（/v1/automations/*）

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { AutomationRecord, AutomationRunRecord } from "../api/types";

/** RRULE 预设（简化创建） */
const RRULE_PRESETS = [
  { label: "每小时", value: "FREQ=HOURLY;INTERVAL=1" },
  { label: "每天 9:00", value: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=9;BYMINUTE=0" },
  { label: "每周一 9:00", value: "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=0" },
];

interface AutomationsViewProps {
  client: RuntimeClient;
  onBack: () => void;
  /** 嵌入 TasksView 时不显示顶部标题栏 */
  embedded?: boolean;
}

export function AutomationsView({ client, onBack, embedded }: AutomationsViewProps) {
  const [items, setItems] = useState<AutomationRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  // 新建表单
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [rrule, setRrule] = useState(RRULE_PRESETS[0].value);
  const [submitting, setSubmitting] = useState(false);

  /** 拉取自动化列表 */
  const refresh = useCallback(async () => {
    try {
      setItems(await client.listAutomations());
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 5000);
    return () => clearInterval(t);
  }, [refresh]);

  /** 创建自动化 */
  const onCreate = useCallback(async () => {
    if (!name.trim() || !prompt.trim()) {
      alert("请填写名称与提示词");
      return;
    }
    setSubmitting(true);
    try {
      await client.createAutomation({ name: name.trim(), prompt: prompt.trim(), rrule });
      setName("");
      setPrompt("");
      await refresh();
    } catch (e) {
      alert(`创建失败：${(e as Error).message}`);
    } finally {
      setSubmitting(false);
    }
  }, [client, name, prompt, rrule, refresh]);

  /** 执行 run/pause/resume/delete 等操作 */
  const act = useCallback(
    async (id: string, action: "run" | "pause" | "resume" | "delete") => {
      setBusy(id);
      try {
        if (action === "run") await client.runAutomation(id);
        else if (action === "pause") await client.pauseAutomation(id);
        else if (action === "resume") await client.resumeAutomation(id);
        else if (action === "delete") {
          if (!window.confirm("确定删除此自动化？")) return;
          await client.deleteAutomation(id);
        }
        await refresh();
      } catch (e) {
        alert(`操作失败：${(e as Error).message}`);
      } finally {
        setBusy(null);
      }
    },
    [client, refresh],
  );

  return (
    <div className={`automations-view${embedded ? " embedded" : ""}`}>
      {!embedded && (
        <div className="tasks-head">
          <span className="pane-title">定时自动化</span>
          <div className="pane-head-btns">
            <button className="btn-mini" onClick={refresh} title="刷新">
              ⟳
            </button>
            <button className="btn-mini" onClick={onBack}>
              返回
            </button>
          </div>
        </div>
      )}

      {error && <div className="banner banner-warn">{error}</div>}

      <div className="auto-form">
        <h4>新建自动化</h4>
        <input
          className="connbar-input"
          placeholder="名称"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <textarea
          className="task-prompt"
          placeholder="每次触发时发送给 Agent 的提示词"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          rows={3}
        />
        <select className="mini-select" value={rrule} onChange={(e) => setRrule(e.target.value)}>
          {RRULE_PRESETS.map((p) => (
            <option key={p.value} value={p.value}>
              {p.label}
            </option>
          ))}
        </select>
        <button className="btn btn-primary" disabled={submitting} onClick={() => void onCreate()}>
          {submitting ? "创建中…" : "创建"}
        </button>
      </div>

      <div className="auto-list">
        <h4>已配置 ({items.length})</h4>
        {loading && <div className="placeholder">加载中…</div>}
        {!loading && items.length === 0 && (
          <div className="placeholder">暂无自动化，可在上方创建。</div>
        )}
        {items.map((a) => (
          <AutomationCard
            key={a.id}
            item={a}
            busy={busy === a.id}
            client={client}
            onAct={act}
          />
        ))}
      </div>
    </div>
  );
}

/** 单条自动化卡片 */
function AutomationCard({
  item,
  busy,
  client,
  onAct,
}: {
  item: AutomationRecord;
  busy: boolean;
  client: RuntimeClient;
  onAct: (id: string, action: "run" | "pause" | "resume" | "delete") => void;
}) {
  const [runs, setRuns] = useState<AutomationRunRecord[]>([]);
  const [showRuns, setShowRuns] = useState(false);

  const loadRuns = useCallback(async () => {
    try {
      setRuns(await client.listAutomationRuns(item.id, 5));
    } catch {
      setRuns([]);
    }
  }, [client, item.id]);

  return (
    <div className="auto-card">
      <div className="auto-card-head">
        <strong>{item.name}</strong>
        <span className={`auto-status status-${item.status}`}>{item.status}</span>
      </div>
      <div className="auto-prompt">{item.prompt.slice(0, 120)}{item.prompt.length > 120 ? "…" : ""}</div>
      <div className="auto-meta">
        <code>{item.rrule}</code>
        {item.next_run_at && <span>下次：{new Date(item.next_run_at).toLocaleString()}</span>}
      </div>
      <div className="auto-actions">
        <button className="btn-mini" disabled={busy} onClick={() => void onAct(item.id, "run")}>
          立即运行
        </button>
        {item.status === "paused" ? (
          <button className="btn-mini" disabled={busy} onClick={() => void onAct(item.id, "resume")}>
            恢复
          </button>
        ) : (
          <button className="btn-mini" disabled={busy} onClick={() => void onAct(item.id, "pause")}>
            暂停
          </button>
        )}
        <button className="btn-mini" disabled={busy} onClick={() => void onAct(item.id, "delete")}>
          删除
        </button>
        <button
          className="btn-mini"
          onClick={() => {
            setShowRuns((v) => !v);
            if (!showRuns) void loadRuns();
          }}
        >
          {showRuns ? "隐藏运行" : "运行记录"}
        </button>
      </div>
      {showRuns && (
        <ul className="auto-runs">
          {runs.length === 0 && <li>暂无运行记录</li>}
          {runs.map((r) => (
            <li key={r.id}>
              {r.status} · {new Date(r.scheduled_for).toLocaleString()}
              {r.error && <span className="auto-run-err"> — {r.error}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
