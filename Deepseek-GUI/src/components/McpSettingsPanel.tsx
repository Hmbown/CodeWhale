// MCP 服务器完整管理（读写 mcp.json，仿 TUI /mcp 命令）

import { useCallback, useEffect, useState } from "react";
import type { Locale } from "../i18n";
import {
  getMcpConfigFile,
  initMcpConfigFile,
  isTauri,
  restartBackend,
  saveMcpConfigFile,
} from "../api/tauri";

interface McpSettingsPanelProps {
  locale: Locale;
}

/** 单条 MCP 服务器（mcp.json servers 表项） */
interface McpServerRow {
  name: string;
  command?: string;
  args: string[];
  url?: string;
  enabled: boolean;
}

/** 从 JSON 文档解析服务器列表 */
function parseServers(doc: Record<string, unknown>): McpServerRow[] {
  const raw = (doc.servers ?? doc.mcpServers) as Record<string, Record<string, unknown>> | undefined;
  if (!raw || typeof raw !== "object") return [];
  return Object.entries(raw).map(([name, cfg]) => ({
    name,
    command: typeof cfg.command === "string" ? cfg.command : undefined,
    args: Array.isArray(cfg.args) ? cfg.args.map(String) : [],
    url: typeof cfg.url === "string" ? cfg.url : undefined,
    enabled: cfg.disabled === true ? false : cfg.enabled !== false,
  }));
}

/** 列表转回 servers 对象 */
function serversToObject(rows: McpServerRow[]): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const r of rows) {
    if (r.url) {
      out[r.name] = { url: r.url, enabled: r.enabled, disabled: !r.enabled };
    } else {
      out[r.name] = {
        command: r.command ?? "",
        args: r.args,
        enabled: r.enabled,
        disabled: !r.enabled,
      };
    }
  }
  return out;
}

/** MCP 设置面板 */
export function McpSettingsPanel({ locale }: McpSettingsPanelProps) {
  const zh = locale === "zh";
  const [path, setPath] = useState("");
  const [rows, setRows] = useState<McpServerRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  // 新建 stdio / http 表单
  const [addMode, setAddMode] = useState<"stdio" | "http">("stdio");
  const [newName, setNewName] = useState("");
  const [newCommand, setNewCommand] = useState("");
  const [newArgs, setNewArgs] = useState("");
  const [newUrl, setNewUrl] = useState("");

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      const doc = await getMcpConfigFile();
      setPath(String(doc.path ?? ""));
      setRows(parseServers(doc));
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

  /** 持久化并重启后端使 MCP 生效 */
  const persist = async (nextRows: McpServerRow[]) => {
    setBusy(true);
    setMsg(zh ? "保存并重启后端…" : "Saving & restarting…");
    try {
      const doc = await getMcpConfigFile();
      const base = { ...doc };
      delete base.path;
      delete base.exists;
      base.servers = serversToObject(nextRows);
      await saveMcpConfigFile(base);
      await restartBackend();
      setRows(nextRows);
      setMsg(zh ? "已保存" : "Saved");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const onInit = async () => {
    if (!window.confirm(zh ? "创建空 mcp.json？" : "Create empty mcp.json?")) return;
    setBusy(true);
    try {
      await initMcpConfigFile(false);
      await refresh();
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const onAdd = () => {
    const name = newName.trim();
    if (!name) return;
    if (rows.some((r) => r.name === name)) {
      alert(zh ? "名称已存在" : "Name exists");
      return;
    }
    const row: McpServerRow =
      addMode === "http"
        ? { name, url: newUrl.trim(), args: [], enabled: true }
        : {
            name,
            command: newCommand.trim(),
            args: newArgs.trim() ? newArgs.trim().split(/\s+/) : [],
            enabled: true,
          };
    void persist([...rows, row]);
    setNewName("");
    setNewCommand("");
    setNewArgs("");
    setNewUrl("");
  };

  if (!isTauri()) {
    return (
      <div className="settings-section">
        <p className="settings-section-desc">{zh ? "MCP 管理仅在桌面版可用。" : "Desktop only."}</p>
      </div>
    );
  }

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">MCP</h3>
      <p className="settings-section-desc">
        {zh
          ? "管理 mcp.json 中的 MCP 服务器（保存后自动重启后端）。运行时状态见「技能/MCP」页。"
          : "Manage MCP servers in mcp.json (restarts backend on save)."}
      </p>
      {path && <p className="cfg-tip">{path}</p>}
      {msg && <p className="settings-hint">{msg}</p>}

      <div className="adv-toolbar">
        <button type="button" className="btn btn-mini" disabled={busy} onClick={() => void onInit()}>
          {zh ? "初始化" : "Init"}
        </button>
        <button type="button" className="btn btn-mini" disabled={busy} onClick={() => void refresh()}>
          {zh ? "刷新" : "Refresh"}
        </button>
      </div>

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : (
        <ul className="adv-list">
          {rows.length === 0 && <li className="adv-empty">{zh ? "暂无服务器" : "No servers"}</li>}
          {rows.map((r) => (
            <li key={r.name} className="adv-list-item">
              <div className="adv-list-main">
                <strong>{r.name}</strong>
                <span className="adv-list-meta">
                  {r.url ? `HTTP ${r.url}` : `stdio ${r.command} ${r.args.join(" ")}`}
                </span>
              </div>
              <div className="adv-list-actions">
                <label className="cfg-check">
                  <input
                    type="checkbox"
                    checked={r.enabled}
                    onChange={(e) => {
                      const next = rows.map((x) =>
                        x.name === r.name ? { ...x, enabled: e.target.checked } : x,
                      );
                      void persist(next);
                    }}
                  />
                  {zh ? "启用" : "On"}
                </label>
                <button
                  type="button"
                  className="btn btn-mini btn-danger"
                  onClick={() => {
                    if (!window.confirm(`${zh ? "删除" : "Remove"} ${r.name}?`)) return;
                    void persist(rows.filter((x) => x.name !== r.name));
                  }}
                >
                  {zh ? "删除" : "Del"}
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      <div className="adv-form">
        <h4 className="adv-form-title">{zh ? "添加服务器" : "Add server"}</h4>
        <div className="adv-form-row">
          <select className="cfg-input" value={addMode} onChange={(e) => setAddMode(e.target.value as "stdio" | "http")}>
            <option value="stdio">stdio</option>
            <option value="http">http</option>
          </select>
          <input
            className="cfg-input"
            placeholder={zh ? "名称" : "name"}
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
          />
        </div>
        {addMode === "stdio" ? (
          <>
            <input
              className="cfg-input"
              placeholder="command"
              value={newCommand}
              onChange={(e) => setNewCommand(e.target.value)}
            />
            <input
              className="cfg-input"
              placeholder="args（空格分隔）"
              value={newArgs}
              onChange={(e) => setNewArgs(e.target.value)}
            />
          </>
        ) : (
          <input
            className="cfg-input"
            placeholder="https://..."
            value={newUrl}
            onChange={(e) => setNewUrl(e.target.value)}
          />
        )}
        <button type="button" className="btn btn-primary" disabled={busy} onClick={onAdd}>
          {zh ? "添加" : "Add"}
        </button>
      </div>
    </div>
  );
}
