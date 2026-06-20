// 统一设置页（Cursor 风格）：左侧分类导航 + 右侧内容面板
// 整合模型配置、后端连接、任务、规则、技能、聊天偏好

import type { ReactElement } from "react";
import type { RuntimeClient } from "../api/client";
import { ConfigView } from "./ConfigView";
import { TasksView } from "./TasksView";
import { RulesView } from "./RulesView";
import { SkillsView } from "./SkillsView";
import { TabSettingsPanel } from "./TabSettingsPanel";
import { McpSettingsPanel } from "./McpSettingsPanel";
import { HooksPanel } from "./HooksPanel";
import { NetworkPanel } from "./NetworkPanel";
import { SubagentsPanel } from "./SubagentsPanel";
import { JobsPanel } from "./JobsPanel";
import { RlmPanel } from "./RlmPanel";
import { MemoryPanel } from "./MemoryPanel";
import { TrustPanel } from "./TrustPanel";
import { t, type Locale } from "../i18n";

/** 设置分类 id */
export type SettingsTab =
  | "models"
  | "connection"
  | "tab"
  | "tasks"
  | "jobs"
  | "subagents"
  | "rlm"
  | "terminal"
  | "memory"
  | "trust"
  | "rules"
  | "skills"
  | "mcp"
  | "hooks"
  | "network"
  | "chat";

interface SettingsViewProps {
  client: RuntimeClient;
  locale: Locale;
  tab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
  onBack: () => void;
  onSaved: () => void;
  rootPath: string | null;
  backendUp: boolean | null;
  formUrl: string;
  formToken: string;
  onFormUrlChange: (v: string) => void;
  onFormTokenChange: (v: string) => void;
  onApplyConnection: () => void;
  showArchived: boolean;
  onShowArchivedChange: (v: boolean) => void;
}

/** 导航项定义 */
const NAV: Array<{ id: SettingsTab; labelZh: string; labelEn: string; icon: ReactElement }> = [
  {
    id: "models",
    labelZh: "模型",
    labelEn: "Models",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M12 2a2 2 0 0 1 2 2c0 .74-.4 1.39-1 1.73V7h1a7 7 0 0 1 7 7h1a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-1v1a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-1H2a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1h1a7 7 0 0 1 7-7h1V5.73c-.6-.34-1-.99-1-1.73a2 2 0 0 1 2-2zm-5 8a5 5 0 0 0-5 5v2h18v-2a5 5 0 0 0-5-5H7z" />
      </svg>
    ),
  },
  {
    id: "connection",
    labelZh: "连接",
    labelEn: "Connection",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M7 2a5 5 0 0 0-5 5v2a3 3 0 0 0 3 3h1v6a2 2 0 0 0 2 2h8v-4h-2v2H8v-6h1a3 3 0 0 0 3-3V7a5 5 0 0 0-5-5zm0 2a3 3 0 0 1 3 3v2a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a3 3 0 0 1 3-3zm10 4h1a3 3 0 0 1 3 3v2a5 5 0 0 1-5 5h-1v2h2v2h-4v-6h1a3 3 0 0 0 3-3v-2a1 1 0 0 0-1-1h-1V8z" />
      </svg>
    ),
  },
  {
    id: "tab",
    labelZh: "Tab",
    labelEn: "Tab",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M4 5h16a2 2 0 0 1 2 2v3H2V7a2 2 0 0 1 2-2zm-2 8h20v6a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-6zm6 2v2h8v-2H8z" />
      </svg>
    ),
  },
  {
    id: "tasks",
    labelZh: "任务",
    labelEn: "Tasks",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M7 3h10a2 2 0 0 1 2 2v14l-4-2-4 2-4-2-4 2V5a2 2 0 0 1 2-2zm0 2v11.5l2-1 4 2 4-2 2 1V5H7zm2 2h6v2H9V7zm0 4h4v2H9v-2z" />
      </svg>
    ),
  },
  {
    id: "jobs",
    labelZh: "Jobs",
    labelEn: "Jobs",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M4 6h16v12H4V6zm2 2v8h12V8H6zm2 2h8v2H8v-2zm0 4h5v2H8v-2z" />
      </svg>
    ),
  },
  {
    id: "subagents",
    labelZh: "Subagents",
    labelEn: "Subagents",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M12 3a4 4 0 1 1 0 8 4 4 0 0 1 0-8zm-7 14a7 7 0 0 1 14 0v1H5v-1zm14-6a3 3 0 1 1 0 6 3 3 0 0 1 0-6z" />
      </svg>
    ),
  },
  {
    id: "rlm",
    labelZh: "RLM",
    labelEn: "RLM",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M4 4h16v2H4V4zm0 4h10v2H4V8zm0 4h16v2H4v-2zm0 4h8v2H4v-2z" />
      </svg>
    ),
  },
  {
    id: "memory",
    labelZh: "记忆",
    labelEn: "Memory",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M9 2a5 5 0 0 0-5 5v1.1A4 4 0 0 0 4 16v1a4 4 0 0 0 5 3.87V20a2 2 0 0 0 2 2 1 1 0 0 0 1-1V4a2 2 0 0 0-2-2H9zm6 0a2 2 0 0 0-2 2v17a1 1 0 0 0 1 1 2 2 0 0 0 2-2v-.13A4 4 0 0 0 21 17v-1a4 4 0 0 0 0-7.9V7a5 5 0 0 0-5-5h-1z" />
      </svg>
    ),
  },
  {
    id: "trust",
    labelZh: "信任",
    labelEn: "Trust",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M12 1 3 5v6c0 5.5 3.8 10.7 9 12 5.2-1.3 9-6.5 9-12V5l-9-4zm-1.2 14.5L7 11.7l1.4-1.4 2.4 2.4 4.8-4.8L17 9.3l-6.2 6.2z" />
      </svg>
    ),
  },
  {
    id: "rules",
    labelZh: "规则",
    labelEn: "Rules",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M6 2h9l5 5v15a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2zm8 1.5V8h4.5L14 3.5zM8 11h8v2H8v-2zm0 4h8v2H8v-2z" />
      </svg>
    ),
  },
  {
    id: "skills",
    labelZh: "技能",
    labelEn: "Skills",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M11.5 2a3.5 3.5 0 0 1 3.45 2.87L17 6.5V8h1.5A2.5 2.5 0 0 1 21 10.5V12h-2v8a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2v-8H3v-1.5A2.5 2.5 0 0 1 5.5 8H7V6.5l2.05-.63A3.5 3.5 0 0 1 11.5 2zm0 2a1.5 1.5 0 0 0-1.47 1.23L9.5 6.5V8h5V6.5l-.53-1.27A1.5 1.5 0 0 0 11.5 4zM7 12v8h10v-8H7z" />
      </svg>
    ),
  },
  {
    id: "mcp",
    labelZh: "MCP",
    labelEn: "MCP",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M12 2l8 4v6c0 5-3.5 9.5-8 10-4.5-.5-8-5-8-10V6l8-4zm0 2.2L6 7v5c0 3.8 2.6 7.2 6 7.8 3.4-.6 6-4 6-7.8V7l-6-2.8z" />
      </svg>
    ),
  },
  {
    id: "hooks",
    labelZh: "Hooks",
    labelEn: "Hooks",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M7 4a3 3 0 0 0-3 3v2h2V7a1 1 0 0 1 1-1h3V4H7zm10 0v2h3a1 1 0 0 1 1 1v2h2V7a3 3 0 0 0-3-3h-3zm-5 5a3 3 0 1 0 0 6 3 3 0 0 0 0-6zm-8 5v2a3 3 0 0 0 3 3h3v-2H7a1 1 0 0 1-1-1v-2H4zm16 0v2a1 1 0 0 1-1 1h-3v2h3a3 3 0 0 0 3-3v-2h-2z" />
      </svg>
    ),
  },
  {
    id: "network",
    labelZh: "网络",
    labelEn: "Network",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10 10-4.5 10-10S17.5 2 12 2zm-1 17.9A8 8 0 0 1 4.1 13H11v6.9zm2 0V13h6.9A8 8 0 0 1 13 19.9zM4.1 11A8 8 0 0 1 11 4.1V11H4.1zm9.9 0V4.1A8 8 0 0 1 19.9 11H14z" />
      </svg>
    ),
  },
  {
    id: "chat",
    labelZh: "聊天",
    labelEn: "Chat",
    icon: (
      <svg viewBox="0 0 24 24" aria-hidden>
        <path d="M4 4h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H8l-4 4V6a2 2 0 0 1 2-2zm2 4v2h12V8H6zm0 4v2h8v-2H6z" />
      </svg>
    ),
  },
];

/** 后端连接配置面板 */
function ConnectionPanel({
  locale,
  backendUp,
  formUrl,
  formToken,
  onFormUrlChange,
  onFormTokenChange,
  onApply,
}: {
  locale: Locale;
  backendUp: boolean | null;
  formUrl: string;
  formToken: string;
  onFormUrlChange: (v: string) => void;
  onFormTokenChange: (v: string) => void;
  onApply: () => void;
}) {
  return (
    <div className="settings-section">
      <h3 className="settings-section-title">
        {locale === "zh" ? "后端连接" : "Backend connection"}
      </h3>
      <p className="settings-section-desc">
        {locale === "zh"
          ? "本地 HTTP 运行时地址与鉴权 Token（桌面版由壳层自动注入 Token）。"
          : "Local HTTP runtime URL and auth token."}
      </p>
      <div className="settings-status-row">
        <span className="settings-status-label">{locale === "zh" ? "状态" : "Status"}</span>
        <span className={backendUp ? "conn-ok" : "conn-bad"}>
          {backendUp ? t("app.backendOnline", locale) : t("app.backendOfflineShort", locale)}
        </span>
      </div>
      <label className="cfg-field">
        <span className="cfg-label">Base URL</span>
        <input
          className="cfg-input"
          placeholder="http://127.0.0.1:7878"
          value={formUrl}
          onChange={(e) => onFormUrlChange(e.target.value)}
        />
      </label>
      <label className="cfg-field">
        <span className="cfg-label">Token</span>
        <input
          className="cfg-input"
          type="password"
          placeholder="dev-local-token"
          value={formToken}
          onChange={(e) => onFormTokenChange(e.target.value)}
        />
      </label>
      <div className="config-actions">
        <button type="button" className="btn btn-primary" onClick={onApply}>
          {locale === "zh" ? "保存并连接" : "Save & connect"}
        </button>
      </div>
    </div>
  );
}

/** 聊天偏好面板 */
function ChatPrefsPanel({
  locale,
  showArchived,
  onShowArchivedChange,
}: {
  locale: Locale;
  showArchived: boolean;
  onShowArchivedChange: (v: boolean) => void;
}) {
  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{locale === "zh" ? "聊天" : "Chat"}</h3>
      <label className="cfg-check settings-pref-row">
        <input
          type="checkbox"
          checked={showArchived}
          onChange={(e) => onShowArchivedChange(e.target.checked)}
        />
        <span>{locale === "zh" ? "在标签栏显示已归档会话" : "Show archived chats in tab bar"}</span>
      </label>
    </div>
  );
}

/**
 * 设置主界面：在中间主区域展示（Cursor 风格左侧分类导航 + 右侧内容面板）
 */
export function SettingsView({
  client,
  locale,
  tab,
  onTabChange,
  onBack,
  onSaved,
  rootPath,
  backendUp,
  formUrl,
  formToken,
  onFormUrlChange,
  onFormTokenChange,
  onApplyConnection,
  showArchived,
  onShowArchivedChange,
}: SettingsViewProps) {
  const label = (item: (typeof NAV)[number]) => (locale === "zh" ? item.labelZh : item.labelEn);

  return (
    <div className="settings-shell">
      <div className="settings-topbar">
        <h2 className="settings-title">{t("status.settings", locale)}</h2>
        <button type="button" className="settings-close" onClick={onBack} title={locale === "zh" ? "关闭" : "Close"}>
          ×
        </button>
      </div>
      <div className="settings-body">
        <nav className="settings-nav" aria-label={t("status.settings", locale)}>
          {NAV.map((item) => (
            <button
              key={item.id}
              type="button"
              className={`settings-nav-item${tab === item.id ? " active" : ""}`}
              onClick={() => onTabChange(item.id)}
            >
              <span className="settings-nav-icon">{item.icon}</span>
              <span className="settings-nav-label">{label(item)}</span>
            </button>
          ))}
        </nav>
        {/* 右侧内容区：内层容器居中并四周留白，各 Tab 共享统一间距 */}
        <div className="settings-panel">
          <div className="settings-panel-inner">
            {tab === "models" && <ConfigView embedded onBack={onBack} onSaved={onSaved} />}
            {tab === "connection" && (
              <ConnectionPanel
                locale={locale}
                backendUp={backendUp}
                formUrl={formUrl}
                formToken={formToken}
                onFormUrlChange={onFormUrlChange}
                onFormTokenChange={onFormTokenChange}
                onApply={onApplyConnection}
              />
            )}
            {tab === "tab" && <TabSettingsPanel locale={locale} />}
            {tab === "tasks" && (
              <TasksView client={client} defaultWorkspace={rootPath} embedded onBack={onBack} />
            )}
            {tab === "jobs" && <JobsPanel client={client} locale={locale} />}
            {tab === "subagents" && (
              <SubagentsPanel client={client} locale={locale} workspace={rootPath ?? ""} />
            )}
            {tab === "rlm" && <RlmPanel client={client} locale={locale} />}
            {tab === "memory" && <MemoryPanel locale={locale} workspace={rootPath} />}
            {tab === "trust" && <TrustPanel locale={locale} workspace={rootPath} />}
            {tab === "rules" && <RulesView rootPath={rootPath} embedded onBack={onBack} />}
            {tab === "skills" && (
              <SkillsView client={client} embedded onBack={onBack} onOpenMcpSettings={() => onTabChange("mcp")} />
            )}
            {tab === "mcp" && <McpSettingsPanel locale={locale} />}
            {tab === "hooks" && <HooksPanel locale={locale} />}
            {tab === "network" && <NetworkPanel locale={locale} />}
            {tab === "chat" && (
              <ChatPrefsPanel
                locale={locale}
                showArchived={showArchived}
                onShowArchivedChange={onShowArchivedChange}
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
