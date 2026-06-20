// Jobs 面板：通过 GET /v1/jobs 管理后台 Shell 作业（对齐 TUI /jobs）

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { ShellJobDetail, ShellJobSnapshot } from "../api/types";
import type { Locale } from "../i18n";

interface JobsPanelProps {
  client: RuntimeClient;
  locale: Locale;
}

/** 格式化作业状态标签 */
function statusLabel(job: ShellJobSnapshot): string {
  if (job.stale) return "stale";
  return String(job.status).toLowerCase();
}

/** Shell Jobs 管理面板 */
export function JobsPanel({ client, locale }: JobsPanelProps) {
  const zh = locale === "zh";
  const [jobs, setJobs] = useState<ShellJobSnapshot[]>([]);
  const [runningCount, setRunningCount] = useState(0);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ShellJobDetail | null>(null);
  const [stdinText, setStdinText] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const res = await client.listJobs({ limit: 50 });
      setJobs(res.jobs);
      setRunningCount(res.running_count);
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(id);
  }, [refresh]);

  /** 加载选中作业详情 */
  const loadDetail = async (id: string) => {
    setSelectedId(id);
    setDetail(null);
    try {
      const d = await client.getJob(id);
      setDetail(d);
    } catch (e) {
      setErr((e as Error).message);
    }
  };

  /** 取消作业 */
  const onCancel = async (id: string) => {
    if (!window.confirm(zh ? `取消作业 ${id}？` : `Cancel job ${id}?`)) return;
    setBusy(true);
    try {
      await client.cancelJob(id);
      await refresh();
      if (selectedId === id) await loadDetail(id);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  /** 发送 stdin */
  const onSendStdin = async (close: boolean) => {
    if (!selectedId || !stdinText.trim()) return;
    setBusy(true);
    try {
      await client.writeJobStdin(selectedId, stdinText, close);
      setStdinText("");
      await loadDetail(selectedId);
      await refresh();
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">Jobs</h3>
      <p className="settings-section-desc">
        {zh
          ? "后台 Shell 作业（GET /v1/jobs），每 3 秒刷新。与 TUI /jobs 同源。"
          : "Background shell jobs via /v1/jobs (same as TUI /jobs)."}
      </p>
      {err && <p className="settings-hint settings-hint-error">{err}</p>}

      <div className="adv-toolbar">
        <button type="button" className="btn btn-mini" onClick={() => void refresh()}>
          {zh ? "刷新" : "Refresh"}
        </button>
        <span className="adv-list-meta">
          {zh ? "运行中" : "Running"}: {runningCount}
        </span>
      </div>

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : jobs.length === 0 ? (
        <p className="adv-empty">{zh ? "暂无 Shell 作业" : "No shell jobs"}</p>
      ) : (
        <ul className="adv-list">
          {jobs.map((j) => (
            <li key={j.id} className="adv-list-item">
              <div className="adv-list-main">
                <strong>{j.id}</strong>
                <span className="adv-list-meta">
                  {statusLabel(j)} · {j.elapsed_ms}ms · {j.command}
                </span>
                {(j.stdout_tail || j.stderr_tail) && (
                  <span className="adv-list-meta">{j.stdout_tail || j.stderr_tail}</span>
                )}
              </div>
              <div className="adv-list-actions">
                <button type="button" className="btn btn-mini" onClick={() => void loadDetail(j.id)}>
                  {zh ? "详情" : "Detail"}
                </button>
                {j.status === "Running" && (
                  <button
                    type="button"
                    className="btn btn-mini btn-danger"
                    disabled={busy}
                    onClick={() => void onCancel(j.id)}
                  >
                    {zh ? "取消" : "Cancel"}
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}

      {detail && (
        <div className="adv-form">
          <h4 className="adv-form-title">{detail.snapshot.command}</h4>
          <pre className="adv-json-preview">
            {detail.stdout}
            {detail.stderr ? `\n[stderr]\n${detail.stderr}` : ""}
          </pre>
          {detail.snapshot.stdin_available && detail.snapshot.status === "Running" && (
            <div className="adv-form-row">
              <input
                className="cfg-input"
                placeholder="stdin"
                value={stdinText}
                onChange={(e) => setStdinText(e.target.value)}
              />
              <button type="button" className="btn btn-mini" disabled={busy} onClick={() => void onSendStdin(false)}>
                {zh ? "发送" : "Send"}
              </button>
              <button type="button" className="btn btn-mini" disabled={busy} onClick={() => void onSendStdin(true)}>
                {zh ? "发送并关闭" : "Send & close"}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
