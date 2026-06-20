// 快照还原模态框（对齐 TUI /restore）：列出工作区 pre/post-turn 快照并一键还原。
// 数据来自后端 GET /v1/threads/{id}/snapshots，还原走 POST .../snapshots/restore。

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { SnapshotEntry } from "../api/types";
import type { Locale } from "../i18n";

interface SnapshotsModalProps {
  client: RuntimeClient;
  locale: Locale;
  /** 当前会话 id（快照按会话工作区读取） */
  threadId: string | null;
  onClose: () => void;
  /** 还原成功后回调（用于刷新文件树/编辑器） */
  onRestored?: () => void;
}

/** 将 Unix 秒格式化为本地时间字符串 */
function fmtTime(ts: number): string {
  if (!ts) return "";
  try {
    return new Date(ts * 1000).toLocaleString();
  } catch {
    return String(ts);
  }
}

/** 工作区快照浏览与还原模态框 */
export function SnapshotsModal({ client, locale, threadId, onClose, onRestored }: SnapshotsModalProps) {
  const zh = locale === "zh";
  const [snapshots, setSnapshots] = useState<SnapshotEntry[]>([]);
  const [workspace, setWorkspace] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!threadId) {
      setLoading(false);
      setErr(zh ? "请先选择一个会话" : "Select a chat first");
      return;
    }
    setLoading(true);
    try {
      const res = await client.listSnapshots(threadId, 100);
      setSnapshots(res.snapshots);
      setWorkspace(res.workspace);
      setErr(null);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [client, threadId, zh]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 还原到指定快照 */
  const onRestore = async (snap: SnapshotEntry) => {
    if (!threadId) return;
    const label = snap.label || snap.id.slice(0, 8);
    if (
      !window.confirm(
        zh
          ? `还原工作区到「${label}」？\n会先自动创建一条安全快照以便反悔。`
          : `Restore workspace to "${label}"?\nA safety snapshot is created first.`,
      )
    )
      return;
    setBusy(true);
    setMsg(zh ? "还原中…" : "Restoring…");
    try {
      const res = await client.restoreSnapshot(threadId, snap.id);
      setMsg(
        (zh ? "已还原到 " : "Restored to ") +
          res.restored.slice(0, 8) +
          (res.safety_snapshot
            ? (zh ? `（安全快照 ${res.safety_snapshot.slice(0, 8)}）` : ` (safety ${res.safety_snapshot.slice(0, 8)})`)
            : ""),
      );
      onRestored?.();
      await refresh();
    } catch (e) {
      setErr((e as Error).message);
      setMsg(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-panel diff-panel" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <span className="pane-title">{zh ? "快照还原 (/restore)" : "Snapshots (/restore)"}</span>
          <div style={{ display: "flex", gap: 8 }}>
            <button className="btn-mini" disabled={busy} onClick={() => void refresh()}>
              {zh ? "刷新" : "Refresh"}
            </button>
            <button className="btn-mini" onClick={onClose}>
              {zh ? "关闭" : "Close"}
            </button>
          </div>
        </div>
        <div className="modal-body">
          {workspace && <p className="cfg-tip">{workspace}</p>}
          {err && <div className="banner banner-warn">{err}</div>}
          {msg && <p className="settings-hint">{msg}</p>}
          {loading ? (
            <p>{zh ? "加载中…" : "Loading…"}</p>
          ) : snapshots.length === 0 ? (
            <p className="adv-empty">
              {zh
                ? "暂无快照（在该工作区运行至少一个回合后会自动生成）。"
                : "No snapshots yet (created automatically per turn)."}
            </p>
          ) : (
            <ul className="snap-list">
              {snapshots.map((s, i) => (
                <li key={s.id} className="snap-item">
                  <div className="snap-main">
                    <span className="snap-label">{s.label || "(no label)"}</span>
                    <span className="snap-meta">
                      #{i + 1} · {s.id.slice(0, 8)} · {fmtTime(s.timestamp)}
                    </span>
                  </div>
                  <button
                    type="button"
                    className="btn btn-mini"
                    disabled={busy}
                    onClick={() => void onRestore(s)}
                  >
                    {zh ? "还原" : "Restore"}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
