# 按键绑定

> 本文翻译自英文版 [KEYBINDINGS.md](../KEYBINDINGS.md)，与英文修订 `1a9600e7c`（2026-08-19）同步。

这里是 TUI 所识别的每一个键盘快捷键的权威目录。
快捷键绑定按**上下文**分组——即它们生效时所处的焦点或模态状态。列在“编辑器”下的快捷键仅在编辑器获得焦点时生效；“对话记录”下的快捷键仅在对话记录获得焦点时生效；依此类推。

全局快捷键目前尚不支持用户自定义——此功能计划在未来的版本中实现（#436, #437）。热键栏位操作可通过 `[[hotbar]]` 和 `/hotbar` 进行配置；热键栏激活快捷键仍为 `Alt-1` 至 `Alt-8`。
## 全局（任意上下文）

| 按键 | 操作 |
|------|------|
| `F1` 或 `Ctrl-/` | 切换帮助浮层 |
| `F2` | 切换键入式设置编辑器 |
| `Ctrl-K` | 打开命令面板（斜杠命令查找器） |
| `Ctrl-C` | 取消当前回合 / 关闭模态框 / 先武装再确认退出 |
| `Ctrl-B` | 将受支持的前台 shell 等待移入 `/jobs`，使对话回合得以继续；可用 `/jobs` 或 `action: "wait"` 的 `Bash` 来查看它 |
| `Ctrl-D` | 退出（仅当输入框为空时） |
| `Tab` | 当输入框为空时，循环切换 TUI 模式：Plan → Work → Operate → Plan |
| `Shift+Tab` | 循环切换权限姿态：Ask → Auto-Review → Full Access。无论输入框内容如何或回合是否在运行都即时生效（仅在打开 Config 以外的模态框时被抑制） |
| `Ctrl-T` | 循环切换当前模型的推理力度。走与 `/model` 和 `/effort` 相同的阶梯（catalog 或已文档化的路由方言）。始终思考的模型省略 `off`;Grok 4.6 包含 `xhigh`。 |
| `Ctrl-Shift-T` | 切换实时 transcript 浮层（粘性尾部自动滚动） |
| `Ctrl-R` | 打开恢复会话选择器 |
| `Ctrl-L` | 压缩对话上下文（状态行显示进度；压缩已在运行时为无操作） |
| `Ctrl-O` | 打开所选或当前回合的推理详情，与输入框内容无关 |
| `Ctrl-Alt-O` | 打开整回合的 Turn Inspector，与输入框内容无关 |
| `Alt-V` / `Option-V`（macOS） | 为所选、可见或最近的工具/子代理卡片打开详情分页器；发出传统 Option-V 字符的终端也会被处理 |
| `Ctrl-Shift-E` / `Cmd-Shift-E` | 切换文件树侧边栏 |
| `Alt-G` / `Alt-Shift-G` |输入框为空时将 transcript 滚动到顶部 / 底部 |
| `Alt-1`-`Alt-8` | 当没有模态框或内联选择器打开时分发 Hotbar 槽位 1-8 |
| `Alt-!` / `Alt-@` / `Alt-#` / `Alt-$` | 选择工作栏面板：Tasks / Agents / Context / Pinned |
| `Ctrl-Alt-0` | 关闭工作栏 / 恢复到顶部位置 |
| `Alt-L` | 为最后一条消息打开分页器（输入框为空） |
| `Alt-P` / `Alt-A` / `Alt-Y` | 跳到 Plan / Work，或请求 Full Access（`Alt-Y` 是旧的权限通道——Work + Full Access——不是独立模式；它遵循锁定的审批策略） |
| `Ctrl-X`（活动侧边栏） | 取消所有正在运行的后台 shell 任务 |
| `Esc` | 关闭最上层模态框 · 取消斜杠菜单 · 关闭 toast |

## Composer（消息输入区）

正在编辑你即将发送的消息。

| 按键 | 操作 |
|------|------|
| `Enter` | 空闲时发送；忙碌时排队;composer 为空时，立即发送下一条已排队的后续消息 |
| `Shift-Enter` / `Alt-Enter` / `Ctrl-J` | 插入换行而不发送（空闲或忙碌均可） |
| `Ctrl-Enter` / `Cmd-Enter` | 把内容发进当前回合；空闲时正常发送（终端能区分时） |
| `Ctrl-U` | 清空整个草稿（可恢复——参见 `Ctrl-Z`） |
| `Ctrl-Z` | 恢复已清空的草稿（仅当 composer 为空时） |
| `Ctrl-W` / `Ctrl-Backspace` / `Alt-Backspace` | 删除前一个单词 |
| `Ctrl-A` / `Home` | 移到输入开头 / 行首（readline 约定） |
| `Ctrl-E` / `End` | 移到输入结尾 / 行尾 |
| `Ctrl-←` / `Alt-←` | 向后移动一个单词 |
| `Ctrl-→` / `Alt-→` | 向前移动一个单词 |
| `Shift-←` / `Shift-→` | 每次扩展选区一个字素 |
| `Ctrl-Shift-←/→` / `Alt-Shift-←/→` | 每次扩展选区一个单词 |
| `Shift-Home` / `Shift-End` | 把选区扩展到行首 / 行尾 |
| `Ctrl-Shift-Home` / `Ctrl-Shift-End` | 把选区扩展到草稿开头 / 结尾 |
| `Ctrl-Shift-A` / `Cmd-A` | 选择整个草稿（参见下方说明） |
| `Ctrl-Shift-U` | 从键盘运行 `/update install`：无需离开 TUI 即可检查并安装最新的 Codewhale 版本。托管安装（Homebrew/npm/cargo）保留其包管理器门槛；已是最新版本时显示更新器的 "Already up to date." 结果，不做任何更改 |
| 鼠标拖动 | 选择 composer 文本；点击移动光标 |
| `Cmd-V` / `Ctrl-Shift-V` | 终端本地粘贴（在支持时以括号粘贴形式到达） |
| `Ctrl-V` | 在本地或转发的图形会话中直接粘贴剪贴板 |
| `Ctrl-Y` | 从 kill buffer 拉取（粘贴） |
| `↑` / `↓` | 循环 composer 历史（也用于选择弹窗/附件条目） |
| `Shift-↑` / `Shift-↓` | 浏览对话历史 |
| `Ctrl-P` / `Ctrl-N` | 在斜杠命令菜单条目间导航；菜单为空时 `Ctrl-P` 打开文件选择器 |
| `Ctrl-G` / `Ctrl-S` | 暂存当前草稿（`/stash pop` 恢复它）；从不发送或排队 |
| `Alt-R` | 搜索提示历史（Alt-R 退出） |
| `Tab` | 斜杠命令 / `@` 提及补全（感知弹窗） |
| `Ctrl-Shift-O` / `F4` | 在 `$VISUAL` / `$EDITOR` 中打开 composer 草稿；当终端无法区分 Ctrl-Shift-O 与 Ctrl-O 时，F4 可用 |
| `! command` | 通过常规的审批、sandbox 和输出界面运行 shell 命令 |

设置 `composer_multiline_mode = true` 即可交换可移植的 `Enter` 与 `Shift-Enter` 行为：`Enter` 插入换行，`Shift-Enter` 发送。`Alt-Enter`、`Ctrl-J` 以及受支持的 `Ctrl-Enter` / `Cmd-Enter` 行为保持不变。

### 选择语义

输入、粘贴、`Backspace` 或 `Delete` 在存在活动选区时会替换或删除所选文本，与任何 GUI 编辑器一致。纯移动键（方向键、`Home`/`End`、单词移动）会折叠选区。当选区覆盖整个草稿时，删除或在上面输入会像 `Ctrl-U` 一样暂存即将离开的文本，因此 `Ctrl-Z`（在空 composer 上）或 `Alt-R` 草稿恢复可以把它找回来。

光标移动和删除是字素感知的：一次 `←`/`→` 步进或一次 `Backspace` 覆盖完整的 emoji ZWJ 序列、旗帜对或组合标记簇——绝不会只删一半。CJK 文本按预期逐字符移动和删除。

**为什么全选不是 `Ctrl-A`：** composer 遵循 readline 约定，其中 `Ctrl-A` 跳到输入开头（与 `Ctrl-E` 配对）。全选在每个平台上都是 `Ctrl-Shift-A`（与 `Ctrl-Shift-O` / `Ctrl-Shift-E` 一样，需要支持增强键盘协议的终端）。在转发 Command 键的 macOS 终端上（kitty、WezTerm、带 Command 重映射的 iTerm2），原生的 `Cmd-A` 也会全选;`Cmd-Shift-A` 在 macOS 上随处可用，因为 Cmd 会归一化为 Ctrl。

### Hotbar

Hotbar 触发语义刻意只限 `Alt-1` 到 `Alt-8`。在 macOS 键盘上，这是 Option/Alt 键加数字行。裸 `1`-`8` 是 composer 中的正常文本输入，并仍归选择器、引导、审批提示和模态视图所有。

功能键和 `Cmd-1` 到 `Cmd-8` 不是 Hotbar 的主要组合。许多终端为标签页、窗口或操作系统快捷键保留了这些键，有些从不把它们转发给终端应用。如果终端配置为把 `Alt-1` 发送给某个自定义快捷键，Hotbar 也会收到同样可靠的组合。

自 #3807 起，缺少 `hotbar` 键会渲染**无栏**——全新配置在配置 `[[hotbar]]` 槽位之前不显示 Hotbar（显式的 `hotbar = []` 也会禁用它）。配置后，栏看起来像：

| 槽位 | 按键 | 默认操作 | 标签 |
|------|------|----------|------|
| 1 | `Alt-1` | `slash.workflow` | `wf` |
| 2 | `Alt-2` | `slash.goal` | `goal` |
| 3 | `Alt-3` | `slash.auto` | `auto` |
| 4 | `Alt-4` | `mode.plan` | `plan` |
| 5 | `Alt-5` | `mode.agent` | `agent` |
| 6 | `Alt-6` | `mode.operate` | `operate` |
| 7 | `Alt-7` | `palette.open` | `palette` |
| 8 | `Alt-8` | `sidebar.toggle` | `side` |

| 焦点状态 | Hotbar 行为 |
|----------|-------------|
| Composer 为空、有文本或空白 | `Alt-1`-`Alt-8` 分发配置的槽位 |
| 侧边栏聚焦、隐藏或自动 | `Alt-1`-`Alt-8` 仍然分发配置的槽位 |
| 斜杠菜单或历史搜索打开 | 被阻止；内联选择器拥有该按键事件 |
| 命令面板、帮助、审批、文件选择器、会话选择器、Fleet 设置或任何模态栈 | 被阻止；模态框拥有该按键事件 |
| Onboarding | 被阻止;Onboarding 拥有数字选择 |

### `@` 提及

输入 `@<partial>` 打开文件提及弹窗。`↑`/`↓` 循环条目，`Tab` 或 `Enter` 接受。`Esc` 隐藏弹窗。自 v0.8.10（#441）起，补全按提及频率重新排序——你经常且最近提到的文件会浮到顶部。

两种提及解析为精选的 git 上下文而不是路径（v0.9.2，#4067）：

| 提及 | 内联内容 | 字节预算 |
|------|----------|----------|
| `@git` | 工作区的 `git status --short --branch` | 8 KB |
| `@diff` | 工作树 diff，已暂存和未暂存（`git diff HEAD`） | 32 KB |

两者都出现在补全弹窗中路径的旁边，并且都显示在上下文检查器中，带有其解析后的大小；当 diff 超过其预算时，还会显示截断标记。当 git 缺失、工作区不是仓库或没有可显示的内容时，回合会携带显式的 `<git-unavailable>` 说明，而不是静默地什么都不贡献。仅以该 token 开头的路径（`@diff.txt`、`@git/config`）仍是文件提及。

### `#` 快速添加（记忆）

当 `[memory] enabled = true` 时，输入 `# foo` 并按 `Enter` 会把 `foo` 作为带时间戳的条目追加到你的记忆文件中，*而不会*发送回合。参见 `docs/MEMORY.md`。

## Transcript（transcript 获得焦点时）

| 按键 | 操作 |
|------|------|
| `↑` / `↓` / `j` / `k` | 滚动一行（v0.8.13+：composer 为空时裸方向键也可滚动） |
| `Alt-↑` / `Alt-↓` | 滚动 transcript（替代方式） |
| `PgUp` / `PgDn` | 滚动一页 |
| `Home` / `g` | 跳到顶部 |
| `End` / `G` | 跳到底部 |
| `Ctrl-Home` / `Ctrl-End` | 跳到顶部 / 底部（也可从 composer 中工作） |
| `Alt-[` / `Alt-]` | 在工具输出块之间跳转 |
| `Esc Esc` | 回溯到上一条用户消息（`←`/`→` 步进，`Enter` 回退） |
| `Esc` | 将焦点返回 composer |
| 鼠标拖动 | 在 Codewhale 中选择 transcript 文本 |
| `Ctrl-C` | 复制活动的 Codewhale 选区 |
| `Cmd-click`（macOS）/ `Ctrl-click`（Linux/Windows） | 在支持的终端中打开 OSC 8 链接（归终端处理） |

对于终端原生选择，按住 `Shift` 拖动（终端支持程度不一），然后使用终端自己的复制命令：通常是 macOS 上的 `Cmd-C` 或 Linux/Windows 上的 `Ctrl-Shift-C`。这些命令由本地终端处理，并刻意与 Codewhale 的 `Ctrl-C` 选择绑定分开。在 SSH 上，Codewhale 通过 OSC 52 发回复制请求，或在 tmux 内运行时通过 tmux 的 `load-buffer -w` 路径。

## Work bar（`Alt-W` 获得焦点后）

| 按键 | 操作 |
|------|------|
| `↑` / `↓` | 移动选择 |
| `Home` / `End` | 跳到第一行 / 最后一行 |
| `PageUp` / `PageDown` | 每次按视口移动选择 |
| `Enter` | 打开所选行的 world（work inspector / agent details）；在已打开的行上则关闭它 |
| `Esc` | 关闭已打开的详情，否则将焦点返回 composer |
| 任意可打印键 | 将焦点返回 composer（输入总是胜出） |

鼠标对等：点击任意工作栏行都执行 `Enter` 的操作，适用于每个面板和位置。`Alt-!`/`Alt-@`/`Alt-#`/`Alt-$` 切换面板。

## 斜杠命令面板（按 `Ctrl-K` 或输入 `/` 后）

| 按键 | 操作 |
|------|------|
| `↑` / `↓` / `Ctrl+P` / `Ctrl+N` | 移动选择 |
| `Enter` / `Tab` | 运行 / 补全高亮的命令 |
| `Esc` | 关闭面板 |

## Session Picker（`Ctrl-R` 或 `/sessions`）

| 按键 | 操作 |
|------|------|
| `↑` / `↓` / `j` / `k` | 在会话列表中移动选择 |
| `1`-`9` | 在该列表槽位打开可见的会话历史 |
| `PgUp` / `PgDn` | 翻历史面板的页 |
| `Enter` | 恢复所选会话 |
| `/` | 搜索会话 |
| `s` | 循环排序方式 |
| `a` | 切换当前工作区范围与所有工作区 |
| `e` | 归档 / 恢复所选会话 |
| `x` | 显示或隐藏已归档会话 |
| `d` | 确认后删除所选会话 |
| `Esc` / `q` | 关闭选择器 |

归档（`e`）非破坏性且无需确认：会话仍留在磁盘上且仍可加载，它只是离开默认列表并停止作为自动恢复候选。再按一次 `e` 把它带回来。删除（`d`）是破坏性的，并保留其确认。

## 审批模态框（当工具请求审批时）

| 按键 | 操作 |
|------|------|
| `y` / `Y` | 批准一次 |
| `a` / `A` | 全部批准（自动批准后续调用） |
| `n` / `N` / `Esc` | 拒绝 |
| `e` | 在运行前编辑已批准的输入 |

## Onboarding（首次运行流程）

| 按键 | 操作 |
|------|------|
| `Enter` | 前进到下一步（欢迎 → 语言 → API/信任门 → 设置检查点） |
| `Esc` | 后退一屏 |
| `1`–`9` | 选择语言（语言步骤） |
| `0`–`9` | 选择 provider（Provider 步骤;SGLang、vLLM 和 Ollama 默认无密钥） |
| `y` / `Y` | 信任工作区（信任步骤） |
| `n` / `N` | 跳过信任提示 |

## v0.8.29 审计说明

- **`Shift+Enter` / `Alt+Enter` 换行现在在 Windows 上的 VSCode 中可用（#1359）。** crossterm 的 `PushKeyboardEnhancementFlags` 命令在 Windows 上无条件返回 `Unsupported`（`is_ansi_code_supported() == false`），因此 Kitty 键盘协议转义从未写入终端。没有它，VSCode 的 xterm.js 停留在传统模式，其中 `Shift+Enter` 与普通 `Enter` 无法区分，导致 composer 发送消息而不是插入换行。修复方案直接在 Windows 上写入 push/pop 转义（`\x1b[>1u` / `\x1b[<1u`），绕过 crossterm 的能力门。VSCode 集成终端和 Windows Terminal ≥1.17 都遵循 Kitty 键盘协议；不理解这些序列的终端会静默丢弃它们。

## v0.8.13 审计说明

- **Ctrl-S 是暂存，不是历史搜索。** 在此修订中修复——`Alt-R` 才是历史搜索。
- **移除了幽灵 `Alt+Up`。** "Edit last queued message" 绑定曾列在 README 中，但从未存在于按键分发代码中。
- **composer 为空时裸 Up/Down 方向键滚动 transcript（v0.8.13）。** 以前 `should_scroll_with_arrows` 门被硬编码为 false，意味着即使 composer 为空，裸方向键也总是导航 composer 历史。虚拟终端（Ghostty、Codex、Kitty 协议）中的用户尤其受影响，因为他们无法使用 Cmd+Up / Alt+Up 快捷键。
- **可配置键位（#436）和 `tui.toml`（#437）仍然延期。** `TuiPrefs` 结构体和加载器存在于 `settings.rs` 中，但未在启动时接线。允许 `~/.codewhale/tui.toml` 覆盖单个条目的命名绑定注册表仍然待办。
- **未发现其他损坏的绑定。** 上面列出的每个其他组合都解析为 `crates/tui/src/tui/ui.rs`（按键事件分发）或 `crates/tui/src/tui/app.rs`（模式 + 状态转换）中的实时处理器。
