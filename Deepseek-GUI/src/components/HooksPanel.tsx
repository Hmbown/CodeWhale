// Hooks 配置面板（读写 config.toml [hooks]）

import { useCallback, useEffect, useState } from "react";
import type { Locale } from "../i18n";
import {
  getHooksConfigFile,
  isTauri,
  restartBackend,
  saveHooksConfigFile,
} from "../api/tauri";

interface HooksPanelProps {
  locale: Locale;
}

/** 单条 hook 定义 */
interface HookRow {
  event: string;
  command: string;
  name?: string;
}

/** Hooks 设置面板 */
export function HooksPanel({ locale }: HooksPanelProps) {
  const zh = locale === "zh";
  const [configPath, setConfigPath] = useState("");
  const [enabled, setEnabled] = useState(false);
  const [hooks, setHooks] = useState<HookRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [jsonMode, setJsonMode] = useState(false);
  const [jsonText, setJsonText] = useState("[]");

  const [newEvent, setNewEvent] = useState("before_tool_call");
  const [newCommand, setNewCommand] = useState("");
  const [newName, setNewName] = useState("");

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      const data = await getHooksConfigFile();
      setConfigPath(data.config_path);
      setEnabled(data.enabled);
      const list = (Array.isArray(data.hooks) ? data.hooks : []) as HookRow[];
      setHooks(list);
      setJsonText(JSON.stringify(list, null, 2));
      setMsg(null);
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /** 保存 hooks 并重启后端 */
  const persist = async (nextEnabled: boolean, nextHooks: HookRow[]) => {
    setBusy(true);
    setMsg(zh ? "保存并重启…" : "Saving…");
    try {
      await saveHooksConfigFile(nextEnabled, nextHooks);
      await restartBackend();
      setHooks(nextHooks);
      setJsonText(JSON.stringify(nextHooks, null, 2));
      setMsg(zh ? "已保存" : "Saved");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const onSaveJson = () => {
    try {
      const parsed = JSON.parse(jsonText) as HookRow[];
      if (!Array.isArray(parsed)) throw new Error("must be array");
      void persist(enabled, parsed);
    } catch (e) {
      setMsg((e as Error).message);
    }
  };

  const onAdd = () => {
    const cmd = newCommand.trim();
    if (!cmd) return;
    const row: HookRow = {
      event: newEvent.trim() || "before_tool_call",
      command: cmd,
      name: newName.trim() || undefined,
    };
    void persist(enabled, [...hooks, row]);
    setNewCommand("");
    setNewName("");
  };

  if (!isTauri()) {
    return (
      <div className="settings-section">
        <p className="settings-section-desc">{zh ? "Hooks 仅在桌面版可用。" : "Desktop only."}</p>
      </div>
    );
  }

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">Hooks</h3>
      <p className="settings-section-desc">
        {zh
          ? "编辑 config.toml [hooks]（保存后重启后端）。"
          : "Edit [hooks] in config.toml."}
      </p>
      {configPath && <p className="cfg-tip">{configPath}</p>}
      {msg && <p className="settings-hint">{msg}</p>}

      <label className="cfg-check adv-toggle">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => void persist(e.target.checked, hooks)}
          disabled={busy}
        />
        {zh ? "启用 Hooks" : "Enable hooks"}
      </label>

      <div className="adv-toolbar">
        <button type="button" className="btn btn-mini" onClick={() => setJsonMode(!jsonMode)}>
          {jsonMode ? (zh ? "表单" : "Form") : "JSON"}
        </button>
        <button type="button" className="btn btn-mini" disabled={busy} onClick={() => void refresh()}>
          {zh ? "刷新" : "Refresh"}
        </button>
      </div>

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : jsonMode ? (
        <div className="adv-json">
          <textarea
            className="cfg-textarea"
            rows={12}
            value={jsonText}
            onChange={(e) => setJsonText(e.target.value)}
          />
          <button type="button" className="btn btn-primary" disabled={busy} onClick={onSaveJson}>
            {zh ? "保存 JSON" : "Save JSON"}
          </button>
        </div>
      ) : (
        <>
          <ul className="adv-list">
            {hooks.length === 0 && <li className="adv-empty">{zh ? "暂无 Hook" : "No hooks"}</li>}
            {hooks.map((h, i) => (
              <li key={`${h.event}-${i}`} className="adv-list-item">
                <div className="adv-list-main">
                  <strong>{h.name ?? h.event}</strong>
                  <span className="adv-list-meta">
                    {h.event} → {h.command}
                  </span>
                </div>
                <button
                  type="button"
                  className="btn btn-mini btn-danger"
                  onClick={() => void persist(enabled, hooks.filter((_, j) => j !== i))}
                >
                  {zh ? "删除" : "Del"}
                </button>
              </li>
            ))}
          </ul>
          <div className="adv-form">
            <h4 className="adv-form-title">{zh ? "添加 Hook" : "Add hook"}</h4>
            <input
              className="cfg-input"
              placeholder="event"
              value={newEvent}
              onChange={(e) => setNewEvent(e.target.value)}
            />
            <input
              className="cfg-input"
              placeholder="command"
              value={newCommand}
              onChange={(e) => setNewCommand(e.target.value)}
            />
            <input
              className="cfg-input"
              placeholder={zh ? "名称（可选）" : "name (optional)"}
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
            />
            <button type="button" className="btn btn-primary" disabled={busy} onClick={onAdd}>
              {zh ? "添加" : "Add"}
            </button>
          </div>
        </>
      )}
    </div>
  );
}
