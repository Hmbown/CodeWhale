# DeepSeek GUI

DeepSeek CLI/TUI 的桌面图形版本，基于 **Tauri + React + TypeScript** 构建。
通过本地 HTTP/SSE 运行时（`deepseek serve --http`）与 DeepSeek agent 内核通信，
提供三栏 IDE 式界面：左侧文件树、中间代码查看、右侧多会话聊天。

## 功能概览

- **多会话管理**：水平标签栏，新建 / 切换 / 重命名（双击）/ 关闭归档 / 恢复。
- **流式对话**：实时显示助手回复、推理（思考）块、工具调用 / 命令执行 / 文件变更卡片。
- **思考块可视化**：流式实时计时 + 动画，完成后折叠显示「已思考 X.Xs」。
- **审批交互**：工具/命令需确认时弹出审批对话框，支持允许/拒绝/记住选择。
- **三栏 IDE**：文件树（含删除）、代码查看（多标签、可关闭）、聊天面板实时联动。
- **@ 文件引用**：输入 `@` 触发工作区文件补全，键盘导航选择并插入相对路径。
- **图片粘贴**：粘贴图片自动存入 `.deepseek/attachments/` 并插入 `@路径` 供 agent 分析。
- **斜杠命令**：`/compact`、`/fork`、`/review`、`/diff`、`/sessions`、`/help`。
- **任务 / 自动化**：创建并管理后台自主任务，查看状态、耗时、计数。
- **技能 / MCP**：技能启用/禁用开关；只读展示 MCP 服务器连接状态。
- **历史会话浏览器**：搜索后端持久化历史会话，恢复为新线程或删除。
- **命令面板**：`Ctrl/Cmd+K` 模糊检索并执行全部命令与视图切换。
- **配置中心**：管理 DeepSeek API Key、Base URL，连接测试，多套配置档。

## 目录结构

```
Deepseek-GUI/
├─ src/                 前端（React + TS）
│  ├─ api/              运行时 API 客户端、Tauri 桥接、类型定义
│  ├─ components/       各 UI 组件（聊天、文件树、任务、技能、会话、命令面板等）
│  ├─ state/            会话状态 Hook（SSE 订阅与聚合）
│  └─ styles.css        全局样式
├─ src-tauri/           Tauri 壳（Rust）：进程托管、文件系统命令、配置读写
└─ dist/               前端构建产物
```

## 构建与运行

> **路径说明**：仓库目录已统一为无空格路径 `E:\Coding\DeekSeel-TUI-GUI`，可直接在本目录构建，无需 `C:\dsgui` 目录联结。
> Windows 构建需将 `D:\Config\mingw64\bin` 与 Rust GNU 工具链加入 `PATH`（本机 Rust 安装在 `D:\Config\rust`）。

### 一键打包（推荐）

```powershell
cd E:\Coding\DeekSeel-TUI-GUI\Deepseek-GUI
npm install   # 首次或依赖变更时
.\scripts\build-release.ps1
```

脚本依次：`cargo build --release -p deepseek-tui` → `npm run build` → 复制 `deepseek-tui.exe` 到 `src-tauri/bin/` → `npm run tauri:build`（安装包内会带上 sidecar）。

### 手动分步

```powershell
# 1. 安装依赖
cd E:\Coding\DeekSeel-TUI-GUI\Deepseek-GUI
npm install

# 2. 构建后端 sidecar（仓库根目录）
cd E:\Coding\DeekSeel-TUI-GUI
cargo build --release -p deepseek-tui

# 3. 构建前端
cd Deepseek-GUI
npm run build

# 4. 复制 sidecar 并打包
Copy-Item ..\target\release\deepseek-tui.exe src-tauri\bin\deepseek-tui.exe -Force
npm run tauri:build              # 生成 .msi 与 .exe 安装器
npm run tauri:build -- --no-bundle   # 仅生成可执行 deepseek-gui.exe
```

产物位置：

- 可执行：`src-tauri/target/release/deepseek-gui.exe`
- 安装器：`src-tauri/target/release/bundle/{msi,nsis}/`

开发模式：`npm run tauri:dev`。

## 注释 / 编码注意

- 本 README 必须以 **UTF-8** 保存；编辑时请使用整文件写入，避免按片段替换导致中文损坏。

## 更新记录

### 001
- 更新时间：2026-06-19 11:00:00
- 涉及模块：项目脚手架
- 功能变更：初始化 Tauri + React + TypeScript + Vite 工程，搭建桌面壳与前端骨架。
- 实现说明：配置 tauri.conf.json、Vite、TS；建立 src/ 与 src-tauri/ 基本结构。

### 002
- 更新时间：2026-06-19 11:10:00
- 涉及模块：运行时 API 客户端
- 功能变更：实现与 `deepseek serve --http` 通信的客户端，封装线程、回合、用量等接口。
- 实现说明：新增 api/client.ts、api/types.ts，统一请求与错误处理、Bearer 鉴权。

### 003
- 更新时间：2026-06-19 11:20:00
- 涉及模块：SSE 事件流
- 功能变更：订阅线程事件流，按 item 聚合增量并落定，支持断线续传。
- 实现说明：新增 api/events.ts 与 state/useConversation.ts，按 seq 单调去重。

### 004
- 更新时间：2026-06-19 11:30:00
- 涉及模块：聊天界面
- 功能变更：消息列表渲染（用户/助手/推理/工具等），输入区发送/转向/打断。
- 实现说明：新增 MessageItem.tsx、Composer.tsx，按 TurnItemKind 区分样式。

### 005
- 更新时间：2026-06-19 11:40:00
- 涉及模块：审批交互
- 功能变更：工具/命令需确认时弹出审批对话框，支持允许/拒绝/记住选择。
- 实现说明：新增 ApprovalDialog.tsx；useConversation 维护审批队列。

### 006
- 更新时间：2026-06-19 11:50:00
- 涉及模块：会话管理
- 功能变更：会话列表的新建、切换、删除（归档）。
- 实现说明：client 增加线程增删改查；App 维护 threads 与 activeId。

### 007
- 更新时间：2026-06-19 12:00:00
- 涉及模块：用量统计
- 功能变更：底部展示输入/输出/缓存 token、回合数与估算成本。
- 实现说明：回合完成后拉取 /v1/usage 聚合并展示。

### 008
- 更新时间：2026-06-19 12:10:00
- 涉及模块：配置中心
- 功能变更：新增设置界面，管理 DeepSeek API Key 与 Base URL。
- 实现说明：新增 ConfigView.tsx；Tauri 壳读写 ~/.deepseek/config.toml。

### 009
- 更新时间：2026-06-19 12:20:00
- 涉及模块：连接测试
- 功能变更：设置中可测试 API Key / Base URL 连通性。
- 实现说明：Tauri 壳新增 test_connection 命令。

### 010
- 更新时间：2026-06-19 12:25:00
- 涉及模块：多配置档
- 功能变更：支持保存多套 API 配置档并切换。
- 实现说明：壳层读写 ~/.deepseek/gui_profiles.json。

### 011
- 更新时间：2026-06-19 12:30:00
- 涉及模块：后端进程托管
- 功能变更：壳启动时自动拉起后端 sidecar，崩溃重启，退出清理。
- 实现说明：src-tauri 管理子进程与运行时 token、CORS。

### 012
- 更新时间：2026-06-19 12:35:00
- 涉及模块：三栏 IDE 布局
- 功能变更：左侧文件树、中间代码查看、右侧聊天的三栏布局。
- 实现说明：新增 FileTree.tsx、CodeView.tsx；壳新增 pick_folder/list_dir/read_file/set_workspace。

### 013
- 更新时间：2026-06-19 12:40:00
- 涉及模块：文件树实时更新
- 功能变更：回合完成后自动刷新文件树，反映 agent 的文件改动。
- 实现说明：treeTick 自增触发重读盘。

### 014
- 更新时间：2026-06-19 12:45:00
- 涉及模块：代码查看标签
- 功能变更：代码查看支持文件标签，可关闭。
- 实现说明：CodeView 管理打开文件与关闭按钮。

### 015
- 更新时间：2026-06-19 12:50:00
- 涉及模块：输入交互
- 功能变更：Enter 发送、Shift+Enter 换行（兼容输入法组合）。
- 实现说明：Composer onKeyDown 处理 isComposing。

### 016
- 更新时间：2026-06-19 12:55:00
- 涉及模块：多会话标签栏
- 功能变更：水平标签栏管理多会话，新建按钮与在线状态指示。
- 实现说明：chat-tabs 横向滚动布局。

### 017
- 更新时间：2026-06-19 13:00:00
- 涉及模块：工具卡片
- 功能变更：工具调用/命令执行/文件变更改为带状态徽标的折叠卡片，文件变更按行着色 diff。
- 实现说明：UiItem 新增 title；抽出 ToolCard 与 DiffBody，新增 .tool-card* 样式。

### 018
- 更新时间：2026-06-19 13:05:00
- 涉及模块：会话管理（路线图 P1-3）
- 功能变更：会话支持重命名（双击标签）与查看已归档（归档置灰、关闭按钮变恢复）。
- 实现说明：App 新增 showArchived；新增 onRenameThread、onRestoreThread。至此 P1 阶段完成。

### 019
- 更新时间：2026-06-19 13:10:00
- 涉及模块：斜杠命令（路线图 P2-1）
- 功能变更：支持 /compact、/fork、/help；标签栏命令菜单一键执行。
- 实现说明：client 新增 compactThread、forkThread；App 新增 runSlashCommand，onSend 拦截 / 开头输入。

### 020
- 更新时间：2026-06-19 13:25:00
- 涉及模块：任务/自动化界面（路线图 P2-2）
- 功能变更：新增「任务/自动化」界面（📋）：创建后台任务、卡片展示状态/模型/模式/耗时/错误、可取消、每 3 秒轮询并显示计数。
- 实现说明：types 新增 Task* 类型；client 新增 listTasks/createTask/cancelTask；新增 TasksView.tsx。

### 021
- 更新时间：2026-06-19 13:40:00
- 涉及模块：技能/MCP（路线图 P2-3）
- 功能变更：新增「技能/MCP」界面（🧩）：技能启用/禁用开关；MCP 只读展示连接状态、命令/URL、工具数。至此 P2 阶段完成。
- 实现说明：types 新增 Skill*/Mcp* 类型；client 新增 listSkills/setSkillEnabled/listMcpServers；新增 SkillsView.tsx（乐观更新+回滚）。

### 022
- 更新时间：2026-06-19 13:50:00
- 涉及模块：目录结构 / 文档
- 功能变更：GUI 目录由 gui/ 重命名为 Deepseek-GUI/，同步更新文档路径引用。
- 实现说明：同步更新 docs/GUI_DEVELOPMENT_PLAN.md。

### 023
- 更新时间：2026-06-19 13:55:00
- 涉及模块：文件树 / Tauri 壳
- 功能变更：文件树新增删除功能（悬浮垃圾桶按钮，确认后删除，目录递归）；删除后刷新，若删的是当前文件则关闭代码视图。
- 实现说明：壳新增 delete_path（拒绝删盘根）；tauri.ts 新增 deletePath；FileTree 透传 onChanged/onDeleted。

### 024
- 更新时间：2026-06-19 13:58:00
- 涉及模块：输入区 Composer（路线图 P3-1 @文件引用）
- 功能变更：输入框支持 @ 文件引用——键入 @ 触发工作区文件下拉，可过滤、键盘导航、Enter/Tab 选中、Esc 关闭，插入 @相对路径。
- 实现说明：Composer 新增 rootPath；首次触发惰性广度优先遍历（复用 list_dir，上限 2000、深度 6）；新增 .mention-pop 样式。

### 025
- 更新时间：2026-06-19 14:05:00
- 涉及模块：消息渲染（路线图 P3-2 思考块可视化）
- 功能变更：推理块改为「思考卡片」：流式实时计时+动画，完成后折叠显示「已思考 X.Xs」，可展开查看推理文本。
- 实现说明：UiItem 新增 startedAt/durationMs；useConversation 记录起止时间；MessageItem 抽出 ThinkingCard，新增 .think-card* 样式。

### 026
- 更新时间：2026-06-19 14:15:00
- 涉及模块：输入区 Composer / Tauri 壳（路线图 P3-3 图片粘贴）
- 功能变更：粘贴图片自动保存到 .deepseek/attachments/ 并插入 @相对路径（运行时回合仅支持文本，故用落盘+引用方案）。
- 实现说明：壳新增 save_attachment(dir,name,bytes)；tauri.ts 新增 saveAttachment；Composer 新增 onPaste。

### 027
- 更新时间：2026-06-19 14:25:00
- 涉及模块：历史会话（路线图 P3-4 会话历史浏览器）
- 功能变更：新增「历史会话」界面（🕘）：搜索历史会话、恢复为新线程、删除。
- 实现说明：types 新增 Session* 类型；client 新增 listSessions/resumeSession/deleteSession；新增 SessionsView.tsx。

### 028
- 更新时间：2026-06-19 14:35:00
- 涉及模块：斜杠命令 / Tauri 壳（路线图 P3-5、P3-6、P3-7）
- 功能变更：新增 /diff（模态框展示工作区 git diff，按行着色）与 /review（构造审查提示词发起回合，可带 @文件）；P3-5（/undo /restore）因运行时无对应接口，沿用 /fork 作等价替代并在 /help 说明。
- 实现说明：壳新增 git_diff(dir)；tauri.ts 新增 gitDiff；新增 DiffModal.tsx；runSlashCommand 扩展 diff/review/sessions。

### 029
- 更新时间：2026-06-19 14:45:00
- 涉及模块：命令面板（路线图 P3-8）
- 功能变更：新增命令面板——Ctrl/Cmd+K 唤出，模糊检索全部命令与视图切换，键盘导航。至此 P3 阶段全部完成。
- 实现说明：新增 CommandPalette.tsx 与 .palette*/.modal-overlay 样式；App 新增 showPalette、全局 keydown 监听与 paletteCommands。

### 030
- 更新时间：2026-06-19 14:55:00
- 涉及模块：右侧面板视图切换（缺陷修复）
- 功能变更：修复「点开第一个面板再点第二个仍显示第一个」的问题——任务/技能/历史/设置改为互斥显示，同一时刻仅一个，再次点击当前面板收起回到聊天。
- 实现说明：App 新增 toggleView 互斥切换函数，工具栏按钮与命令面板视图入口统一改走该逻辑。已重建前端并重新打包安装器。

### 031
- 更新时间：2026-06-19 15:05:00
- 涉及模块：文档 / 编码修复
- 功能变更：修复本 README 因按片段替换导致全文中文损坏的问题，整文件以分块写入+二进制拼接方式重建为正确 UTF-8。
- 实现说明：经测试整文件写入对大体积中文会损坏，改用约 1.8KB 分块写入并用 Python 二进制拼接；后续维护本文件请沿用分块或小步写入避免编码损坏。

### 032
- 更新时间：2026-06-19 15:20:00
- 涉及模块：代码查看 CodeView / Tauri 壳
- 功能变更：中间栏代码区改为可编辑——文本文件可直接修改；标签栏新增「保存」按钮与 Ctrl+S 快捷键；未保存时显示 ● 标记，关闭前确认；二进制/超大截断文件仍为只读。
- 实现说明：Tauri 壳新增 write_file(path, content)；tauri.ts 新增 writeFile；CodeView 由只读高亮改为 textarea 编辑器，Tab 插入 4 空格；新增 .code-editor/.code-dirty 样式。已 npm run build 验证。

### 033
- 更新时间：2026-06-19 15:35:00
- 涉及模块：代码编辑 CodeView
- 功能变更：修复可编辑模式下代码全白无高亮的问题——改用 CodeMirror（VS Code 暗色主题），编辑时按文件类型着色（HTML/CSS/JS/TS/JSON/MD/PY/RS 等），保留行号、折叠、Ctrl+S 保存。
- 实现说明：引入 @uiw/react-codemirror 与对应语言包；CodeView 用 ResizeObserver 自适应中间栏高度。已 npm run build 验证。

### 034
- 更新时间：2026-06-19 15:50:00
- 涉及模块：App / SnakeGame
- 功能变更：移除开发期彩蛋「贪吃蛇」——删除右上角 🐍 按钮、SnakeGame 组件及相关样式；中间栏仅保留代码编辑。
- 实现说明：删除 src/components/SnakeGame.tsx；App.tsx 去掉 showGame 状态与切换逻辑。已 npm run build 验证。

### 035
- 更新时间：2026-06-19 16:05:00
- 涉及模块：代码编辑 CodeView
- 功能变更：新增代码字号缩放——标签栏提供 − / + 按钮与当前字号显示（点击字号重置为 13px）；支持 Ctrl+滚轮缩放；字号范围 10–24px 并持久化到 localStorage。
- 实现说明：CodeMirror 通过 EditorView.theme 动态设置 fontSize；新增 .code-zoom* 样式。已 npm run build 验证。

### 036
- 更新时间：2026-06-19 16:25:00
- 涉及模块：全局 UI / App 布局 / styles.css
- 功能变更：仿 Cursor 优化界面——新增左侧活动栏（资源管理器/历史/设置）、VS Code 暗色配色、Chat 面板标题栏与 ghost 图标按钮、底部状态栏（工作区/文件/Token/后端状态）、会话标签下划线风格、消息与输入区视觉 refinement；侧边栏可折叠。
- 实现说明：grid 布局改为 活动栏+侧栏+编辑区+Chat；新增 .activity-bar/.icon-btn/.status-bar/.app-shell 等样式；Token 用量移至状态栏。已 npm run build 验证。

### 037
- 更新时间：2026-06-19 16:40:00
- 涉及模块：App 会话标签 / 右侧面板
- 功能变更：修复在「任务/技能/设置」等子页面时点击会话标签无反应的问题——点击标签现在会自动切回聊天视图并切换会话；新建会话同样回到聊天；标签对比度提高便于识别当前会话。
- 实现说明：新增 showChatView/selectThread；标签 onClick 改走 selectThread。已 npm run build 验证。

### 038
* 更新时间：2026-06-19 14:33:59
* 涉及模块：编辑器 / useConversation / App（路线图 P4-1）
* 功能变更：中间栏支持多文件标签；Agent 回合结束后自动打开本回合修改的文件；状态栏显示已打开标签数量。
* 实现说明：新增 EditorPanel、useEditorTabs、workspacePaths 路径解析；useConversation 在 turn.completed 收集 file_change 路径；CodeView 支持 embedded 多实例挂载保留未保存状态。

### 039
* 更新时间：2026-06-19 14:33:59
* 涉及模块：FileTree / Tauri 壳（路线图 P4-2）
* 功能变更：文件树支持新建文件/文件夹、重命名；删除后同步关闭编辑器标签，重命名同步更新标签路径。
* 实现说明：壳新增 create_file/create_dir/rename_path；FileTree 工具栏与行内重命名按钮；useEditorTabs.renameFile。

### 040
* 更新时间：2026-06-19 14:33:59
* 涉及模块：App 会话设置（路线图 P4-3）
* 功能变更：Chat 面板新增 per-thread 开关：Shell、信任模式、自动批准（allow_shell/trust_mode/auto_approve）。
* 实现说明：ThreadRecord 扩展字段；patchThread 本地合并；chat-modelbar 下方 thread-settings 复选框。

### 041
* 更新时间：2026-06-19 14:33:59
* 涉及模块：布局 / QuickOpen / 状态栏（路线图 P4-4）
* 功能变更：左栏与 Chat 分栏可拖拽调整宽度（持久化）；Ctrl+P 快速打开文件；状态栏展示 Git 分支与变更计数。
* 实现说明：useResizablePanels 写 CSS 变量；QuickOpen 递归索引工作区；client.getWorkspaceStatus。

### 042
* 更新时间：2026-06-19 14:33:59
* 涉及模块：TasksView / AutomationsView / SkillsView / client（路线图 P4-5）
* 功能变更：任务页新增「定时自动化」子页签（CRUD、运行/暂停/恢复）；技能页展示 MCP 工具列表。
* 实现说明：AutomationsView + /v1/automations API 封装；SkillsView 调用 listMcpTools。

### 043
* 更新时间：2026-06-19 14:33:59
* 涉及模块：斜杠命令 / 命令面板（路线图 P4-6）
* 功能变更：扩展 /clear /model /models /rename /cost；/help 更新；命令面板新增「快速打开文件」。
* 实现说明：/clear 以 fork 等价替代无 HTTP 的清空接口；/cost 调用 getUsage 弹窗展示。已 npm run build 验证。

### 044
* 更新时间：2026-06-19 14:37:07
* 涉及模块：useConversation / threadConvCache（路线图 P5-1）
* 功能变更：多会话状态缓存——切换标签时保留消息列表，SSE 从 latest_seq 续传，避免每次从 0 全量回放。
* 实现说明：新增 threadConvCache.ts；useConversation 切换线程时读写快照；SSE 订阅起始 seq 取自缓存。

### 045
* 更新时间：2026-06-19 14:37:07
* 涉及模块：ThreadSearch / client（路线图 P5-2）
* 功能变更：新增会话搜索——Ctrl+Shift+P 或 Chat 栏 ⌕ 按钮，调用 /v1/threads/summary?search= 模糊检索并切换。
* 实现说明：新增 ThreadSearch.tsx；client.searchThreads；命令面板增加入口。

### 046
* 更新时间：2026-06-19 14:37:07
* 涉及模块：NoticeList / MessageItem（路线图 P5-3）
* 功能变更：展示 sandbox.denied 与 coherence.state 系统通知；文件变更卡片点击可在编辑器打开对应文件。
* 实现说明：useConversation 解析两类事件为 notices；NoticeList 组件；ToolCard 支持 openChangedFile。

### 047
* 更新时间：2026-06-19 14:37:07
* 涉及模块：i18n / App 状态栏（路线图 P5-4）
* 功能变更：新增中英文界面切换（状态栏 中文/EN 按钮，localStorage 持久化）；核心 UI 文案 i18n 化。
* 实现说明：src/i18n/index.ts 轻量 t() 函数；App 主要标签与占位符接入 locale。

### 048
* 更新时间：2026-06-19 14:37:07
* 涉及模块：会话 system_prompt / 文档（路线图 P5-5）
* 功能变更：Chat 面板可编辑并保存 per-thread system_prompt；docs/GUI_DEVELOPMENT_PLAN.md 路线图同步为 P0–P5 已完成。
* 实现说明：thread-prompt 折叠编辑区 + patchThread；已 npm run build 验证。

### 049
* 更新时间：2026-06-19 14:41:28
* 涉及模块：分栏布局 / useResizablePanels / styles.css
* 功能变更：修复 Chat 面板过宽或窗口较窄时中间编辑器与文件树被挤没、界面看似「只剩 Chat」的问题；状态栏新增 ⊞ 重置布局按钮。
* 实现说明：grid 使用 minmax 保证中间栏至少 200px；启动与 resize 时 clamp sidebar/chat 宽度；ide 填满 ide-resize-host。

### 050
* 更新时间：2026-06-19 14:43:09
* 涉及模块：发布打包（Tauri bundle）
* 功能变更：重新构建前端 dist 并打包桌面安装器，生成最新 MSI 与 NSIS setup 安装包。
* 实现说明：经由 C:\dsgui junction（规避路径空格）执行 npm run tauri:build；产物位于 src-tauri/target/release/bundle/{msi,nsis}/，版本 0.1.0_x64（MSI 7.77MB、NSIS setup 5.24MB、deepseek-gui.exe 22.12MB）。

### 051
* 更新时间：2026-06-19 14:46:00
* 涉及模块：App.tsx（命令面板）/ 代码审查
* 功能变更：修复 paletteCommands 的 useMemo 依赖数组缺失 rootPath，导致「快速打开文件」命令的 disabled 状态在打开文件夹后不及时更新的问题。
* 实现说明：审查中核对 SSE sandbox.denied/coherence.state 字段与后端一致、tsc --noEmit 通过；其余为不影响功能的性能/缓存建议（见会话审查结论）。

### 052
* 更新时间：2026-06-19 15:05:00
* 涉及模块：App.tsx / api/client.ts / workspacePaths.ts / i18n / styles.css
* 功能变更：修复「换文件夹后 Agent 仍在旧目录创建文件」：打开文件夹时等待后端重启并自动新建绑定该目录的会话；新建会话始终携带 workspace；会话与资源管理器目录不一致时显示警告条。
* 实现说明：根因是每个 thread 创建时 workspace 写死且 PATCH 不可改；chooseFolder 调用 waitForBackend + createThread(dir)；文件路径解析改用 activeThread.workspace。

### 053
* 更新时间：2026-06-19 15:14:13
* 涉及模块：发布打包（Tauri bundle）
* 功能变更：重新构建含 workspace 同步修复（#052）的桌面安装包。
* 实现说明：npm run build + C:\dsgui junction 下 npm run tauri:build；产物 0.1.0_x64（MSI 7.77MB、NSIS 5.24MB、exe 22.12MB）。

### 054
* 更新时间：2026-06-19 15:25:00
* 涉及模块：App.tsx / ApprovalDialog / runtime_threads.rs
* 功能变更：修复「已开自动批准/信任仍反复弹审批」：GUI 在 auto_approve/trust_mode 下静默代批且不弹窗；remember 与 PATCH 同步 active_turn；记住此选择默认勾选。
* 实现说明：根因是 approval.required 始终 SSE 推送且 remember 未更新进行中的回合 flags；后端 sync_active_turn_approval_flags + 前端 showApprovalDialog 条件与 auto-approve effect。

### 055
* 更新时间：2026-06-19 15:32:00
* 涉及模块：FileTree / styles.css
* 功能变更：移除资源管理器顶部「＋文件 / ＋文件夹」按钮；改为在文件夹、空白区域或文件上右键弹出菜单新建（新建文件/新建文件夹），条目上另含重命名与删除。
* 实现说明：TreeContextMenu 固定定位；右键目标目录为所点文件夹或其父目录；空目录「（空）」行亦可右键新建。

### 056
* 更新时间：2026-06-19 15:40:00
* 涉及模块：useConversation / App.tsx / workspacePaths.ts
* 功能变更：Agent 创建项目时资源管理器实时更新：每个 write_file 等工具完成即刷新文件树并打开新文件，不再等整轮结束；运行中每 2s 轻量轮询兜底。
* 实现说明：新增 fileChangeTick/lastFileChangePaths；item.completed 聚合路径后立即 notify；extractPaths 兼容 file_path 字段。

### 057
* 更新时间：2026-06-19 16:05:00
* 涉及模块：RulesView / cursorRules.ts / project_context.rs / App.tsx / styles.css
* 功能变更：新增 Cursor 风格「项目规则」：在 `.cursor/rules/*.mdc` 管理规则（description、globs、alwaysApply、正文），活动栏与 Chat 工具栏入口；后端自动合并规则到 Agent 上下文。
* 实现说明：格式与 Cursor Rules 兼容；保存后下一条消息生效（prompts 每轮重载 project context）；支持 `.cursorrules`  legacy 文件。

### 058
* 更新时间：2026-06-19 16:35:00
* 涉及模块：App.tsx / workspaceSessions.ts
* 功能变更：打开项目时类似 Cursor：自动恢复该项目上次 Agent 会话，无历史则新建；标签栏仅显示当前项目会话；跨项目搜索会话时同步切换资源管理器目录。
* 实现说明：`ds_workspace_threads` 持久化项目→会话映射；`openWorkspace` 替代每次打开文件夹都新建；启动时按 `ds_root` 恢复；`switchToThread` 支持 syncWorkspace。

### 059
* 更新时间：2026-06-19 15:50:00
* 涉及模块：发布打包（Tauri bundle / deepseek-tui）
* 功能变更：重新构建前端 dist、deepseek-tui release 与桌面安装器（含项目规则 #057、按项目恢复会话 #058 等近期改动）。
* 实现说明：`cargo build --release -p deepseek-tui` + 真实路径 `npm run build` + `C:\dsgui` junction 下 `npm run tauri:build`；`deepseek-tui.exe` 已复制至 release 目录与 GUI 同目录运行；产物 0.1.0_x64（MSI 7.78MB、NSIS 5.25MB、gui 22.13MB、tui 61.88MB）。

### 060
* 更新时间：2026-06-19 16:10:00
* 涉及模块：App.tsx / styles.css
* 功能变更：模型/模式选择与安全开关（Shell、信任、自动批准、系统提示词）从 Chat 顶栏移至底部输入区下方，布局对齐 Cursor Agent 面板。
* 实现说明：新增 `chat-main`/`chat-footer`/`chat-bottom-bar`；顶栏仅保留标题与会话标签；系统提示词展开区在输入框上方。

### 061
* 更新时间：2026-06-19 15:55:00
* 涉及模块：发布打包（Tauri bundle）
* 功能变更：重新构建前端与桌面安装器（含 Chat 底部设置栏 #060 等近期 GUI 改动）。
* 实现说明：真实路径 `npm run build` + `C:\dsgui` 下 `npm run tauri:build`；产物 0.1.0_x64（MSI 7.78MB、NSIS 5.25MB、gui 22.13MB、tui 61.88MB）。

### 062
* 更新时间：2026-06-19 16:20:00
* 涉及模块：Composer.tsx / App.tsx / styles.css
* 功能变更：Chat 输入区改为 Cursor 风格一体化 Composer：圆角容器、底栏 ∞ Agent / 模型 pill 选择、圆形 ↑ 发送；Shell/信任/自动批准/系统提示词收入 ⋯ 菜单。
* 实现说明：移除独立 chat-bottom-bar；输入框自动增高；focus 时容器高亮边框。

### 063
* 更新时间：2026-06-19 16:25:00
* 涉及模块：发布打包（Tauri bundle）
* 功能变更：重新构建并打包桌面安装器（含 Cursor 风格一体化 Composer #062）。
* 实现说明：`npm run build` + `C:\dsgui` 下 `npm run tauri:build`；产物 0.1.0_x64（MSI 7.78MB、NSIS 5.25MB、gui 22.13MB、tui 61.88MB）。

### 065
* 更新时间：2026-06-19 16:40:00
* 涉及模块：SettingsView / App.tsx / ConfigView / TasksView / RulesView / SkillsView / styles.css
* 功能变更：Chat 顶栏归档/连接/任务/规则/技能图标移入统一「设置」页；Cursor 风格左侧分类导航（模型、连接、任务、规则、技能、聊天）；活动栏移除独立规则入口。
* 实现说明：新增 SettingsView 整合各子模块 embedded 模式；后端连接与「显示已归档会话」在设置内；Chat 顶栏仅保留搜索、斜杠命令与可点击状态点。

### 066
* 更新时间：2026-06-19 16:45:00
* 涉及模块：App.tsx
* 功能变更：Agent 创建/修改文件时不再自动打开编辑器标签，仅刷新左侧文件树；用户仍可通过文件树或变更卡片手动打开。
* 实现说明：移除 fileChangeTick/usageTick 中对 `editor.openFilesBatch` 的调用，保留 treeTick 增量刷新与运行中 2s 轮询。

### 067
* 更新时间：2026-06-19 16:50:00
* 涉及模块：EditorPanel / useEditorTabs / styles.css
* 功能变更：编辑器标签支持批量关闭（仿 Cursor/VS Code）：关闭、关闭其他、关闭左侧、关闭右侧、关闭全部；标签右键或 ⋯ 菜单触发；含未保存确认。
* 实现说明：useEditorTabs 新增 closeOthers/closeToRight/closeToLeft；EditorPanel 右侧 ⋯ 与右键 TabCloseMenu。

### 068
* 更新时间：2026-06-19 17:05:00
* 涉及模块：runtime_threads.rs / threadTitle.ts / App.tsx / ThreadSearch.tsx / ThreadList.tsx / i18n
* 功能变更：Agent 会话命名仿 Cursor：未发送消息时标签显示「新对话」而非 `thr_` id；首条用户消息首行自动截断为标题（32 字）；历史无标题会话在 list/get 时补全。
* 实现说明：后端 `derive_thread_title_from_text` + `start_turn` 首回合写入 title；前端 `formatThreadTabTitle` 统一展示，`onSend` 合并 API 返回 thread 并乐观更新。

### 069
* 更新时间：2026-06-19 17:15:00
* 涉及模块：发布打包（Tauri bundle）
* 功能变更：重新构建并打包桌面安装器（含 Cursor 风格会话命名 #068）。
* 实现说明：`cargo build --release -p deepseek-tui` + 真实路径 `npm run build` + `C:\dsgui` 下 `npm run tauri:build`；`deepseek-tui.exe` 已复制至 release 与 GUI 同目录；产物 0.1.0_x64（MSI 7.78MB、NSIS 5.25MB、gui 22.13MB、tui 61.83MB）。

### 070
* 更新时间：2026-06-19 17:25:00
* 涉及模块：App.tsx / SettingsView.tsx / styles.css
* 功能变更：设置页从中栏右侧 Chat 区域移至中间编辑器主区域显示（仿 Cursor）；打开设置时 Chat 面板保持可见可对话。
* 实现说明：`pane-center` 在 `showSettings` 时渲染 `SettingsView`，否则渲染 `EditorPanel`；补充 `.pane-center .settings-shell` 满高样式。

### 071
* 更新时间：2026-06-19 17:40:00
* 涉及模块：TitleMenuBar / EditorEmptyState / EditorPanel / App.tsx / styles.css
* 功能变更：仿 Cursor 顶栏菜单（文件/编辑/选择/视图/转到/运行/终端/帮助）与中央搜索条；编辑器空状态改为居中引导 + 可点击快捷键列表。
* 实现说明：菜单仅接线已有能力（打开文件夹、快速打开、命令面板、设置、会话搜索等）；Undo/终端等占位 disabled；不要求实现 Cursor 全量菜单功能。

### 072
* 更新时间：2026-06-19 17:50:00
* 涉及模块：发布打包（Tauri bundle）
* 功能变更：重新构建并打包桌面安装器（含顶栏菜单 + 编辑器空状态 #071、设置居中 #070、会话命名 #068）。
* 实现说明：`cargo build --release -p deepseek-tui` + 真实路径 `npm run build` + `C:\dsgui` 下 `npm run tauri:build`；`deepseek-tui.exe` 已复制至 release 同目录；产物 0.1.0_x64（MSI 7.79MB、NSIS 5.25MB、gui 22.13MB、tui 61.83MB）。

### 073
* 更新时间：2026-06-19 18:05:00
* 涉及模块：tauri.conf.json / WindowControls / TitleMenuBar / capabilities / styles.css
* 功能变更：方案 A 自定义标题栏：`decorations: false`，菜单与最小化/最大化/关闭合并为单行 Cursor 风格顶栏；空白区可拖动窗口、双击最大化。
* 实现说明：`WindowControls` 组件 + `data-tauri-drag-region` 拖拽区；`theme: Dark`；补充 window 权限；已重新打包 0.1.0_x64（MSI 7.79MB、NSIS 5.26MB、gui 22.14MB、tui 61.83MB）。

### 074
* 更新时间：2026-06-19 18:15:00
* 涉及模块：TitleMenuBar / App.tsx / styles.css
* 功能变更：顶栏搜索框改为三列网格真正居中；设置齿轮移至顶栏右侧（窗口按钮左侧）；活动栏移除重复设置入口。
* 实现说明：`grid-template-columns: 1fr minmax(280px,480px) 1fr`；`title-menu-icon` 切换设置页；已重新打包 0.1.0_x64（MSI 7.79MB、NSIS 5.26MB、gui 22.14MB、tui 61.83MB）。

### 075
* 更新时间：2026-06-19 18:25:00
* 涉及模块：fileIcons.tsx / FileTree.tsx / styles.css
* 功能变更：左侧资源管理器仿 Cursor/VS Code：按扩展名 SVG 彩色文件图标（JS/TS/RS/MD 等）、文件夹主题色、Chevron 展开箭头、22px 行高、选中左侧蓝条、子级缩进引导线。
* 实现说明：新增 `FileTypeIcon`/`FolderIcon`/`TreeChevron`；移除 emoji 图标；已重新打包 0.1.0_x64（MSI 7.79MB、NSIS 5.26MB、gui 22.14MB、tui 61.83MB）。

### 076
* 更新时间：2026-06-19 18:35:00
* 涉及模块：uiZoom / useUiZoom / StatusZoom / CodeView / App.tsx / TitleMenuBar / capabilities
* 功能变更：全局界面缩放仿 Cursor：Tauri `setWebviewZoom` 整窗缩放；状态栏 − / 100% / 🔍+ 控件；Ctrl+=/−/0 与 Ctrl+滚轮；视图菜单增缩放项；移除编辑器局部字号缩放。
* 实现说明：缩放持久化 `ds_ui_zoom`（50%–250%）；已重新打包 0.1.0_x64（MSI 7.79MB、NSIS 5.26MB、gui 22.14MB、tui 61.83MB）。

### 077
* 更新时间：2026-06-19 19:10:00
* 涉及模块：SettingsView.tsx / styles.css
* 功能变更：设置页布局优化：内容区居中列（max-width 680px）、四周留白（32/40/48px）、各 Tab 卡片纵向等距；连接/聊天区块统一卡片样式。
* 实现说明：新增 `settings-panel-inner` 容器；嵌入视图移除重复 padding；已重新打包 0.1.0_x64。

### 078
* 更新时间：2026-06-19 19:45:00
* 涉及模块：EditorEmptyState / styles.css / runtime_threads.rs / App.tsx / useConversation.ts / client.ts
* 功能变更：编辑器空状态改为居中卡片 + 四周留白 + 快捷键与装饰图标并排（窄屏换行）；修复转向时 `Thread is not loaded` HTTP 400。
* 实现说明：后端 `prepare_active_turn_engine` 在 steer/interrupt 前重载引擎并恢复 active_turn；前端切换线程时对照 API 校正 running 状态，steer 失败时 resume 重试或降级为 startTurn。

### 079
* 更新时间：2026-06-19 20:05:00
* 涉及模块：ConfigView.tsx
* 功能变更：编辑已配置档案时 API Key 输入框显示 `********` 掩码，不再空白；保存/测试时掩码未改动则不覆盖原 Key。
* 实现说明：新增 `KEY_MASK` 与 `resolveApiKeyForSubmit`；掩码态用 `type=text` 显示星号，聚焦清空、失焦恢复；已重新打包 0.1.0_x64。

### 080
* 更新时间：2026-06-19 20:20:00
* 涉及模块：runtimeStatus.ts / MessageItem.tsx / App.tsx
* 功能变更：聊天区「状态」消息中文化，如串行执行工具、会话上下文已同步等；界面为中文时自动翻译后端英文 status。
* 实现说明：新增 `translateRuntimeStatus`，精确匹配 + 正则覆盖常见引擎状态文案；已重新打包 0.1.0_x64（NSIS 5.26MB、MSI 7.79MB、gui 22.15MB、tui 62.21MB）。

### 081
* 更新时间：2026-06-19 20:35:00
* 涉及模块：useTranscriptAutoScroll.ts / App.tsx
* 功能变更：发送/转向消息后聊天区自动滚到底部；流式回复时在底部则持续跟随；切换会话时滚至最新消息。
* 实现说明：新增 `useTranscriptAutoScroll` Hook，绑定 `.transcript` 容器 ref；已重新打包 0.1.0_x64（NSIS 5.26MB、MSI 7.79MB、gui 22.15MB、tui 62.21MB）。

### 082
* 更新时间：2026-06-19 20:50:00
* 涉及模块：RulesView.tsx / tauri.ts / FileTree.tsx
* 功能变更：修复新增第二条项目规则时「保存失败: undefined」；保存时不再对已存在的 `.cursor/rules` 目录重复 createDir。
* 实现说明：根因是目录已存在时 createDir 报错，且 Tauri reject 为 string 导致 message 为 undefined；新增 `formatInvokeError` 统一解析错误文案；已重新打包 0.1.0_x64。

### 083
* 更新时间：2026-06-19 21:05:00
* 涉及模块：CodeView.tsx / package.json
* 功能变更：编辑器补充 Java/C/C++/SQL/XML 等语法高亮；`.java` 不再整页同色，关键字/字符串/注释分色（VS Code 暗色主题）。
* 实现说明：新增 `@codemirror/lang-java`、`lang-cpp`、`lang-sql`、`lang-xml`；扩展 `langExtension` 映射；已重新打包 0.1.0_x64。

### 064
* 更新时间：2026-06-19 16:30:00
* 涉及模块：MessageItem.tsx / styles.css
* 功能变更：用户消息改为 Cursor 风格圆角 pill：去掉「我」标签与气泡框，全宽胶囊条 + 右侧 ↵ 图标；助手消息保持无框正文。
* 实现说明：新增 `UserMessagePill` 组件与 `.user-msg-pill` 样式。

### 084
* 更新时间：2026-06-19 22:30:00
* 涉及模块：project_context.rs / ruleCompliance.ts / useRuleCompliance.ts / RuleComplianceBanner.tsx / App.tsx / cursorRules.ts / i18n / styles.css
* 功能变更：项目规则从「仅注入提示词」升级为「可执行合规」：alwaysApply 规则（如必须写 README）在回合结束后自动检测；缺 README 变更记录时自动发起跟进回合，仍缺口则显示横幅一键补充。
* 实现说明：后端解析 .mdc frontmatter 并注入 Mandatory 前言；前端 loadCursorRules + detectReadmeChangelogGap；useRuleCompliance 防重复自动跟进；新增 readmeChangelogRuleTemplate 模板。

### 085
* 更新时间：2026-06-19 23:15:00
* 涉及模块：ruleCompliance/* / cursorRules.ts / RulesView.tsx / useRuleCompliance.ts / RuleComplianceBanner.tsx / project_context.rs / i18n / styles.css
* 功能变更：规则合规扩展为可插拔检查器注册表；支持 5 类 compliance（readme_changelog、code_comments、database_schema、tests_required、api_docs）；.mdc 新增 frontmatter compliance: 字段；多项缺口合并自动跟进；规则页提供 5 种快捷模板。
* 实现说明：无 explicit compliance 时仍按关键词回退匹配；新增检查器只需在 checkers.ts 注册并实现 detect/buildPrompt；注释类规则无法路径验证，回合结束后触发 Agent 自检。

### 086
* 更新时间：2026-06-19 17:55:00
* 涉及模块：EditorPanel.tsx
* 功能变更：修复编辑器标签右键菜单「关闭 / 关闭其他 / 关闭全部」等项点击无效。
* 实现说明：根因是 document mousedown 在 click 前关闭浮层且未排除 editor-tab-menu-float；新增 menuFloatRef 并在菜单上 stopPropagation。

### 087
* 更新时间：2026-06-19 18:10:00
* 涉及模块：codemirrorLang.ts / CodeView.tsx / package.json
* 功能变更：.properties / .ini / .env 使用专用 properties 语法高亮（键、值、注释分色）；.yaml/.toml 改用 legacy-modes 高亮。
* 实现说明：新增 @codemirror/legacy-modes 与 langExtensionsForPath；修复此前误用 markdown 导致 application.properties 几乎无高亮。

### 088
* 更新时间：2026-06-19 18:25:00
* 涉及模块：styles.css（title-menu-bar）
* 功能变更：修复顶栏「帮助」菜单与中央搜索框文字重叠。
* 实现说明：搜索条改为绝对居中（仿 Cursor）；左侧菜单 max-width + overflow:hidden，搜索按钮 z-index 与 pointer-events 分离。

### 089
* 更新时间：2026-06-19 18:35:00
* 涉及模块：ComposerPillDropdown.tsx / Composer.tsx / styles.css
* 功能变更：Plan/Agent/YOLO 与模型切换改为自定义深色下拉，修复 WebView2 原生 select 白底低对比、选项重叠问题。
* 实现说明：向上展开菜单、当前项 ✓ 标记、模式附带中英文说明副标题；已重新打包 0.1.0_x64。

### 090
* 更新时间：2026-06-19 19:00:00
* 涉及模块：uiZoom.ts / useUiZoom.ts / main.tsx / styles.css（title-menu-bar）
* 功能变更：界面缩放对齐 Cursor/VS Code：按 zoom level 步进（每级约 20%，1.2^level）；顶栏菜单缩放时不再竖排换行。
* 实现说明：持久化改为 ds_ui_zoom_level，自动迁移旧 ds_ui_zoom；顶栏改为三列 grid + nowrap 溢出裁剪；监听 tauri://scale-change 在 DPI 变化后重应用缩放；浏览器首帧前同步恢复 zoom。

### 091
* 更新时间：2026-06-19 20:30:00
* 涉及模块：src-tauri/lsp/* / lsp/* / useEditorLsp.ts / CodeView.tsx / api/tauri.ts / package.json
* 功能变更：编辑器一步到位接入 LSP IntelliSense（补全、悬停、诊断）；Rust stdio 桥 + @codemirror/lsp-client；支持 rust-analyzer、pyright、gopls、clangd、typescript-language-server。
* 实现说明：Tauri 命令 lsp_start_session/lsp_send/lsp_stop_session；前端 TauriLspTransport + 会话池按 workspace+语言复用；需本机 PATH 安装对应 language server；Java 暂未默认 jdtls。

### 092
* 更新时间：2026-06-19 21:00:00
* 涉及模块：styles.css（title-menu-bar）
* 功能变更：修复界面缩放时顶栏菜单文字拆成上下两行的问题。
* 实现说明：菜单 trigger 设 inline-flex + nowrap + min-width:max-content + flex:0 0 auto，禁止 flex 压扁；nav 用 max-content；搜索条 min-width 降至 120px 让出左侧空间；顶栏改 min-height 32px。

### 093
* 更新时间：2026-06-19 21:30:00
* 涉及模块：TitleMenuBar.tsx / useUiZoom.ts / styles.css
* 功能变更：缩放或窗口变窄时，右侧放不下的菜单（终端、帮助等）自动隐藏，避免与搜索框重叠。
* 实现说明：ResizeObserver + ds-ui-zoom 事件重算可见数量；从右向左 display:none；与搜索条保留 16px 间距。

### 094
* 更新时间：2026-06-19 22:00:00
* 涉及模块：打包发布
* 功能变更：重新打包 0.1.0_x64，含顶栏缩放溢出菜单自动隐藏（093）。
* 实现说明：NSIS 5.39MB、MSI 7.94MB、gui 22.5MB、tui 61.8MB；产物路径 C:\dsgui\Deepseek-GUI\src-tauri\target\release\bundle\

### 095
* 更新时间：2026-06-19 22:30:00
* 涉及模块：providerPresets.ts / ConfigView.tsx
* 功能变更：模型配置服务商扩展为 17 项（DeepSeek/OpenAI/NVIDIA NIM/OpenRouter/Groq/Together/Moonshot/智谱/硅基流动/Ollama/vLLM/SGLang 等）；切换预设自动填充 Base URL 与默认模型；模型名称 datalist 按服务商给出候选。
* 实现说明：OpenAI 兼容网关仍写 provider=openai；与 deepseek-tui ProviderKind 对齐；自定义 URL 时回落「自定义」项。

### 096
* 更新时间：2026-06-19 23:15:00
* 涉及模块：tabCompletionExtension.ts / tabCompletionSettings.ts / tabCompletionService.ts / TabSettingsPanel.tsx / SettingsView.tsx / CodeView.tsx / main.rs / api/tauri.ts / styles.css
* 功能变更：新增 Cursor Tab 风格 AI 内联补全：幽灵文本预览、Tab 接受、Ctrl+→ 逐词接受、Esc 取消；设置页新增 Tab 分类（开关、注释内建议、空白建议、忽略 glob）；Tauri 命令 tab_complete 调用 DeepSeek chat completions。
* 实现说明：与 LSP IntelliSense 互补；默认忽略 *.md 与 generated；TS/Python 自动 import 预留占位；需 config.toml 配置 API Key 与 default_text_model。

### 097
* 更新时间：2026-06-19 23:45:00
* 涉及模块：docs/GUI-TUI-ROADMAP.md
* 功能变更：新增 GUI 对齐 TUI 功能路线图文档，分四阶段（快赢 / 配置面板 / 新 API / 打磨）与 M1–M4 里程碑。
* 实现说明：含 30+ 项任务 ID、工作量、优先级、HTTP API 缺口与 Top 5 开工建议；详见 `Deepseek-GUI/docs/GUI-TUI-ROADMAP.md`。

### 098
* 更新时间：2026-06-20 00:15:00
* 涉及模块：slashCommands.ts / executeSlashCommand.ts / UsageModal.tsx / Composer.tsx / App.tsx / reasoningEffort.ts / main.rs / styles.css / ConfigView.tsx
* 功能变更：M1 阶段一落地：斜杠命令扩展（/mode /trust /retry /export /workspace /task /provider /profile /attach /tokens 等）；Composer 斜杠自动补全；Shift+Tab 推理强度切换；/cost|/tokens 用量弹窗；Tauri pick_file。
* 实现说明：命令逻辑抽至 executeSlashCommand；/help 从注册表生成；YOLO 模式自动开 trust+auto_approve；会话 JSON 导出为浏览器下载。

### 099
* 更新时间：2026-06-20 01:30:00
* 涉及模块：config_bridge.rs / main.rs / McpSettingsPanel.tsx / HooksPanel.tsx / NetworkPanel.tsx / SubagentsPanel.tsx / JobsPanel.tsx / SettingsView.tsx / SkillsView.tsx / slashCommands.ts / executeSlashCommand.ts / api/tauri.ts / styles.css
* 功能变更：M2 完成：MCP/Hooks/Network 配置面板（Tauri 读写本地配置，保存后 restart_backend）；斜杠 /mcp /hooks /network；M3 起步：Subagents 只读面板、Jobs 占位（展示运行中 Tasks）；Skills 页跳转 MCP 管理。
* 实现说明：config_bridge 读 ~/.deepseek 下 mcp.json 与 config.toml [hooks]/[network]；subagents 读工作区 .deepseek/subagents.v1.json 每 5s 轮询；Jobs 说明 HTTP 尚无 /v1/jobs 差距。

### 100
* 更新时间：2026-06-20 01:30:00
* 涉及模块：docs/GUI-TUI-ROADMAP.md
* 功能变更：路线图标记 M2（2.1–2.3）完成；M3 部分交付（3.1 只读 Jobs、3.3 Subagents 本地读）。
* 实现说明：3.2 集成终端、3.4 RLM 仍待 HTTP API 或后续 sprint。

### 101
* 更新时间：2026-06-20 02:45:00
* 涉及模块：runtime_api.rs / runtime_threads.rs / tools/spec.rs / core/engine.rs / JobsPanel.tsx / SubagentsPanel.tsx / api/client.ts / api/types.ts
* 功能变更：M3 续：新增 HTTP API /v1/jobs（list/detail/cancel/stdin）与 /v1/subagents（list/detail/cancel）；HTTP 运行时挂载共享 Shell/Subagent 管理器；GUI Jobs/Subagents 面板升级为可操作 UI。
* 实现说明：RuntimeThreadManager.attach_shell_manager/subagent_manager；引擎 RuntimeToolServices 复用同一实例；Subagents 面板 API 失败时回退读 subagents.v1.json；集成终端(3.2)/RLM(3.4)仍待后续。

### 102
* 更新时间：2026-06-20 03:30:00
* 涉及模块：rlm/session.rs / runtime_threads.rs / runtime_api.rs / pty.rs / main.rs / RlmPanel.tsx / TerminalPanel.tsx / SettingsView.tsx / api/client.ts / package.json / styles.css / slashCommands.ts
* 功能变更：M3 收尾：GET /v1/rlm/sessions 与 RlmPanel；Tauri PTY + xterm 集成终端（Terminal Tab）；斜杠 /rlm /terminal；共享 RLM session store 挂载到 HTTP 运行时。
* 实现说明：portable-pty 0.8；pty-output 事件推送；终端默认 PowerShell（Windows）或 \；需重装 deepseek-tui 以启用 RLM/Jobs API。

### 103
* 更新时间：2026-06-20 04:15:00
* 涉及模块：发布打包（Tauri bundle / deepseek-tui）
* 功能变更：重新打包 0.1.0_x64，含 M1–M3 全部近期功能（斜杠命令、MCP/Hooks/Network、Jobs/Subagents/RLM、集成终端 PTY+xterm）。
* 实现说明：cargo build --release -p deepseek-tui + 真实路径 
pm run build + C:\dsgui junction 下 
pm run tauri:build；deepseek-tui.exe 已复制至 release 与 GUI 同目录；产物：NSIS 5.56MB、MSI 8.17MB、gui 22.99MB、tui 62.16MB；路径 C:\dsgui\Deepseek-GUI\src-tauri\target\release\bundle\。

### 104
* 更新时间：2026-06-20 04:35:00
* 涉及模块：Composer.tsx / styles.css
* 功能变更：修复 Chat 面板较窄时 Composer 底栏 pill 换行上下重叠；输入框增高上限 160px，超出后禁止继续拉高、改为内部滚动。
* 实现说明：toolbar 改为单行 nowrap + 横向滚动；composer-box flex 纵向布局；textarea max-height 与 JS 常量 COMPOSER_INPUT_MAX_H 对齐。

### 105
* 更新时间：2026-06-20 09:40:00
* 涉及模块：config_bridge.rs / main.rs / api/tauri.ts / MemoryPanel.tsx / SettingsView.tsx / slashCommands.ts / executeSlashCommand.ts / styles.css
* 功能变更：M4 对齐 TUI——新增记忆/笔记/锚点面板（设置页 Memory Tab），斜杠 /memory /note /anchor 跳转；读写 ~/.deepseek/memory.md 与工作区 .deepseek/notes.md、anchors.md，与 TUI 互通。
* 实现说明：Tauri 命令 get_memory/save_memory_cmd/get_notes/save_notes_cmd/get_anchors/save_anchors_cmd；条目以 \\n---\\n 分隔；整列表覆盖写入。

### 106
* 涉及模块：executeSlashCommand.ts / slashCommands.ts
* 更新时间：2026-06-20 09:45:00
* 功能变更：补齐斜杠命令 /agent（开持久子代理，发 agent_open 指令）、/relay（生成 .deepseek/handoff.md 接力）、/settings /config /load /save，以及 /undo /restore（指示模型用 revert_turn 回滚上一回合改动）。
* 实现说明：均复用现有 onSend/openSettings/setShowSessions，无需新增后端接口；/agent 支持深度前缀 N(0-3)。

### 107
* 更新时间：2026-06-20 09:50:00
* 涉及模块：config_bridge.rs / main.rs / api/tauri.ts / TrustPanel.tsx / SettingsView.tsx / executeSlashCommand.ts
* 功能变更：新增信任目录面板（设置页 Trust Tab），/trust list|add|remove 打开；读写 ~/.deepseek/workspace-trust.json，按工作区分组，与 TUI workspace_trust 互通。
* 实现说明：Tauri 命令 get_trust/add_trust_cmd/remove_trust_cmd；路径用 std canonicalize 规范化以匹配 TUI 键（Windows 带 \\?\\ 前缀）；/trust on|off 仍切换信任模式。

### 108
* 更新时间：2026-06-20 09:55:00
* 涉及模块：App.tsx / QueueBar.tsx / executeSlashCommand.ts / slashCommands.ts / styles.css
* 功能变更：新增 Composer 消息队列/暂存（对齐 TUI /queue /stash）；回合进行中排队、结束后自动按序发送；暂存停泊到 localStorage 可弹回。队列条展示在 Composer 上方，支持编辑/删除/清空/整体暂存。
* 实现说明：纯 GUI 本地状态，未改后端；自动排空用 useEffect 监听 conv.running；/queue <消息>|clear|stash、/stash [pop|clear]。

### 109
* 更新时间：2026-06-20 10:05:00
* 涉及模块：runtime_api.rs / api/client.ts / api/types.ts / SnapshotsModal.tsx / App.tsx / executeSlashCommand.ts / styles.css
* 功能变更：快照还原真打通（对齐 TUI /restore /undo）。后端新增 GET /v1/threads/{id}/snapshots（按会话工作区列出 pre/post-turn 快照）与 POST /v1/threads/{id}/snapshots/restore（还原到指定快照，缺省取最近）；GUI 新增 SnapshotsModal 浏览/一键还原；/restore 无参开面板、/restore N 还原第 N 新、/undo 还原最近 pre-turn 快照（不再仅指示模型）。
* 实现说明：后端直接复用 snapshot::repo::SnapshotRepo（side-git），git 操作放 spawn_blocking；还原前自动打 pre-restore 安全快照便于反悔。已 cargo check 通过并重建 release deepseek-tui.exe（65402071B）覆盖 GUI 同目录 sidecar；前端 npm run build 通过。还原后 GUI 刷新文件树，已打开文件需重新打开以加载新内容。

### 110
* 更新时间：2026-06-20 10:30:00
* 涉及模块：utils/editorCommands.ts（新增）/ TitleMenuBar.tsx / CodeView.tsx / 发布打包
* 功能变更：顶部菜单栏仿 VS Code 真正可用。文件：新增 保存(Ctrl+S)/退出；编辑：撤销/重做/剪切/复制/粘贴/查找全部生效；选择：全选/展开选择；视图：新增 集成终端入口；终端：新建终端打开 PTY 终端面板（移除占位提示）。
* 实现说明：新增「编辑命令总线」editorCommands.ts，通过全局 focusin 跟踪「最后聚焦目标」——CodeMirror 视图走原生 undo/redo/selectAll/selectParentSyntax/openSearchPanel，普通 input/textarea 走 execCommand + navigator.clipboard，统一覆盖代码编辑器与 Composer 输入框；CodeView 用 onCreateEditor 注册视图、卸载时注销，并监听 ds-editor-save 响应菜单保存（仅当前可见标签）；终端/退出复用 onOpenSettings("terminal") 与 Tauri getCurrentWindow().close()。已 npm run build 通过并重打 NSIS/MSI 安装包。

### 111
* 更新时间：2026-06-20 10:33:00
* 涉及模块：TitleMenuBar.tsx
* 功能变更：修复顶部菜单「点了没反应/下拉不出现」。根因：下拉为 position:absolute，被顶栏 .title-menu-nav/.title-menu-left 的 overflow:hidden 裁剪掉，导致看不见也点不到。
* 实现说明：下拉改为 createPortal 渲染到 document.body，用 position:fixed 按触发按钮 getBoundingClientRect 定位（右溢出钳制）；外部点击监听放行 .title-menu-dropdown 内点击，避免 mousedown 提前关闭导致菜单项 onClick 不触发；滚动/缩放时自动关闭防错位。已 npm run build 通过并重打 NSIS/MSI 安装包；需重新安装或重启应用生效。

### 112
* 更新时间：2026-06-20 10:18:00
* 涉及模块：App.tsx / TitleMenuBar.tsx / TerminalPanel.tsx / SettingsView.tsx / styles.css / src-tauri/main.rs / api/tauri.ts / tabCompletionService.ts / codemirror/tabCompletionExtension.ts / TabSettingsPanel.tsx
* 功能变更：处理三项反馈。①终端改为底部停靠面板（仿 VS Code/Cursor）：Ctrl+` 或菜单切换，可拖动高度，从设置页移除终端 Tab。②修复「设置点多了卡死」：终端不再随设置 Tab 反复挂载/重复 spawn PTY，改为首次打开后常驻单实例（切显隐保活）。③补全 Tab 的两项「自动 Import」（TypeScript/Python）：启用开关，按语言在补全请求中携带 autoImport，后端 tab_complete 据此在系统提示中要求补全自动补上缺失 import。
* 实现说明：底部终端在 pane-center 内 center-main(flex:1)+terminal-dock(高度 JS 控制) 布局，TerminalPanel 新增 fill 模式与 ResizeObserver 自适应；终端 everOpened 后保持挂载、用 is-hidden 切显隐以保活 PTY；autoImport 经 tauri.ts→service→extension 透传，main.rs tab_complete 新增 auto_import 参数并拼接 import 指令。已 npm run build + cargo（随 tauri:build）通过并重打 NSIS/MSI 安装包；需重装或重启生效。

### 113
* 更新时间：2026-06-20 10:35:00
* 涉及模块：App.tsx / TitleMenuBar.tsx / styles.css
* 功能变更：仿 Cursor 三项布局调整。①终端「往左到底」：终端停靠从 pane-center 内移出，改为 .ide 网格底部行（grid-row 2，跨「侧栏+编辑器」两列 grid-column 2/4），活动栏与右侧聊天整列全高。②两边收齐：顶栏新增「左侧栏」「右侧聊天」两个面板开关按钮，右侧聊天可收起（.ide.chat-collapsed 将第 4 列宽设为 0）。③聊天头合并成一行：去掉「CHAT」标题行，标签页与操作图标（历史/搜索//命令/状态点）合并为单行（.chat-top-row + flex order），历史记录入口从活动栏时钟按钮移入右侧聊天头部，新建会话仍用「+」（仿 Cursor）。
* 实现说明：.ide 新增 grid-template-rows: minmax(0,1fr) auto 与显式行列定位（活动栏/聊天 grid-row 1/3 全高，终端 grid-column 2/4 grid-row 2）；新增 chatOpen 状态与 .ide.chat-collapsed / .pane-right.collapsed 样式；TitleMenuBar 新增 onToggleChat/chatOpen 及两枚 SVG 面板切换按钮；聊天头 .chat-top-row 用 flex order 将标签置左、操作置右，历史按钮调用 toggleView("sessions")。已 npm run build 通过并重打 NSIS/MSI 安装包；需重装或重启生效。

### 114
* 更新时间：2026-06-20 11:05:00
* 涉及模块：App.tsx / TitleMenuBar.tsx / EditorPanel.tsx / PanelToggleButton.tsx / AgentHistoryPanel.tsx / styles.css / useResizablePanels.ts
* 功能变更：移除左侧活动栏（与侧栏开关冲突）；左侧收起按钮移至资源管理器 pane-head 最左及侧栏收起后的 IDE 左缘悬浮位（仿 Cursor）；顶栏右侧两枚面板开关移除，右侧聊天开关改到聊天头最右；新增 Cursor 风格 Agent 历史模块（搜索、新建会话 Ctrl+N 提示、按今天/昨天/本周/更早分组列表），点击历史图标在右栏全屏展示，选中会话后回到聊天。
* 实现说明：.ide 网格改为三列（去掉 --act-w 活动栏列），终端 grid-column 1/3；新增 PanelToggleButton 复用组件、AgentHistoryPanel 替代原 SessionsView 全页切换；sidebar-edge-toggle / chat-edge-toggle 在面板收起时提供边缘 reopen；useResizablePanels ACT_W 改为 0。已 npm run build 通过。

### 115
* 更新时间：2026-06-20 11:02:00
* 涉及模块：发布打包 / tauri.conf.json / scripts/build-release.ps1 / 项目路径
* 功能变更：文件夹重命名为 `DeekSeel-TUI-GUI`（无空格）后完整重建并打包；安装包现包含 `deepseek-tui.exe` sidecar；新增一键构建脚本。
* 实现说明：`tauri.conf.json` 增加 `bundle.resources` 将 `bin/deepseek-tui.exe` 打入安装目录；`scripts/build-release.ps1` 串联 tui 编译、前端 build、sidecar 复制与 tauri:build。产物 0.1.0_x64：NSIS 20.44MB、MSI 30.09MB、gui 22.99MB、tui 62.38MB；路径 `Deepseek-GUI\src-tauri\target\release\bundle\`。

### 116
* 更新时间：2026-06-20 11:15:00
* 涉及模块：TitleMenuBar.tsx / App.tsx / styles.css
* 功能变更：左右侧栏开关移至顶栏菜单同一行（左开关在最左、右开关在最右，仿 Cursor）；移除资源管理器 pane-head 与聊天头内的重复开关及收起时的悬浮边缘开关。
* 实现说明：TitleMenuBar 左侧品牌前、右侧设置按钮前各放一枚 PanelToggleButton；顶栏收起后仍可切换面板。已 npm run build 通过。

### 117
* 更新时间：2026-06-20 12:30:00
* 涉及模块：ComposerPillDropdown.tsx / Composer.tsx / App.tsx / styles.css
* 功能变更：修复 Chat 底栏 Composer（Agent/模型/推理 pill 与发送按钮）无法点击的问题。
* 实现说明：分栏 resizer z-index 降至 5、Chat 面板 z-index 6/8，避免 4px 拖拽条误挡底栏；transcript 增加 min-height:0 防止 flex 溢出盖住 footer；pill 下拉 portal 到 body 防 overflow 裁剪；底栏与 pill 触发改用 mousedown；模态浮层移出 .ide 网格；Tauri 无边框窗口为 composer/chat-footer 加 no-drag。已 npm run build 通过。

### 118
* 更新时间：2026-06-20 11:45:00
* 涉及模块：useResizablePanels.ts / TitleMenuBar.tsx / EditorEmptyState.tsx / styles.css
* 功能变更：修复三栏缩小时 Chat 底栏控件被裁切、左侧栏开关消失、中间空状态挤压三项布局问题。
* 实现说明：Chat 最小宽度提升至 380px 且拖拽/窗口缩放不再压低于此；顶栏改为四列网格，左右 PanelToggle 独立固定列；中间编辑器空状态改纵向卡片 + container query 窄屏隐藏说明文字；Composer 发送按钮固定右侧不收缩。已 npm run build 通过。

### 119
* 更新时间：2026-06-20 12:05:00
* 涉及模块：.github/workflows/gui-build.yml / tauri.conf.json / tauri.macos.conf.json / tauri.windows.conf.json / scripts/build-release.sh
* 功能变更：新增 GitHub Actions 云端打包 DeepSeek GUI（Windows NSIS + macOS DMG），无需实体 Mac。
* 实现说明：sidecar 按平台写入 `tauri.*.conf.json` resources；workflow 在 `macos-latest`/`windows-latest` 构建 deepseek-tui + tauri bundle，Artifacts 上传 `deepseek-gui-macos-arm64`（dmg+app.zip）与 `deepseek-gui-windows-x64`（nsis/msi）。本地 Mac 可用 `scripts/build-release.sh`。
