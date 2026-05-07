# 超越 Claude Code 的编程体验方案

> 基于 DeepSeek TUI v0.8.15 代码库的分析与实施路线图
> 制定日期: 2026-05-07

---

## 一、现状诊断

### Claude Code 的核心优势（我们落后的地方）

| 维度 | Claude Code | DeepSeek TUI 现状 | 差距 |
|------|-------------|-------------------|------|
| **代码理解** | LSP 全集成（跳转定义、引用、hover、符号搜索） | 仅 post-edit diagnostics | 大 |
| **权限控制** | 工具 × 路径 allow/deny/ask 规则，级联审批 | 全有或全无（Plan/Agent/YOLO） | 大 |
| **持久记忆** | 跨会话学习用户偏好、项目约定 | 仅 memory.md 手动写入 | 大 |
| **技能生态** | 多路径自动发现 + URL 技能源 | 基础目录扫描，社区安装 | 中 |
| **Agent 画像** | 命名 Agent 类型，模型/权限继承 | Plan/Agent/YOLO 三模式 | 中 |
| **生命周期钩子** | 6 个事件类型的 shell/plugin 钩子 | crates/hooks 基础框架存在 | 中 |
| **视觉能力** | 截图分析、浏览器交互 | web_run（headless） | 中 |
| **插件系统** | JS/TS 热加载插件（工具、模型、中间件） | 无 | 大 |
| **编辑器集成** | VS Code / JetBrains 原生扩展 | ACP stdio 适配器（基础） | 中 |

### DeepSeek TUI 的独特优势（我们领先的地方）

| 能力 | 说明 | 竞品状态 |
|------|------|----------|
| **RLM** | Python 沙箱内的批量 LLM 处理（chunk/batch/recurse） | 无竞品具备 |
| **自动化** | cron-style RRULE 定时任务调度 | 无竞品具备 |
| **持久化任务** | 可重启的任务对象，证据追踪、门禁验证 | 无竞品具备 |
| **Turn 回滚** | side-git 快照，单 turn 级 workspace 恢复 | 无竞品具备 |
| **1M 上下文窗口** | DeepSeek V4 原生，成本极低 | 竞品受限于各自模型 |
| **极低成本** | Flash $0.14/M input，Pro $0.435/M（含 75% 折扣） | 优势显著 |
| **子代理并行** | agent_spawn + RLM llm_query_batched 双层并行 | 仅 OpenCode 有近似 |
| **数据验证** | JSON/TOML 验证工具 | 无竞品具备 |

---

## 二、核心策略：不对称竞争

**不追平，要超越。** Claude Code 在 Anthropic 模型上跑，我们在 DeepSeek V4 上跑。我们的策略是：

1. **Prompt 即产品** — 行为质量（规划能力、并行本能、验证习惯）是编程体验的第一要素
2. **V4 原生能力武器化** — 1M 上下文 + RLM 批量处理 + 子代理并行 = 竞品无法复制的体验
3. **功能对标但不盲目** — LSP、权限、记忆等基础能力对标，但用 V4 成本优势做出更好的实现
4. **编辑器深度集成** — 不只做 TUI，要做到 VS Code / JetBrains 内的一等公民

---

## 三、分阶段实施计划

### Phase 1: Prompt & 行为大修（最高杠杆，预计 2 天）

**目标**: 让模型从"谨慎但能力未充分发挥"变为"策略性、并行、自验证"的编程助手。

Prompt 是编程体验的核心——它决定模型如何规划、如何并行、如何验证。当前 prompt（PROMPT_ANALYSIS.md 已诊断）的主要问题：

- RLM 被框架为"最后手段"而非"战略工具"
- 子代理被视为"实现步骤"而非"探索工具"  
- 缺少"先批处理再执行"的本能
- 思考预算对 V4 太保守
- 缺少"验证后声明"的行为模式

**具体变更**:

1. **RLM 重新定位** — 从"仅用于超大输入"改为三种模式（CHUNK/BATCH/RECURSE），每种都有明确的使用场景和非使用场景
2. **子代理策略正面化** — 从"何时不要用"移到正面策略指导，鼓励并行探索
3. **并行优先启发式** — 在任何工具调用前，扫描 checklist 是否有可并行的操作
4. **思考预算上调** — 代码生成从 Light 提到 Medium，多文件重构从 Light 提到 Medium
5. **验证原则** — 每次工具调用后验证结果再行动，不信任记忆
6. **组合模式** — update_plan + checklist_write + 每阶段重新评估的完整工作流

**Prompt 文件**:
- `crates/tui/src/prompts/base.md`
- `crates/tui/src/prompts/modes/agent.md`
- `crates/tui/src/prompts/modes/plan.md`

**验收标准**: 相同任务（如"修复一个跨 3 个文件的 bug"）的 turn 数减少 30-50%，并行工具调用增加 2-3x。

---

### Phase 2: LSP 工具（预计 5 天）

**目标**: 让模型拥有代码智能——跳转定义、查找引用、类型信息——不再靠 grep + 逐文件阅读。

**当前基础**: 
- `crates/tui/src/lsp/` 已有 LspManager、StdioLspTransport、diagnostics 注入
- post-edit diagnostics 已通过 engine/lsp_hooks.rs 接入
- registry.rs 已有语言检测和默认 server 映射

**需要新增的模型可见工具**:

```rust
// crates/tui/src/tools/lsp.rs
tool lsp_goto_definition(file, line, col) -> Location[]
tool lsp_find_references(file, line, col) -> Location[]  
tool lsp_hover(file, line, col) -> String (type info, docs)
tool lsp_document_symbols(file) -> Symbol[]
tool lsp_workspace_symbols(query) -> Symbol[]
```

**设计要点**:
- 复用现有 LspManager 的 transport 池，不重连
- 如果 LSP server 未启动，自动懒启动
- 结果缓存（同一位置短时间内不重复查询）
- 语言回退：rust-analyzer → pyright → typescript-language-server → gopls → clangd

**验收标准**: 代码库探索 turn 数减少 30-50%（与竞品分析预测一致）。具体：查找函数定义从"grep + read_file + 人工看"变为单次 LSP 调用。

---

### Phase 3: 细粒度权限系统（预计 4 天）

**目标**: 从"全有或全无"到"工具 × 路径 × 操作"的精确控制，消除审批疲劳。

**当前基础**: `crates/execpolicy/` 已有 ExecPolicyEngine，支持 layered rulesets（builtin/agent/user-priority）。

**需要新增**:

```
权限规则引擎:
  Rule { tool: &str, path_pattern: &str, action: Allow | Deny | Ask }
  支持通配符: "edit_file" × "*.env" → Ask, "read_file" × "src/**" → Allow
  支持 ~ 和 $HOME 展开
  级联: "always allow" 自动解决同一 session 内的 pending 请求
```

**TUI 交互**:
- 审批弹窗增加 "Always allow this pattern" / "Deny this pattern" 选项
- `/permissions` 命令查看/编辑当前规则
- 项目级 `.deepseek/permissions.toml` 覆盖用户全局规则

**存储**: 规则持久化到 `~/.deepseek/permissions.toml`，项目覆盖到 `<workspace>/.deepseek/permissions.toml`

**验收标准**: 长 session（>20 turns）的审批弹窗减少 60-80%。用户可以表达"src/ 下永远允许读，.env 永远询问"。

---

### Phase 4: 持久记忆系统（预计 4 天）

**目标**: 跨会话积累知识——用户偏好、项目约定、过去的决策——每次新会话都更聪明。

**当前基础**: `memory.md` 手动写入 + `remember` 工具。

**需要新增**:

1. **自动记忆提取** — 会话结束时（或每 N turns），模型自动提取关键事实：
   - 用户偏好："prefers 4-space indentation"
   - 项目约定："tests run with `cargo test --workspace --all-features`"
   - 架构决策："moved from actix-web to axum in v0.8"
   - 已知问题："the Windows build needs MSVC, not MinGW"

2. **记忆检索** — 新会话开始时，注入相关记忆：
   - 关键词匹配（项目名、文件名、技术栈）
   - 简单的语义相关性（未来可升级到 embedding）

3. **存储格式**:
```toml
# ~/.deepseek/memories/project/<hash>.toml
[[memories]]
content = "This project uses Rust edition 2024"
source = "session_abc123"
created = 2026-05-07T10:30:00Z
confidence = "high"
tags = ["rust", "project-config"]
```

4. **模型可见工具**:
   - `memory_search(query)` — 模型可主动检索记忆
   - 自动注入：会话开始时注入与当前 workspace 最相关的 5-10 条记忆

**验收标准**: 第 3 次在同一项目工作时，模型不再需要重新发现基础事实。Turn 数减少 15-25%。

---

### Phase 5: 生命周期钩子（预计 3 天）

**目标**: 用户可定义的 shell 命令在关键事件触发——不污染 system prompt 的安全网。

**当前基础**: `crates/hooks/` 已有基础框架。

**需要新增的钩子事件**:

| 事件 | 触发时机 | Payload | 用途示例 |
|------|----------|---------|----------|
| `ToolCallBefore` | 工具执行前 | tool_name, params | "编辑 .rs 前先 fmt" |
| `ToolCallAfter` | 工具执行后 | tool_name, result, duration | "写文件后自动 git add" |
| `SessionStart` | 新会话开始 | session_id, cwd | "激活虚拟环境" |
| `MessageSubmit` | 用户发送消息 | prompt_text | "日志记录用户输入" |
| `SessionEnd` | 会话结束 | reason | "清理临时文件" |
| `PermissionRequest`（规划中） | 权限请求时 | tool, justification | "记录所有审批日志" |
| `PreCompaction`（规划中） | compaction 前 | token_count | "备份当前会话" |

**配置格式**:
```toml
# ~/.deepseek/hooks.toml
[[hooks]]
event = "ToolCallAfter"
matcher = { tool = "edit_file", path_pattern = "*.rs" }
command = "cargo fmt"
timeout_sec = 10

[[hooks]]
event = "ToolCallBefore"
matcher = { tool = "exec_shell", command_pattern = "rm *" }
command = "echo 'WARNING: destructive command' >&2"
action = "warn"  # 不阻止，只警告
```

**验收标准**: 用户可表达 "每次编辑 .rs 文件后自动 cargo fmt"，无需在 system prompt 中说明。

---

### Phase 6: Agent 画像系统（预计 3 天）

**目标**: 不同任务类型使用不同的 Agent 配置——代码审查用只读 Agent，构建用全权限 Agent。

**设计**:

```
Agent 类型:
  - general: 默认，全工具，中等权限
  - explore: 只读，grep/read/lsp/web_search
  - plan: 只读 + plan 工具，不可编辑
  - implementer: 全工具，自动审批（YOLO-lite）
  - reviewer: 只读 + review + github
  - builder: shell + file，用于编译/测试
  - custom: 用户自定义
```

**每个 Agent 画像包含**:
```toml
[agents.reviewer]
model = "deepseek-v4-pro"
reasoning_effort = "high"
tools = ["read_file", "grep_files", "lsp_*", "review", "github_*"]
permissions = { "*" = "deny", "read_file" = "allow", "grep_files" = "allow" }
system_prompt_extension = "You are a code reviewer..."
```

**模型可见工具**: `agent_spawn` 接受 `type = "reviewer"` 参数，自动继承画像配置。

**验收标准**: 用户可以用 `/agent reviewer` 切换 Agent，或 `agent_spawn(type="reviewer", objective="review PR #123")`。

---

### Phase 7: 视觉与多媒体（预计 5 天）

**目标**: 支持截图分析、PDF 视觉审查、图片生成，覆盖更多编程场景。

**DeepSeek V4 视觉能力**: DeepSeek V4 支持多模态输入（image_url content blocks），但当前 TUI 未暴露。

**需要新增**:

1. **截图分析工具**:
```rust
tool screenshot_analyze(path: &str, question: &str) -> String
// 读取本地截图文件，发送给 V4 多模态分析
```

2. **PDF 视觉审查**（提升现有 read_file PDF 支持）:
```rust
// read_file 已支持 pdftotext 文本提取
// 新增: pages="1-3" + visual=true 时发送 PDF 页面截图给 V4
```

3. **TUI 内图片粘贴** — 从剪贴板粘贴图片到 composer，自动保存为临时文件并发送

4. **图片生成**（可选，依赖外部 API）:
```rust
tool image_generate(prompt: &str) -> ImageResult
// 通过 DALL-E / Stable Diffusion API
```

**验收标准**: 用户可以截图一个 UI bug，粘贴到 TUI，模型分析截图并给出修复建议。

---

### Phase 8: V4 原生优势武器化（预计 5 天）

**目标**: 把 DeepSeek V4 独有能力变成竞品无法复制的体验。

#### 8.1 RLM 2.0 — 交互式 REPL

当前 RLM 是 one-shot batch——子 LLM 写 Python，运行一次返回结果。升级为交互式：

```
RLM 2.0:
  - 子 LLM 可以分多步执行 Python
  - 每步的 stdout/stderr 返回给子 LLM
  - 支持条件逻辑："如果分类结果置信度 < 0.8，再调用一次 llm_query"
  - max_steps = 10 防止无限循环
```

#### 8.2 推测执行

当模型生成 tool_calls 时，如果多个 tool_call 相互独立：

```
当前: 模型生成 3 个 read_file → 逐个执行 → 逐个返回
推测执行: 模型生成 3 个 read_file → 全部并行执行 → 一次性返回 → 模型基于全部结果继续推理
```

这已经是当前 dispatcher 的行为（parallel tool calls），但可以进一步：
- 预测性执行：模型还在生成 tool_call 2 时，tool_call 1 已开始执行
- chunked streaming: 每完成一个 tool_call 就流式返回，不等待全部完成

#### 8.3 前缀缓存感知编排

DeepSeek V4 的前缀缓存是 128-token 粒度，缓存命中率决定成本和速度：

```
当前: 靠 prompt 指导模型保持前缀稳定（AGENTS.md 中有提示）
升级: 引擎层自动优化
  - 检测即将发送的请求与前一个请求的共同前缀长度
  - 如果共同前缀 > 90%，优先组装而非重建
  - /compact 时自动计算缓存友好的截断点
  - 在 /cost 中显示 cache hit rate 历史曲线
```

#### 8.4 成本感知调度

```
当前: auto mode 根据任务类型选模型/thinking
升级: 成本预算调度器
  - 用户设 daily/weekly 成本预算
  - 引擎自动在 Flash / Pro 之间切换以保持在预算内
  - 关键任务（安全审查、发布）自动升级到 Pro
  - 简单任务（读文件、grep）强制 Flash
```

**验收标准**: RLM 2.0 可处理需要多轮分析的任务。推测执行使多文件操作 turn 数减少。成本预算下，日均花费可控制。

---

### Phase 9: IDE 级体验（预计 4 天）

**目标**: TUI 内的编程体验接近 IDE——内联 diff、流式补丁进度、诊断叠层。

#### 9.1 内联 Diff 预览

当前 `edit_file` / `apply_patch` 结果中已包含 unified diff（v0.8.8 新增），但仅在结果区域以只读方式渲染。升级：

```
内联 Diff:
  - 在编辑操作前，模型可生成 preview diff
  - 用户在 TUI 中看到带颜色的 +/- 行
  - 按 y 批准，n 拒绝，e 编辑
  - 批准的 diff 直接应用
```

#### 9.2 流式补丁进度

当模型通过 `apply_patch` 生成大型补丁时：

```
流式进度:
  - 实时显示 "正在生成 patch: models.rs (3 hunks)..."
  - 每个 hunk 应用后立即显示进度
  - 失败的 hunk 高亮标注
```

#### 9.3 诊断叠层

当前 LSP 诊断已注入到下一轮 API 请求前（v0.8.6）。提升到实时叠层：

```
诊断叠层:
  - 文件编辑后，编辑器侧边栏显示诊断图标（🔴 error / 🟡 warning）
  - 按 F2 跳转到下一个诊断
  - 模型可看到诊断上下文而不消耗额外 turn
```

#### 9.4 文件树与符号导航

```
侧边栏:
  - /files 命令打开文件树（类似 IDE 的 project explorer）
  - /symbols 命令打开当前文件符号列表
  - 方向键导航，Enter 展开/跳转
```

**验收标准**: 编辑操作有内联 diff 预览，诊断实时可见，文件树可导航。

---

### Phase 10: 生态与分发（预计 5 天）

**目标**: 不只做 TUI——做平台。

#### 10.1 插件系统

```
插件架构:
  - 类似 OpenCode 的 JS/TS 热加载插件
  - 插件可注册: tools, models, auth providers, hooks, chat middleware
  - 插件沙箱: 默认只读文件系统，申请权限
  - 插件发现: ~/.deepseek/plugins/ + npm registry

插件 manifest:
```json
{
  "name": "deepseek-docker",
  "version": "1.0.0",
  "tools": ["docker_build", "docker_run"],
  "hooks": ["PostToolUse", "SessionStart", "PreCompaction"],
  "permissions": ["exec_shell"]
}
```

#### 10.2 VS Code / JetBrains 扩展

```
VS Code 扩展:
  - 复用现有 ACP stdio 适配器
  - 侧边栏面板显示对话
  - 内联建议（类似 Copilot）
  - 右键菜单 "DeepSeek: Explain this" / "DeepSeek: Fix this"

JetBrains 插件:
  - 基于 ACP 协议
  - 工具窗口集成
  - 编辑器上下文感知
```

#### 10.3 社区技能市场

```
技能发现:
  - github.com/Hmbown/deepseek-skills 作为官方 registry
  - /skill search "docker" 搜索社区技能
  - /skill publish 发布自己的技能
  - 评分/下载量/更新时间
```

**验收标准**: 第三方开发者可发布插件和技能，VS Code 扩展可用。

---

## 四、优先级矩阵

按 **影响力 × 实现成本** 排序：

```
影响力 ↑
    │  Phase 1 ★★★★★     Phase 8 ★★★★
    │  (Prompt 大修)      (V4 武器化)
    │  2天, 极高ROI       5天, 竞品无法复制
    │
    │  Phase 2 ★★★★★     Phase 3 ★★★★
    │  (LSP 工具)         (权限系统)
    │  5天, 最大能力差    4天, 体验质变
    │
    │  Phase 4 ★★★★      Phase 7 ★★★
    │  (持久记忆)         (视觉能力)
    │  4天, 复利效应      5天, 新场景
    │
    │  Phase 5 ★★★       Phase 9 ★★★
    │  (钩子系统)         (IDE 体验)
    │  3天, 安全网        4天, 精致度
    │
    │  Phase 6 ★★★       Phase 10 ★★
    │  (Agent 画像)       (生态)
    │  3天, 灵活性        5天, 长期价值
    └──────────────────────────────→ 实现成本
```

**推荐攻击顺序**: 1 → 2 → 3 → 4 → 8 → 5 → 6 → 7 → 9 → 10

Phase 1 和 Phase 2 是**立刻见效**的——Prompt 大修让现有功能发挥更好，LSP 填补最大能力空白。两者加起来预计 7 天，即可在核心编程体验上接近 Claude Code。

Phase 8（V4 武器化）是**不对称优势**——竞品无法复制 RLM 2.0、推测执行、成本感知调度。应在基础能力对标完成后立即投入。

---

## 五、关键指标

| 指标 | 当前（估计） | Phase 1 后 | Phase 1-3 后 | Phase 1-8 后 |
|------|-------------|-----------|-------------|-------------|
| 平均 turn 数/任务 | 8-12 | 5-8 | 4-6 | 3-5 |
| 并行工具调用率 | ~15% | ~35% | ~40% | ~50% |
| 用户审批次数/task | 3-5 | 3-5 | 1-2 | 1-2 |
| 代码探索准确率 | ~70% | ~80% | ~90% | ~95% |
| 会话间知识保留 | ~10% | ~10% | ~30% | ~70% |
| 日均 API 成本(Flash) | $0.50 | $0.35 | $0.30 | $0.25 |

---

## 六、风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| Prompt 修改导致模型行为退化 | 中 | 高 | A/B 测试，先用 Flash 低成本验证 |
| LSP server 稳定性问题 | 中 | 中 | 优雅降级到 grep 回退，不阻断主流程 |
| 权限规则过于复杂，用户不敢用 | 低 | 低 | 好的默认值 + 渐进式暴露 |
| V4 API 变更导致武器化功能失效 | 低 | 中 | 功能检测 + 优雅降级 |
| 插件系统安全风险 | 中 | 高 | 沙箱执行，权限最小化 |

---

## 七、差异化总结

超越 Claude Code 不在于逐项追平功能，而在于：

1. **Prompt 质量** — 更会规划、更会并行、更会验证的 Agent
2. **V4 原生优势** — 1M 上下文 + 极低成本 + 前缀缓存 + RLM
3. **成本优势** — 让用户用得起 Pro mode，"奢侈"地使用高思考预算
4. **独特的自动化能力** — 定时任务、持久化任务、turn 回滚
5. **编辑器和终端双栖** — 不只做 TUI，做所有开发界面的 Agent 后端

**一句话**: DeepSeek TUI 不应该成为"更便宜的 Claude Code 替代品"——它应该成为"因为有 V4 所以能做 Claude Code 做不了的事"的编程助手。
