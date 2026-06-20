// 技能 / MCP 界面：
// - 技能（Skills）：列出技能目录下发现的技能，可逐个开关（POST /v1/skills/{name}）。
// - MCP 服务器：只读展示已配置服务器的连接状态、是否启用、命令/URL 与可用工具数
//   （后端未提供 MCP 启停接口，故此处仅作可视化）。

import { useCallback, useEffect, useState } from "react";
import type { RuntimeClient } from "../api/client";
import type { McpServerEntry, McpToolEntry, SkillEntry } from "../api/types";

interface SkillsViewProps {
  client: RuntimeClient;
  onBack: () => void;
  /** 嵌入统一设置页：隐藏顶栏返回 */
  embedded?: boolean;
  /** 跳转 MCP 管理 Tab */
  onOpenMcpSettings?: () => void;
}

/**
 * 技能 / MCP 主界面。
 * @param client 运行时 API 客户端
 * @param onBack 返回聊天界面回调
 */
export function SkillsView({ client, onBack, embedded, onOpenMcpSettings }: SkillsViewProps) {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [skillDir, setSkillDir] = useState<string>("");
  const [warnings, setWarnings] = useState<string[]>([]);
  const [servers, setServers] = useState<McpServerEntry[]>([]);
  const [mcpTools, setMcpTools] = useState<McpToolEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // 正在切换中的技能名（防抖：禁用对应开关）
  const [busy, setBusy] = useState<string | null>(null);

  /** 拉取技能与 MCP 服务器列表 */
  const refresh = useCallback(async () => {
    try {
      const [sk, mcp, tools] = await Promise.all([
        client.listSkills(),
        client.listMcpServers().catch(() => ({ servers: [] })),
        client.listMcpTools().catch(() => ({ tools: [] })),
      ]);
      setSkills(sk.skills);
      setSkillDir(sk.directory);
      setWarnings(sk.warnings ?? []);
      setServers(mcp.servers);
      setMcpTools(tools.tools);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  /** 切换技能启用状态 */
  const onToggle = useCallback(
    async (name: string, next: boolean) => {
      setBusy(name);
      // 乐观更新，失败时回滚
      setSkills((prev) =>
        prev.map((s) => (s.name === name ? { ...s, enabled: next } : s)),
      );
      try {
        await client.setSkillEnabled(name, next);
      } catch (e) {
        setSkills((prev) =>
          prev.map((s) => (s.name === name ? { ...s, enabled: !next } : s)),
        );
        alert(`切换失败：${(e as Error).message}`);
      } finally {
        setBusy(null);
      }
    },
    [client],
  );

  return (
    <div className="skills-view">
      <div className="tasks-head">
        <span className="pane-title">技能 / MCP</span>
        <div className="pane-head-btns">
          <button className="btn-mini" onClick={refresh} title="刷新">
            ⟳
          </button>
          {!embedded && (
          <button className="btn-mini" onClick={onBack} title="返回聊天">
            ← 返回
          </button>
          )}
        </div>
      </div>

      {error && <div className="banner banner-warn">加载失败：{error}</div>}
      {loading && <div className="pane-placeholder">加载中…</div>}

      {!loading && (
        <>
          {/* 技能区 */}
          <div className="sk-section">
            <div className="sk-section-title">技能 Skills</div>
            {skillDir && (
              <div className="sk-dir" title={skillDir}>
                目录：{skillDir}
              </div>
            )}
            {warnings.length > 0 && (
              <div className="banner banner-warn">
                {warnings.map((w, i) => (
                  <div key={i}>{w}</div>
                ))}
              </div>
            )}
            {skills.length === 0 ? (
              <div className="pane-placeholder">
                未发现技能。可在技能目录下放置 <code>名称/SKILL.md</code>。
              </div>
            ) : (
              skills.map((s) => (
                <div key={s.name} className="sk-card">
                  <div className="sk-card-main">
                    <div className="sk-name">{s.name}</div>
                    <div className="sk-desc">{s.description || "（无描述）"}</div>
                  </div>
                  <label className="switch">
                    <input
                      type="checkbox"
                      checked={s.enabled}
                      disabled={busy === s.name}
                      onChange={(e) => onToggle(s.name, e.target.checked)}
                    />
                    <span className="slider" />
                  </label>
                </div>
              ))
            )}
          </div>

          {/* MCP 区（只读运行时状态；编辑请进 MCP 设置） */}
          <div className="sk-section">
            <div className="sk-section-head">
              <div className="sk-section-title">MCP 服务器（运行时）</div>
              {onOpenMcpSettings && (
                <button type="button" className="btn btn-mini" onClick={onOpenMcpSettings}>
                  管理 MCP 配置
                </button>
              )}
            </div>
            {servers.length === 0 ? (
              <div className="pane-placeholder">未配置 MCP 服务器。</div>
            ) : (
              servers.map((m) => (
                <div key={m.name} className="sk-card">
                  <div className="sk-card-main">
                    <div className="sk-name">
                      {m.name}
                      {m.required && <span className="sk-tag">必需</span>}
                    </div>
                    <div className="sk-desc">
                      {m.command || m.url || "—"}
                      {" · "}
                      工具 {m.enabled_tools.length} 启用
                      {m.disabled_tools.length > 0 &&
                        ` / ${m.disabled_tools.length} 禁用`}
                    </div>
                  </div>
                  <div className="sk-status">
                    <span
                      className={m.connected ? "dot dot-on" : "dot dot-off"}
                      title={m.connected ? "已连接" : "未连接"}
                    />
                    <span className="sk-status-text">
                      {m.enabled ? (m.connected ? "已连接" : "未连接") : "已停用"}
                    </span>
                  </div>
                </div>
              ))
            )}
          </div>

          {/* MCP 工具列表 */}
          <div className="sk-section">
            <div className="sk-section-title">MCP 工具 ({mcpTools.length})</div>
            {mcpTools.length === 0 ? (
              <div className="pane-placeholder">暂无可用 MCP 工具（请确认服务器已连接）。</div>
            ) : (
              mcpTools.map((t) => (
                <div key={`${t.server}:${t.name}`} className="sk-card sk-tool">
                  <div className="sk-card-main">
                    <div className="sk-name">
                      <code>{t.server}/{t.name}</code>
                    </div>
                    <div className="sk-desc">{t.description || "（无描述）"}</div>
                  </div>
                </div>
              ))
            )}
          </div>
        </>
      )}
    </div>
  );
}
