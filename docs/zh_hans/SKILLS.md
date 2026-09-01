# Skills 管理器

> 本文翻译自英文版 [SKILLS.md](../SKILLS.md)，与英文修订 `fc23323c4`（2026-08-17）同步。

Skills 是可复用的 `SKILL.md` 指令包。Codewhale 从多个根目录发现它们，但**只有 CodeWhale 拥有的目录可写**。统一的 `/skills` 管理器是审计与变更的交互界面；斜杠别名共享同一条写入路径。

关于 Claude Code 插件边界，参见 [CLAUDE_PLUGIN_COMPAT.md](../CLAUDE_PLUGIN_COMPAT.md)。
关于 `skills_dir` 和 `[skills]` 配置键，参见 [CONFIGURATION.md](CONFIGURATION.md)。

## 架构（四层）

| 层 | 角色 |
| --- | --- |
| **Root catalog** | 优先级与所有权的单一来源（`SkillRootCatalog`）。 |
| **Audit** | 只读、未合并的磁盘清单（状态、摘要、动作）。 |
| **Mutation controller** | 安装 / 导入 / 更新 / 移除 / 信任的唯一写入者。 |
| **Skills manager view** | TUI：只发出事件；自身从不写文件。 |

运行时发现（`SkillRegistry`）仍会为模型合并 skills。审计刻意**不**合并——它展示磁盘上的每一份副本，让冲突和遮蔽保持可见。

## 所有权与根目录

**可写（CodeWhale 拥有）**

| 范围 | 路径 |
| --- | --- |
| 项目 | `<workspace>/.codewhale/skills/` |
| 全局 | `~/.codewhale/skills/` |

**只读兼容**（仅作为发现 / 导入来源——绝不就地变更）

例如：`<workspace>/.agents/skills`、`./skills`、`.claude/skills`、`.cursor/skills`、`.opencode/skills`、`~/.agents/skills`、`~/.claude/skills`，以及其他类似的 harness 布局。

**仅审计（不参与运行时）**

- `.codex/skills` 会出现在**兼容**审计扫描中，方便操作员查看。它**不会**加入运行时发现集合。

配置的 `skills_dir` 如果不是 CodeWhale 拥有的根目录之一，则保持只读。发现与管理器可以列出它；变更仍只作用于拥有的项目 / 全局根目录。

## 斜杠命令

| 命令 | 行为 |
| --- | --- |
| `/skills` | 打开 Skills 管理器（仅拥有方扫描，**无网络**）。 |
| `/skills <prefix>` | 按名称前缀过滤的文本列表。 |
| `/skills inspect` | 文本发现模式、搜索的目录与来源路径。 |
| `/skills --remote` | 显式 registry 列表（网络）。 |
| `/skills suggest <task>` | 为一个任务对最多三个远程 skills 排序，附带匹配证据和显式安装命令（网络；不安装）。 |
| `/skills sync` | 显式 registry → 本地缓存同步（网络）。 |
| `/skill <name>` | 为下一轮激活一个 skill。 |
| `/skill install [--project\|--global] <spec>` | 通过变更控制器安装。 |
| `/skill update [--project\|--global] <name>` | 从其 registry 来源更新一个受管理的 skill。 |
| `/skill uninstall [--project\|--global] <name>` | 移除一个受管理的 skill。 |
| `/skill trust [--project\|--global] <name>` | 写入绑定摘要的建议性信任。 |

说明：

- **没有** `/skills audit` 子命令。使用管理器（以及用 `c` 切换兼容根目录）或 `/skills inspect` 获取发现细节。
- 裸 `/skill install <spec>`（无范围标志）会安装到 CodeWhale **全局**拥有根目录。
- `/skills suggest` 只通过现有网络策略读取精选 registry。它从不下载、信任、启用或激活 skill；每个结果都会给出一个单独的 `/skill install <name>` 命令供用户选择。
- 如果同一名称同时存在于项目与全局拥有根目录，update / uninstall / trust 需要 `--project` 或 `--global`。
- 如果某名称只存在于兼容的外部根目录下，写入会被拒绝；请通过 `/skills` 导入它，而不是编辑 harness 目录。

## Skills 管理器（TUI）

默认打开路径：输入 `/skills` 并确认。该界面打开时零网络（仅拥有方审计）。

| 按键 | 动作 |
| --- | --- |
| `↑`/`↓` 或 `j`/`k` | 移动选择 |
| `Enter` | 主要可用动作 / 确认待处理的提示 |
| `i` | 导入（外部 → 拥有） |
| `u` | 更新（受管理 + registry 来源） |
| `r` | 移除（受管理；先确认） |
| `t` | 信任（受管理；绑定摘要） |
| `s` | 切换导入目标：项目 ↔ 全局 |
| `c` | 切换扫描：仅拥有方 ↔ 兼容（仍只在本地磁盘） |
| `Esc` | 取消确认，或关闭管理器 |

该视图从不调用安装助手，也不触碰文件系统。它发出一个变更请求；宿主运行控制器、显示回执并重建清单。

## 内置目录层级

Codewhale 以两个紧凑层级呈现其随附 skills，这样 agentic 工作流不会被文档与集成助手淹没：

- **核心 agentic**——规划、实现、调试、审查、验证、委派、Fleet、发布和 `best-of-n` 比较工作流。
- **格式与工具**——文档格式、数据可视化、前端与 Web 测试，以及 skill / plugin / MCP 编写助手。

工作区、用户和兼容 harness 的 skills 保持标记为 **custom**；Codewhale 不会从名称猜测其意图。随附包也不会宣传运行时缺失的能力。特别是，图像理解可用，但在真正图像生成工具出现之前，不会捆绑图像生成 skill。

仓库维护与发布操作助手（cgh-*c skills 和 [cskills/c](../skills/README.md) 下的 ccodew-release-qa-sweepc）**不**属于最终用户入门包，绝不会自动安装；一个 catalog-matrix 测试固定了这条边界。把它们作为可选包发布是插件交付工作，单独在 [#4836](https://github.com/Hmbown/CodeWhale/issues/4836) 中跟踪。

### 调用与别名元数据

内置和用户 skills 可以在 frontmatter 中声明两个运行时路由字段：

| 字段 | 含义 |
| --- | --- |
| `invocation: model+user` | 默认值；该 skill 出现在模型的紧凑目录中，模型或用户都可加载它。 |
| `invocation: explicit-only` | 该 skill 仍可按显式名称加载，但从模型目录中省略，这样 opt-in 指令不会变成环境上下文。 |
| `aliases-for: name, other-name` | 同一规范 skill 的附加查找名称。别名不是独立的目录条目，也不复制提示内容。 |

缺失或未知的 invocation 值保留历史 `model+user` 行为。发生冲突时规范名称优先于别名。加载 skill 会报告其规范调用与别名，让回执保持可检查。

### 入门包对等决策

[#4698](https://github.com/Hmbown/CodeWhale/issues/4698) 中的 v0.9.2 对等审计比较了五个 `xai-grok-memory` / `xai-grok-shell` 参考 skills 与实际的 Codewhale 包。这是一张决策矩阵，不是复制参考文本或宣传不受支持工具的请求：

| 参考 skill | Codewhale 决策 | 运行时依据 |
| --- | --- | --- |
| `check-work` | 规范别名 / 兼容映射到 `verify` | `verify` 是随附的证据收集工作流。 |
| `code-review` | 规范别名 / 兼容映射到 `review` | `review` 是随附的只读正确性工作流。 |
| `create-skill` | 规范别名 / 兼容映射到 `skill-creator` | `skill-creator` 是随附的编写工作流。 |
| `help` | 有界的 `invocation: explicit-only` 路由器，而非环境手册 | 路由到 `/help`、`/skills`、`/config`、`doctor` 和已安装的 `docs/` 树；不内嵌手册文本。 |
| `imagine` | 有意排除在范围之外 | Codewhale 没有图像生成 / 编辑工具，因此入门包不得宣传它。 |

关于两个非别名决策的说明：

- **`help`** 作为内置 skill 随附（第 7 代），但为 `explicit-only`，因此从不出现在模型目录中，环境提示预算为零。其主体是一张路由卡——哪个界面拥有哪个事实——并明确禁止把命令列表或设置表粘贴进上下文。一个受检查的不变式让它保持在 80 行以内，并要求它点名 `/help`、`/skills`、`/config` 和 `doctor` 界面。
- **`imagine`** 保持在外。随附运行时暴露的是图像*理解*，而非图像生成或编辑，因此任何内置 skill 都不得宣传它。目录矩阵断言 `imagine`、`image` 和 `image-gen` 不在包中，且解析为空。

此兼容切片不复制任何参考 skill 主体。显式别名与调用元数据是有界的路由事实；完整 skill 主体仍只通过 `load_skill` 进入上下文。

### 目录夹具矩阵（无 provider）

[`crates/tui/assets/skills-catalog-matrix.json`](../../crates/tui/assets/skills-catalog-matrix.json) 是一张**编写**的期望表，覆盖每个内置 skill：规范名称、层级、调用、别名、是否渲染为环境目录条目，以及它的哪些别名被另一个规范名称遮蔽。`crates/tui/src/skills/catalog_matrix.rs` 中的测试断言该夹具与 `BUNDLED_SKILLS` 之间存在一一对应，因此随附包不能在没有显式夹具更新的情况下改变。

这些测试断言什么、不断言什么：

- 它们验证**确定性 registry / catalog / resolver 行为**：安装、解析、资格、显式加载、不激活、别名解析、explicit-only 排除、冲突优先级和提示预算。
- 它们**不验证任何关于语义 LLM 路由的内容**。模型是否为一个 stack trace 选择 `debug` 是实时 provider 的问题；参见 [LIVE_SMOKE.md](../LIVE_SMOKE.md)。

当前断言的冲突与提示预算不变式：

| 不变式 | 含义 |
| --- | --- |
| 规范优先 | 规范内置名称总是胜过另一个 skill 的别名（`docx` → `docx`，绝不会是 `documents`）。 |
| 单一别名所有者 | 不得有两个内置 skills 声称同一别名。 |
| 无重复条目 | 每个规范名称最多渲染一行目录；别名渲染零行。 |
| 预算余量 | 随附包单独渲染时低于 `MAX_AVAILABLE_SKILLS_CHARS`（2 400 字符），且**没有**"additional skills omitted"行，因此用户 skills 永远不会被静默挤掉。 |
| 无上下文污染 | 描述保持单行，并在进入提示前被截断到 `MAX_SKILL_DESCRIPTION_CHARS`（280）。 |

### 区域感知的路由元数据

支持 `description_<tag>` frontmatter（先是精确 tag，然后是主子标签，最后是规范描述——繁体中文被排除在简体的 `zh` 回退之外）。**没有内置 skill 附带本地化路由描述**，也不会伪造一个。因此随附契约是一个显式的、经过测试的回退：

- 对于包中每个 skill × `Locale::shipped()` 中每个 locale——全部 15 个：`en`、`ja`、`zh-Hans`、`zh-Hant`、`pt-BR`、`es-419`、`vi`、`ko`、`ca`、`de`、`fr`、`id`、`hi`、`ru`、`uk`（`crates/tui/src/localization.rs:70-88`）——`description_for_locale` 返回规范英文描述。
- 渲染的目录块在所有随附 locale 中逐字节相同。
- 精确 tag 匹配、主子标签回退（`pt-BR` → `description_pt`）和英文回退都针对一个合成的编写夹具覆盖，因此即使包本身仅英文，解析路径也保持被测试。

如果内置 skill 以后附带本地化路由元数据，对等测试会失败，直到为它添加来源支撑的覆盖——回退契约不能静默吸收一个翻译。

## 审计状态

每个被审计的行携带优先级与关系标志：

| 状态 | 含义 |
| --- | --- |
| **Active** | 扫描中该规范名称优先级最高的副本。 |
| **Shadowed** | 同一名称存在于更高优先级的根目录。 |
| **Duplicate** | 与另一副本具有相同规范名称和相同包摘要。 |
| **Conflict** | 同一规范名称、不同包摘要。 |

没有拥有方对等项（且有有效摘要）的外部 skills 是**导入候选**。与拥有副本冲突或完全重复的外部项仍可提供导入——重复 → 已存在；冲突 → 在所选导入范围中确认替换。

## 来源与标记

受管理的安装会在 skill 目录下写入 schema **v2** 元数据：

**`.installed-from`（v2）**——成功安装 / 导入时最后写入：

```json
{
  "schema_version": 2,
  "spec": "github:owner/repo",
  "url": "https://…",
  "source_checksum": "…",
  "content_digest": "…",
  "installed_name": "my-skill",
  "registry_version": null
}
```

- `content_digest` 是一个有界的包树哈希（不只是 SKILL.md）。
- URL 的显示会剥离 userinfo、query 和 fragment。
- 导入使用本地 `import:…` 来源，**不能**从 registry 更新；请改为重新导入或移除它们。
- 旧版 v1 标记在刷新前以 `LegacyMetadataUnknown` 完整性被识别为受管理。

**`.trusted`（v2）**——建议性、绑定摘要：

```json
{
  "schema_version": 2,
  "content_digest": "…"
}
```

信任记录审查意图。它**不**沙箱该 skill，也**不**自动批准工具。内容更新会清除信任，这样过期的标记就不能比字节活得更久。

手动 skills（拥有根目录、无受管理标记）可见，但无法通过受管理动作进行 update / remove / trust。

## 包摘要与安全

审计与变更共享一个有界的包摘要：

- 仅常规文件；逃逸 skill 根目录或形成循环的符号链接 → 失败关闭。
- 对总大小、文件数和深度设上限。
- 变更在写入前重新检查期望摘要（TOCTOU）。
- 导入 / 替换保留一个 `.bak`，直到摘要 + 标记都成功完成；失败会恢复之前的拥有包。

## 就绪状态

审计模型有一个就绪状态字段和可选 provider 钩子，用于将来的就绪缓存（[#4407](https://github.com/Hmbown/CodeWhale/issues/4407)）。目前，当没有接入缓存时，就绪状态始终为 **`Unknown`**。管理器不运行就绪探针，也不会因就绪状态而阻止变更。

## 配置旋钮

```toml
# 发现偏好的可选覆盖项（除非是 CodeWhale 项目/全局拥有路径，否则不会自动成为写入目标）。
skills_dir = "/path/to/skills"

[skills]
# 为 true 时，运行时发现会跳过跨工具根目录（.claude、.agents 等）。
# 拥有方 CodeWhale 根目录与显式 skills_dir 覆盖项仍然生效。
scan_codewhale_only = false

# 由 --remote、sync 和 install 使用的可选 registry / 安装大小覆盖项。
# registry_url = "https://…"
# max_install_size_bytes = 5242880
```

完整配置面参见 [CONFIGURATION.md](CONFIGURATION.md)。

## 操作员清单

1. 日常管理优先使用 `/skills`；把 `--remote` / `sync` 保持显式。
2. 绝不手工编辑 `.claude` / `.agents` / `.cursor` 树来为 Codewhale "安装"——改为导入到 `.codewhale/skills`。
3. 把 `.trusted` 视为审查的建议性文档，而不是安全边界。
4. 在改变内容的 registry 更新之后，如果你仍想要建议性标记，请重新信任。
5. 同一名称的项目 + 全局双副本在 CLI 变更上需要显式范围标志。
