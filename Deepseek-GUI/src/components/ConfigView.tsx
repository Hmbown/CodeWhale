// 模型配置页面：多供应商档案管理（卡片展示 + 新增/编辑/删除/设为使用中）
// 桌面应用内：档案存于 ~/.deepseek/gui_profiles.json；「使用中」档案会写入
// config.toml 并重启后端使其生效。另含「运行行为」全局开关。
// 纯浏览器环境：降级为手动配置指引。

import { useCallback, useEffect, useState } from "react";
import {
  activateProfile,
  deleteProfile,
  getConfig,
  isTauri,
  listProfiles,
  restartBackend,
  saveConfig,
  testConnection,
  upsertProfile,
  type AppConfig,
  type ProfileInfo,
  type ProfileInput,
} from "../api/tauri";
import {
  findPreset,
  presetKeyForForm,
  PROVIDER_PRESETS,
  type ProviderPreset,
} from "../utils/providerPresets";
import { REASONING_EFFORT_OPTIONS } from "../utils/reasoningEffort";

interface ConfigViewProps {
  /** 返回对话界面（嵌入设置页时可省略顶栏返回） */
  onBack: () => void;
  /** 配置生效后回调，便于刷新连接状态 */
  onSaved: () => void;
  /** 嵌入统一设置页：隐藏顶栏返回按钮 */
  embedded?: boolean;
}

/** 推理强度可选项（与 reasoningEffort.ts 共享） */
const EFFORT_OPTIONS = [...REASONING_EFFORT_OPTIONS];

/** 当前表单对应的预设（用于模型候选与 Key 提示） */
function activePresetForForm(form: FormState): ProviderPreset {
  const key = presetKeyForForm(form.provider, form.base_url);
  return findPreset(key) ?? findPreset("deepseek")!;
}

/** 切换服务商预设：同步 provider、Base URL、默认模型 */
function applyProviderPreset(form: FormState, presetKey: string): FormState {
  const preset = findPreset(presetKey);
  if (!preset) return form;
  return {
    ...form,
    provider: preset.providerId,
    base_url: preset.baseUrl,
    model: preset.defaultModel || form.model,
  };
}

/** 已配置 Key 在表单中的占位掩码（展示用，非真实 Key） */
const KEY_MASK = "********";

/** 编辑表单的数据形态 */
interface FormState {
  id?: string;
  name: string;
  provider: string;
  base_url: string;
  model: string;
  api_key: string;
  /** 是否已有 Key（编辑态：掩码未改动时不覆盖原 Key） */
  keyPresent: boolean;
}

/** 判断 API Key 输入是否仍为「未修改的掩码」 */
function isKeyMaskUnchanged(form: FormState): boolean {
  return form.keyPresent && (form.api_key === KEY_MASK || form.api_key.trim() === "");
}

/** 解析用于保存/测试的 Key：掩码未改动时不提交 */
function resolveApiKeyForSubmit(form: FormState): string | undefined {
  if (isKeyMaskUnchanged(form)) return undefined;
  const trimmed = form.api_key.trim();
  return trimmed || undefined;
}

/** 新增时的默认表单 */
function emptyForm(): FormState {
  return {
    name: "",
    provider: "deepseek",
    base_url: "https://api.deepseek.com",
    model: "deepseek-v4-pro",
    api_key: "",
    keyPresent: false,
  };
}

export function ConfigView({ onBack, onSaved, embedded }: ConfigViewProps) {
  const tauri = isTauri();

  // 档案列表与当前使用中 id
  const [profiles, setProfiles] = useState<ProfileInfo[]>([]);
  const [activeId, setActiveId] = useState("");
  // 编辑表单（null 表示未在编辑）
  const [form, setForm] = useState<FormState | null>(null);

  // 全局运行行为
  const [effort, setEffort] = useState("");
  const [allowShell, setAllowShell] = useState(false);
  const [configPath, setConfigPath] = useState("");

  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  // 表单内测试连接状态
  const [testState, setTestState] = useState<"idle" | "testing" | "ok" | "fail">("idle");
  const [testMsg, setTestMsg] = useState("");

  /** 刷新档案列表 */
  const refreshProfiles = useCallback(async () => {
    const r = await listProfiles();
    setProfiles(r.profiles);
    setActiveId(r.active_id);
  }, []);

  // 进入时加载档案与全局配置
  useEffect(() => {
    refreshProfiles().catch(() => {
      /* 忽略 */
    });
    getConfig()
      .then((c: AppConfig | null) => {
        if (!c) return;
        setConfigPath(c.config_path);
        setEffort(c.reasoning_effort ?? "");
        setAllowShell(Boolean(c.allow_shell));
      })
      .catch(() => {
        /* 忽略 */
      });
  }, [refreshProfiles]);

  /** 当前使用中档案是否有可用 Key（决定「AI 已启用」状态） */
  const aiEnabled = profiles.some((p) => p.id === activeId && p.key_present);

  /** 打开新增表单 */
  const openNew = () => {
    setTestState("idle");
    setTestMsg("");
    setMsg("");
    setForm(emptyForm());
  };

  /** 打开编辑表单 */
  const openEdit = (p: ProfileInfo) => {
    setTestState("idle");
    setTestMsg("");
    setMsg("");
    setForm({
      id: p.id,
      name: p.name,
      provider: p.provider,
      base_url: p.base_url,
      model: p.model,
      // 已配置 Key 时展示掩码，避免编辑框看起来为空
      api_key: p.key_present ? KEY_MASK : "",
      keyPresent: p.key_present,
    });
  };

  /** 表单内测试连接 */
  const onTest = async () => {
    if (!form) return;
    setTestState("testing");
    setTestMsg("正在测试连接…");
    try {
      const res = await testConnection(
        resolveApiKeyForSubmit(form),
        form.base_url.trim() || undefined,
      );
      setTestState("ok");
      setTestMsg(res);
    } catch (e) {
      setTestState("fail");
      setTestMsg((e as Error).message);
    }
  };

  /** 保存档案：写入档案库；若保存的是使用中档案则同步到 config.toml 并重启后端 */
  const onSaveProfile = async () => {
    if (!form) return;
    setBusy(true);
    setMsg("正在保存配置…");
    try {
      const input: ProfileInput = {
        id: form.id,
        name: form.name,
        provider: form.provider,
        base_url: form.base_url,
        model: form.model,
      };
      const keyForSave = resolveApiKeyForSubmit(form);
      if (keyForSave) input.api_key = keyForSave;

      const savedId = await upsertProfile(input);
      const r = await listProfiles();
      setProfiles(r.profiles);
      setActiveId(r.active_id);

      // 若保存的恰是「使用中」档案（含首个自动设为使用中的情况），同步并重启后端
      if (r.active_id === savedId) {
        setMsg("正在应用配置并重启后端…");
        await activateProfile(savedId);
        await new Promise((res) => setTimeout(res, 1500));
        onSaved();
      }
      setMsg("已保存");
      setForm(null);
    } catch (e) {
      setMsg(`失败：${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  /** 设为使用中 */
  const onActivate = async (id: string) => {
    setBusy(true);
    setMsg("正在切换并重启后端…");
    try {
      await activateProfile(id);
      await new Promise((res) => setTimeout(res, 1500));
      await refreshProfiles();
      setMsg("已切换为使用中");
      onSaved();
    } catch (e) {
      setMsg(`失败：${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  /** 删除档案 */
  const onDelete = async (p: ProfileInfo) => {
    if (!confirm(`确定删除配置「${p.name}」？`)) return;
    setBusy(true);
    try {
      await deleteProfile(p.id);
      await refreshProfiles();
      setMsg("已删除");
    } catch (e) {
      setMsg(`删除失败：${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  /** 保存全局运行行为并重启后端 */
  const onSaveBehavior = async () => {
    setBusy(true);
    setMsg("正在保存并重启后端…");
    try {
      await saveConfig({ reasoning_effort: effort, allow_shell: allowShell });
      await restartBackend();
      await new Promise((res) => setTimeout(res, 1500));
      setMsg("已保存并生效");
      onSaved();
    } catch (e) {
      setMsg(`失败：${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`config-view${embedded ? " config-view-embedded" : ""}`}>
      {!embedded && (
      <div className="config-head">
        <button className="btn" onClick={onBack}>
          ← 返回
        </button>
        <h2 className="config-title">配置</h2>
      </div>
      )}

      {!tauri && (
        <div className="config-card">
          <div className="settings-hint">
            当前在浏览器中运行，无法直接写配置。请在后端环境设置环境变量
            <code>DEEPSEEK_API_KEY</code> 或编辑 <code>~/.deepseek/config.toml</code>，
            然后重启 <code>deepseek serve --http</code>。
          </div>
        </div>
      )}

      {tauri && (
        <div className="config-card">
          <h3 className="config-section">模型配置</h3>
          <div className="cfg-status">
            状态：
            <strong className={aiEnabled ? "conn-ok" : "conn-bad"}>
              {aiEnabled ? "AI 已启用" : "未启用"}
            </strong>
          </div>

          {/* 档案卡片列表 */}
          <div className="profile-grid">
            {profiles.map((p) => (
              <div
                key={p.id}
                className={p.id === activeId ? "profile-card active" : "profile-card"}
              >
                <div className="profile-card-head">
                  <span className="profile-name">{p.name}</span>
                  {p.id === activeId ? (
                    <span className="badge badge-active">使用中</span>
                  ) : (
                    <button
                      className="badge badge-set"
                      disabled={busy}
                      onClick={() => onActivate(p.id)}
                    >
                      设为使用中
                    </button>
                  )}
                </div>
                <div className="profile-model">{p.model || "（未指定模型）"}</div>
                <div className="profile-key">{p.key_present ? "********" : "（无 Key）"}</div>
                <div className="profile-actions">
                  <button className="btn-mini" disabled={busy} onClick={() => openEdit(p)}>
                    编辑
                  </button>
                  <button className="btn-mini" disabled={busy} onClick={() => onDelete(p)}>
                    删除
                  </button>
                </div>
              </div>
            ))}

            {!form && (
              <button className="profile-card profile-add" disabled={busy} onClick={openNew}>
                ＋ 新增配置
              </button>
            )}
          </div>

          {/* 新增 / 编辑表单 */}
          {form && (() => {
            const presetKey = presetKeyForForm(form.provider, form.base_url);
            const preset = activePresetForForm(form);
            return (
            <div className="profile-form">
              <h4 className="config-section">{form.id ? "编辑配置" : "新增配置"}</h4>

              <label className="cfg-field">
                <span className="cfg-label">配置名称</span>
                <input
                  className="cfg-input"
                  placeholder="如：DeepSeek 主号"
                  value={form.name}
                  disabled={busy}
                  onChange={(e) => setForm({ ...form, name: e.target.value })}
                />
              </label>

              <label className="cfg-field">
                <span className="cfg-label">服务商</span>
                <select
                  className="cfg-input"
                  value={presetKey}
                  disabled={busy}
                  onChange={(e) => setForm(applyProviderPreset(form, e.target.value))}
                >
                  {PROVIDER_PRESETS.map((p) => (
                    <option key={p.key} value={p.key}>
                      {p.label}
                    </option>
                  ))}
                </select>
              </label>

              <label className="cfg-field">
                <span className="cfg-label">
                  API Key
                  <em className={form.keyPresent ? "conn-ok" : "conn-bad"}>
                    {form.keyPresent ? "（已配置）" : "（未配置）"}
                  </em>
                </span>
                <input
                  className="cfg-input"
                  type={form.keyPresent && form.api_key === KEY_MASK ? "text" : "password"}
                  placeholder={form.keyPresent ? "输入新 Key 将覆盖原值" : "粘贴 sk-... 开头的 Key"}
                  value={form.api_key}
                  disabled={busy}
                  onChange={(e) => setForm({ ...form, api_key: e.target.value })}
                  onFocus={() => {
                    // 聚焦时清空掩码，便于粘贴新 Key
                    if (form.keyPresent && form.api_key === KEY_MASK) {
                      setForm({ ...form, api_key: "" });
                    }
                  }}
                  onBlur={() => {
                    // 未输入新 Key 时恢复掩码展示
                    if (form.keyPresent && !form.api_key.trim()) {
                      setForm({ ...form, api_key: KEY_MASK });
                    }
                  }}
                />
                <span className="cfg-tip">
                  {preset.keyHint
                    ? `申请地址：${preset.keyHint}`
                    : "申请地址：platform.deepseek.com/api_keys"}
                </span>
              </label>

              <label className="cfg-field">
                <span className="cfg-label">API Base URL</span>
                <input
                  className="cfg-input"
                  placeholder="https://api.deepseek.com"
                  value={form.base_url}
                  disabled={busy}
                  onChange={(e) => setForm({ ...form, base_url: e.target.value })}
                />
                <span className="cfg-tip">
                  {preset.urlHint ||
                    "DeepSeek 默认 https://api.deepseek.com；Ollama 默认 http://localhost:11434/v1"}
                </span>
              </label>

              <label className="cfg-field">
                <span className="cfg-label">模型名称</span>
                <input
                  className="cfg-input"
                  list="model-options"
                  placeholder="deepseek-v4-pro"
                  value={form.model}
                  disabled={busy}
                  onChange={(e) => setForm({ ...form, model: e.target.value })}
                />
                <datalist id="model-options">
                  {preset.models.map((m) => (
                    <option key={m} value={m} />
                  ))}
                </datalist>
              </label>

              <div className="cfg-test">
                <button className="btn" onClick={onTest} disabled={busy || testState === "testing"}>
                  {testState === "testing" ? "测试中…" : "测试连接"}
                </button>
                {testMsg && (
                  <span
                    className={
                      testState === "ok"
                        ? "conn-ok"
                        : testState === "fail"
                          ? "conn-bad"
                          : "cfg-tip"
                    }
                  >
                    {testState === "ok" ? "✓ " : testState === "fail" ? "✕ " : ""}
                    {testMsg}
                  </span>
                )}
              </div>

              <div className="config-actions">
                <button className="btn" onClick={() => setForm(null)} disabled={busy}>
                  取消
                </button>
                <button className="btn btn-primary" onClick={onSaveProfile} disabled={busy}>
                  保存配置
                </button>
              </div>
            </div>
            );
          })()}
        </div>
      )}

      {tauri && (
        <div className="config-card">
          <h3 className="config-section">运行行为</h3>
          <label className="cfg-field">
            <span className="cfg-label">推理强度</span>
            <select
              className="cfg-input"
              value={effort}
              disabled={busy}
              onChange={(e) => setEffort(e.target.value)}
            >
              <option value="">（默认）</option>
              {EFFORT_OPTIONS.map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </select>
          </label>
          <label className="cfg-check">
            <input
              type="checkbox"
              checked={allowShell}
              disabled={busy}
              onChange={(e) => setAllowShell(e.target.checked)}
            />
            默认允许执行 Shell 命令（allow_shell）
          </label>
          <div className="config-actions">
            <button className="btn btn-primary" onClick={onSaveBehavior} disabled={busy}>
              保存并生效
            </button>
          </div>
        </div>
      )}

      <div className="config-meta">
        配置文件：<code>{configPath || "~/.deepseek/config.toml"}</code>
      </div>

      {msg && <div className="settings-msg">{msg}</div>}
    </div>
  );
}
