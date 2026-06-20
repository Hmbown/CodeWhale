// 网络策略面板（读写 config.toml [network]）

import { useCallback, useEffect, useState } from "react";
import type { Locale } from "../i18n";
import {
  getNetworkConfigFile,
  isTauri,
  restartBackend,
  saveNetworkConfigFile,
} from "../api/tauri";

interface NetworkPanelProps {
  locale: Locale;
}

const DEFAULT_OPTIONS = ["allow", "deny", "audit"];

/** 网络策略设置面板 */
export function NetworkPanel({ locale }: NetworkPanelProps) {
  const zh = locale === "zh";
  const [configPath, setConfigPath] = useState("");
  const [defaultPolicy, setDefaultPolicy] = useState("deny");
  const [allow, setAllow] = useState<string[]>([]);
  const [deny, setDeny] = useState<string[]>([]);
  const [audit, setAudit] = useState(true);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [newAllow, setNewAllow] = useState("");
  const [newDeny, setNewDeny] = useState("");

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    try {
      const data = await getNetworkConfigFile();
      setConfigPath(data.config_path);
      setDefaultPolicy(data.default || "deny");
      setAllow(data.allow ?? []);
      setDeny(data.deny ?? []);
      setAudit(data.audit ?? true);
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

  /** 保存网络策略并重启后端 */
  const persist = async (payload: {
    default: string;
    allow: string[];
    deny: string[];
    audit: boolean;
  }) => {
    setBusy(true);
    setMsg(zh ? "保存并重启…" : "Saving…");
    try {
      await saveNetworkConfigFile(payload);
      await restartBackend();
      setMsg(zh ? "已保存" : "Saved");
    } catch (e) {
      setMsg((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const current = () => ({
    default: defaultPolicy,
    allow,
    deny,
    audit,
  });

  if (!isTauri()) {
    return (
      <div className="settings-section">
        <p className="settings-section-desc">{zh ? "网络策略仅在桌面版可用。" : "Desktop only."}</p>
      </div>
    );
  }

  return (
    <div className="settings-section adv-settings">
      <h3 className="settings-section-title">{zh ? "网络" : "Network"}</h3>
      <p className="settings-section-desc">
        {zh
          ? "配置出站网络 default / allow / deny / audit（保存后重启后端）。"
          : "Outbound network policy."}
      </p>
      {configPath && <p className="cfg-tip">{configPath}</p>}
      {msg && <p className="settings-hint">{msg}</p>}

      {loading ? (
        <p>{zh ? "加载中…" : "Loading…"}</p>
      ) : (
        <>
          <div className="adv-form-row">
            <label className="cfg-label">{zh ? "默认策略" : "Default"}</label>
            <select
              className="cfg-input"
              value={defaultPolicy}
              onChange={(e) => setDefaultPolicy(e.target.value)}
            >
              {DEFAULT_OPTIONS.map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </select>
          </div>

          <label className="cfg-check adv-toggle">
            <input type="checkbox" checked={audit} onChange={(e) => setAudit(e.target.checked)} />
            audit
          </label>

          <div className="adv-dual">
            <div className="adv-half">
              <h4 className="adv-form-title">allow</h4>
              <ul className="adv-tags">
                {allow.map((a) => (
                  <li key={a}>
                    {a}
                    <button
                      type="button"
                      className="adv-tag-del"
                      onClick={() => setAllow(allow.filter((x) => x !== a))}
                    >
                      ×
                    </button>
                  </li>
                ))}
              </ul>
              <div className="adv-form-row">
                <input
                  className="cfg-input"
                  placeholder="host / pattern"
                  value={newAllow}
                  onChange={(e) => setNewAllow(e.target.value)}
                />
                <button
                  type="button"
                  className="btn btn-mini"
                  onClick={() => {
                    const v = newAllow.trim();
                    if (v && !allow.includes(v)) setAllow([...allow, v]);
                    setNewAllow("");
                  }}
                >
                  +
                </button>
              </div>
            </div>
            <div className="adv-half">
              <h4 className="adv-form-title">deny</h4>
              <ul className="adv-tags">
                {deny.map((d) => (
                  <li key={d}>
                    {d}
                    <button
                      type="button"
                      className="adv-tag-del"
                      onClick={() => setDeny(deny.filter((x) => x !== d))}
                    >
                      ×
                    </button>
                  </li>
                ))}
              </ul>
              <div className="adv-form-row">
                <input
                  className="cfg-input"
                  placeholder="host / pattern"
                  value={newDeny}
                  onChange={(e) => setNewDeny(e.target.value)}
                />
                <button
                  type="button"
                  className="btn btn-mini"
                  onClick={() => {
                    const v = newDeny.trim();
                    if (v && !deny.includes(v)) setDeny([...deny, v]);
                    setNewDeny("");
                  }}
                >
                  +
                </button>
              </div>
            </div>
          </div>

          <button
            type="button"
            className="btn btn-primary"
            disabled={busy}
            onClick={() => void persist(current())}
          >
            {zh ? "保存" : "Save"}
          </button>
        </>
      )}
    </div>
  );
}
