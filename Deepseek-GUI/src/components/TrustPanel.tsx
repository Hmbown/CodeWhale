// 工作区信任目录面板（对齐 TUI /trust list/add/remove）
// 信任路径允许 agent 的文件工具读写工作区之外的指定目录/文件；
// 按工作区独立存储于 ~/.deepseek/workspace-trust.json。

import { useCallback, useEffect, useState } from "react";
import type { Locale } from "../i18n";
import { addTrust, getTrust, isTauri, pickFolder, removeTrust } from "../api/tauri";

interface TrustPanelProps {
  locale: Locale;
  /** 当前工作区路径（信任列表按工作区分组） */
  workspace: string | null;
}

/**
 * 工作区信任目录管理面板。
 * @param locale 当前语言
 * @param workspace 当前工作区根目录
 */
export function TrustPanel({ locale, workspace }: TrustPanelProps) {
  const zh = locale === "zh";
  const [filePath, setFilePath] = useState("");
  const [items, setItems] = useState<string[]>([]);
  const [newPath, setNewPath] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  /** 加载当前工作区的信任列表 */
  const refresh = useCallback(async () => {
    if (!isTauri() || !workspace || !workspace.trim()) {
      setLoading(false);
      return;
    }
    try {
      const data = await getTrust(workspace);
      setFilePath(data.path);
      setItems(data.items);
      setMsg(null);
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [workspace]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 新增一条信任路径 */
  const onAdd = async (path: string) => {
    if (!workspace) return;
    const p = path.trim();
    if (!p) return;
    setBusy(true);
    try {
      await addTrust(workspace, p);
      await refresh();
      setNewPath("");
      setMsg(zh ? "已新增信任路径" : "Trusted path added");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  /** 移除一条信任路径 */
  const onRemove = async (path: string) => {
    if (!workspace) return;
    if (!window.confirm(zh ? `移除信任路径？\n${path}` : `Remove trusted path?\n${path}`)) return;
    setBusy(true);
    try {
      await removeTrust(workspace, path);
      await refresh();
      setMsg(zh ? "已移除" : "Removed");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  /** 通过原生对话框选择目录后加入信任 */
  const onPick = async () => {
    const dir = await pickFolder();
    if (dir) await onAdd(dir);
  };

  if (!isTauri()) {
    return (
      <div className="settings-section">
        <p className="settings-section-desc">{zh ? "信任目录仅在桌面版可用。" : "Desktop only."}</p>
      </div>
    );
  }

  if (!workspace || !workspace.trim()) {
    return (
      <div className="settings-section adv-settings">
        <h3 className="settings-section-title">{zh ? "信任目录" : "Trusted paths"}</h3>
        <p className="adv-empty">{zh ? "请先打开工作区文件夹" : "Open a workspace first"}</p>
      </div>
    );
  }

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">{zh ? "信任目录" : "Trusted paths"}</h3>
      <p className="settings-section-desc">
        {zh
          ? "允许 agent 的文件工具读写工作区之外的指定目录/文件；按工作区独立生效。"
          : "Allow file tools to access paths outside the workspace (per-workspace)."}
      </p>
      {filePath && <p className="cfg-tip">{filePath}</p>}
      {msg && <p className="settings-hint">{msg}</p>}

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : (
        <>
          {items.length === 0 ? (
            <p className="adv-empty">{zh ? "暂无信任路径" : "No trusted paths"}</p>
          ) : (
            <ul className="adv-list">
              {items.map((p) => (
                <li key={p} className="adv-list-item mem-entry">
                  <pre className="mem-entry-text">{p}</pre>
                  <button
                    type="button"
                    className="btn btn-mini"
                    disabled={busy}
                    onClick={() => void onRemove(p)}
                  >
                    {zh ? "移除" : "Remove"}
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div className="adv-form-row">
            <input
              className="cfg-input"
              placeholder={zh ? "目录或文件的绝对路径…" : "Absolute path…"}
              value={newPath}
              onChange={(e) => setNewPath(e.target.value)}
            />
            <button type="button" className="btn btn-mini" disabled={busy} onClick={() => void onAdd(newPath)}>
              +
            </button>
            <button type="button" className="btn btn-mini" disabled={busy} onClick={() => void onPick()}>
              {zh ? "选择目录" : "Pick"}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
