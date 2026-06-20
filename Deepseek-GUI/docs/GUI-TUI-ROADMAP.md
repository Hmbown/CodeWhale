# GUI 对齐 TUI 功能路线图

> 文档版本：2026-06-19  
> 适用范围：`Deepseek-GUI` 桌面客户端 vs `deepseek-tui` 终端客户端

## 背景

- **GUI** 通过 `deepseek serve --http` 复用 TUI 同一套 agent 内核。
- 聊天时 **模型侧工具**（shell、subagent、RLM、MCP 等）大多仍可用。
- 差距主要在 **用户操作面**：TUI 有 50+ 斜杠命令与完整管理 UI，GUI 目前约 12 条斜杠命令 + 设置页。

### HTTP API 现状

| 已有 `/v1/*` | 尚未暴露 HTTP |
|--------------|---------------|
| threads / turns / compact / fork / steer / interrupt | jobs（后台 Shell） |
| tasks / automations | subagent 状态 |
| skills（开关）/ MCP（只读） | RLM 会话 |
| sessions / usage / approvals | hooks / network / memory |
| workspace/status | queue / stash / undo |

---

## 阶段划分

```
阶段一（快赢）     → 斜杠命令、Composer、设置扩展
阶段二（配置面板） → MCP / Hooks / Network / Memory
阶段三（新 API）   → Jobs / 终端 / Subagent / RLM
阶段四（打磨）     → 主题、CLI 桥接、高级调试
```

---

## 阶段一：快赢（约 1–2 周）

目标：常用操作不必回 TUI；不改后端或改动极小。

| ID | 功能 | TUI 对照 | 实现方式 | 模块 | 工作量 | 优先级 |
|----|------|----------|----------|------|--------|--------|
| 1.1 | 斜杠命令补齐（第一批） | `/mode` `/trust` `/retry` `/provider` | 扩展 `runSlashCommand` + 命令面板 | `App.tsx` | S | P0 |
| 1.2 | 推理强度快捷切换 | Shift+Tab | Composer 底栏 effort pill | `Composer.tsx` | S | P0 |
| 1.3 | 会话导出 | `/export` | Tauri 聚合 thread JSON 另存为 | `SessionsView` | S | P1 |
| 1.4 | 工作区切换命令 | `/workspace` | `/workspace` + `set_workspace` | `App.tsx` | S | P1 |
| 1.5 | 斜杠自动补全 | slash_menu | Composer 输入 `/` 弹出候选 | 新 `slashCommands.ts` | M | P1 |
| 1.6 | 用量/上下文面板 | `/tokens` `/cost` `/context` | 设置页 Usage 卡片 | 新 `UsagePanel.tsx` | M | P1 |
| 1.7 | Profile 快捷切换 | `/profile` | Composer/顶栏调 `activateProfile` | `ConfigView.tsx` | S | P1 |
| 1.8 | 附件命令 | `/attach` | 文件选择 → `save_attachment` | `Composer.tsx` | S | P2 |

**1.1 首批斜杠命令行为**

| 命令 | GUI 行为 |
|------|----------|
| `/mode [plan\|agent\|yolo]` | PATCH thread |
| `/trust [on\|off]` | PATCH `trust_mode` |
| `/retry` | 重发上一条 user 消息 |
| `/provider` | 打开设置 → 模型 |
| `/task` | 打开设置 → 任务 |

---

## 阶段二：配置与管理面板（约 2–3 周）

目标：TUI 的 `/mcp` `/hooks` `/network` `/memory` 有 GUI 等价物；主要靠 Tauri 读写配置。

| ID | 功能 | TUI 对照 | 实现方式 | 模块 | 工作量 | 优先级 |
|----|------|----------|----------|------|--------|--------|
| 2.1 | MCP 完整管理 | `/mcp add/enable/remove/reload` | Tauri 读写 MCP 配置 + 重启 backend | `McpSettingsPanel.tsx` | L | P0 |
| 2.2 | Hooks 管理 | `/hooks` | Tauri 读写 hooks 配置 | `HooksView.tsx` | M | P1 |
| 2.3 | Network 策略 | `/network` | Tauri 读写 network 配置 | `NetworkPanel.tsx` | M | P1 |
| 2.4 | Memory / Note / Anchor | `/memory` `/note` `/anchor` | Tauri 读写本地 memory/notes | `MemoryPanel.tsx` | M | P2 |
| 2.5 | Skills 安装/卸载 | `/skill install/uninstall` | 文件系统 + `/v1/skills` | `SkillsView.tsx` | M | P2 |
| 2.6 | 运行时 LSP 开关 | `/lsp on/off` | config.toml + 编辑器 LSP 联动 | `SettingsView` | S | P2 |
| 2.7 | 信任目录列表 | `/trust list/add/remove` | Tauri 读写 trusted paths | 设置新 Tab | M | P2 |
| 2.8 | Onboarding 向导 | TUI onboarding | 首次启动 Stepper | `Onboarding.tsx` | M | P2 |

---

## 阶段三：需扩展 HTTP API（约 3–4 周）

目标：补齐 TUI 进程内能力；需改 `crates/tui/src/runtime_api.rs`。

| ID | 功能 | TUI 对照 | 建议新路由 | 前端 | 工作量 | 优先级 |
|----|------|----------|------------|------|--------|--------|
| 3.1 | 后台 Shell Jobs | `/jobs` | `GET/POST /v1/jobs`, poll/cancel/stdin | `JobsPanel.tsx` | XL | P0 |
| 3.2 | 集成终端 | 内嵌 shell | xterm.js + Tauri PTY | `TerminalPanel.tsx` | XL | P0 |
| 3.3 | Subagent 面板 | `/subagents` `/agent` | `GET /v1/subagents` | `SubagentsPanel.tsx` | L | P0 |
| 3.4 | RLM 会话面板 | `/rlm` | `GET /v1/rlm/sessions` | `RlmPanel.tsx` | L | P1 |
| 3.5 | Queue / Stash | `/queue` `/stash` | `PATCH /v1/threads/{id}/composer` | Composer 排队条 | L | P1 |
| 3.6 | Patch Undo | `/undo` `/restore` | `POST .../undo`, `GET .../snapshots` | DiffModal 还原 | L | P1 |
| 3.7 | Context 调试 | `/context` `/cycles` | `GET /v1/threads/{id}/context` | 调试抽屉 | M | P2 |
| 3.8 | 会话 Save/Load | `/save` `/load` | 复用 `/v1/sessions` + 导出 | `SessionsView` | M | P2 |

**推荐顺序**：Jobs 只读面板 → Subagent 面板 → xterm 终端集成。

---

## 阶段四：体验增强（约 2–3 周，可选）

| ID | 功能 | TUI 对照 | 说明 | 工作量 | 优先级 |
|----|------|----------|------|--------|--------|
| 4.1 | 主题切换 | `/theme` | 2–3 套 CSS 变量主题 | M | P3 |
| 4.2 | Vim 输入模式 | composer vim | IDE 场景优先级低 | L | P4 |
| 4.3 | 外部编辑器 | `/edit` | 「在 VS Code 打开」 | S | P2 |
| 4.4 | Doctor 诊断 | `deepseek doctor` | 设置页 spawn CLI | M | P2 |
| 4.5 | 非交互 Exec | `deepseek exec` | 任务页脚本任务 | M | P3 |
| 4.6 | PR 预填 | `deepseek pr` | 命令面板从 PR 导入 | M | P3 |
| 4.7 | Tab 补全增强 | — | auto-import、FIM `/beta` | M | P2 |
| 4.8 | 引擎 LSP 钩子 | post-tool lint | 保存时 LSP 诊断 toast | M | P2 |

---

## 里程碑

| 里程碑 | 周期 | 交付 | 对齐估算 |
|--------|------|------|----------|
| **M1 命令基线** | 第 1–2 周 | 1.1–1.7 + slash 补全 | ~45% 常用命令 |
| **M2 配置中心** | 第 3–5 周 | 2.1–2.3 MCP/Hooks/Network | ~60% 管理面 |
| **M3 运行时面板** | 第 6–9 周 | 3.1–3.4 Jobs/终端/Subagent/RLM | ~80% 核心能力 |
| **M4 打磨** | 第 10–12 周 | 3.5–3.8 + 4.x | ~90% |

---

## 工作量图例

| 代号 | 含义 | 人天（约） |
|------|------|-----------|
| S | 小改 | 1–2 |
| M | 新组件 | 3–5 |
| L | 新面板 + API | 1–2 周 |
| XL | 终端/PTY 等 | 2–3 周 |

---

## 建议优先开工 Top 5

1. **1.1** 斜杠命令第一批（`/mode` `/trust` `/retry`）
2. **1.5** Composer 斜杠自动补全
3. **2.1** MCP 完整管理
4. **3.3** Subagent 面板（需 API）
5. **3.1** Jobs 只读面板

---

## GUI 已有、无需重复

- 三栏 IDE、多标签编辑器、语法高亮
- 编辑器 LSP IntelliSense
- Cursor Tab AI 内联补全
- Rules 编辑器 + Rule Compliance Banner
- 任务/自动化、技能开关、历史会话
- Plan/Agent/YOLO、审批、steer/interrupt
- @ 文件引用、图片粘贴、Git diff

---

## 风险

| 风险 | 缓解 |
|------|------|
| Jobs/Subagent 无 HTTP API | 优先在 `runtime_api.rs` 专开 sprint |
| MCP 改配置需重启 backend | 保存后自动 `restart_backend` |
| Windows PTY 兼容 | 先做 Jobs 输出 WebView，PTY 二期 |

---

## 变更记录

| 日期 | 说明 |
|------|------|
| 2026-06-19 | 初版：基于 GUI vs TUI 功能差距分析 |
| 2026-06-20 | **M1 完成**：斜杠命令扩展、自动补全、推理强度、用量弹窗、export/attach/profile |
| 2026-06-20 | **M2 完成**：MCP/Hooks/Network 配置面板 + Tauri config_bridge |
| 2026-06-20 | **M3 部分**：Jobs 占位、Subagents 本地只读 |
| 2026-06-20 | **M3 续**：`/v1/jobs` + `/v1/subagents` HTTP API 与 GUI 可操作面板 |
| 2026-06-20 | **M3 完成**：RLM API、集成终端（PTY+xterm）、/rlm /terminal 斜杠命令 |

### M1 已完成项

| ID | 状态 |
|----|------|
| 1.1 | ✅ 斜杠命令补齐 |
| 1.2 | ✅ 推理强度 Shift+Tab + 底栏 pill |
| 1.3 | ✅ /export 会话 JSON |
| 1.4 | ✅ /workspace |
| 1.5 | ✅ 斜杠自动补全 |
| 1.6 | ✅ UsageModal（/cost / /tokens） |
| 1.7 | ✅ /profile |
| 1.8 | ✅ /attach + pick_file |

**下一步：M2** — MCP 完整管理（2.1）、Hooks（2.2）、Network（2.3）

### M2 已完成项

| ID | 状态 |
|----|------|
| 2.1 | ✅ MCP 完整管理（McpSettingsPanel + /mcp） |
| 2.2 | ✅ Hooks 管理（HooksPanel + /hooks） |
| 2.3 | ✅ Network 策略（NetworkPanel + /network） |

### M3 已完成（核心）

| ID | 状态 |
|----|------|
| 3.1 | ✅ Jobs API + GUI |
| 3.2 | ✅ 集成终端（Tauri PTY + xterm，设置页 Terminal Tab） |
| 3.3 | ✅ Subagents API + GUI |
| 3.4 | ✅ RLM 面板（GET /v1/rlm/sessions + RlmPanel） |

**下一步：M4** — Queue/Stash、Patch Undo、Context 调试、体验增强
