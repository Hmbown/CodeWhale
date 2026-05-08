# DeepMap -- DeepSeek-TUI 的 AI 原生代码库地图引擎

## 起源与动机

代码库自动映射的概念由
[aider](https://github.com/Aider-AI/aider) 率先提出。aider 是第一个认识到
AI 编程助手在修改代码之前，需要先理解项目结构才能做出有意义的修改的
CLI 工具。aider 首创了一套将 tree-sitter 解析与 PageRank 排序相结合的
流水线，生成一份紧凑的"项目地图"——一份大约 1000 token 的 Markdown 文档，
按结构重要性排序，展示项目的入口点、热点文件和关键符号。

aider 的核心洞察既简单又深刻：一份 1000 token 的结构化地图，在帮助 AI
理解"应该在哪里修改"这件事上，始终优于 50000 token 的原始代码转储。
原始代码倾泻会淹没模型的注意力预算；而一份排好序的地图直接告诉它应该
优先看哪里。

在 aider 的基础上，我们开发了
[repomap](https://github.com/gjczone/repomap) 作为一个独立的开源研究项目。
repomap 在多个方向上扩展了原始概念：语言覆盖从 8 种扩展到 15 种，为大
型项目增加了增量扫描，引入了影响分析（给定一组修改的文件，还有哪些文件
需要关注？）、编辑后验证建议、LSP 集成用于实时符号解析，以及专为 AI
消费而非人工阅读设计的结构化 JSON 报告。

DeepMap 是这项工作的自然演进。它将 repomap 中经过验证的核心引擎用 Rust
重写，原生集成到 DeepSeek-TUI 中，成为一等公民的分析能力。如果说
repomap 是一个独立的 Python CLI 工具，那么 DeepMap 就活在 AI 编程助手的
同一个进程里——没有子进程，没有序列化开销，不需要单独部署。

repomap 和 DeepMap 共享同一个起源故事：它们都是由 @gjczone 构建的——
一位非程序员，使用 DeepSeek-V4-Pro，由 GLM-5.1 和 MIMO-V2.5-Pro
进行交叉验证和审查。整个项目本身就是一个证据：我们正在构建的、具备代码库
感知能力的 AI 原生编程工具，也能帮助它们的创造者构建有意义的基础设施。
"吃自己的狗粮"在这里不是偶然，而是重点。

## 我们要解决的问题

现代 AI 编程工具大致分为两类，它们与代码库理解的关系截然不同。

**基于 IDE 的编程助手**（Cursor、Trae、Qoder、GitHub Copilot）运行在完整
的开发环境中，可以访问语言服务器（LSP）。它们天生就知道：存在哪些符号、
它们在哪里定义、它们如何相互引用、以及当前有哪些错误。IDE 提供了全能补全、
跳转到定义、查找所有引用和重命名等功能——全部由实时更新的索引支撑。
对于这些工具来说，代码库意识是平台的内置功能。

**基于 CLI 的编程助手**（aider、Claude Code、DeepSeek-TUI）历史上一直
没有这个优势。它们在终端中运行，面对的是工作目录。它们不控制编辑器，
也没有运行中的 LSP 服务器。它们理解代码库的默认策略是退回到通用的
Unix 工具：用 `grep` 做文本搜索，用 `find` 或 `fd` 做文件发现，用暴力
读取文件来理解依赖关系。这种方法有三个根本问题。

第一，**token 浪费严重**。不知道哪些文件重要的 AI，会读入大量与当前
任务无关的文件。一个 1500 行的样板文件可能消耗 5000-10000 token，仅仅
为了确认它没有有用的内容。在一个有数百个文件的项目中，这种浪费在每个
会话中轻易达到数十万 token。

第二，**依赖关系对 grep 不可见**。文本搜索可以找到函数名出现的位置，
但它无法告诉你这些出现是调用还是注释。它无法告诉你模块 A 中的 `foo()`
经过一连串的重新导出，实际解析到模块 B 中定义的 `foo`。它无法构建调用图，
无法识别孤立代码，也无法追踪修改一个共享数据结构的影响范围。

第三，**变更影响评估完全靠人工**。当开发者修改了三个文件时，AI 需要
知道哪些其他文件依赖这些被修改的符号，需要重新验证。没有依赖图，AI
要么靠猜（冒着遗漏重要回归的风险），要么读取项目中的每一个文件（浪费
无上限的 token）。

DeepMap 填补了这个空白。它为基于 CLI 的 AI 提供了与 IDE 工具相同级别的
结构性感知能力，而不需要 IDE、MCP 服务器、运行中的 LSP 进程或任何外部
基础设施。它是一个编译进 DeepSeek-TUI 二进制文件的库，能在几秒内为
任何项目生成一份紧凑、高信噪比的"地图"。

## DeepMap 在 DeepSeek-TUI 中提供的能力

DeepMap 通过两个渠道暴露其能力：**TUI 工具**（AI 在对话中可以主动调用）
和 **CLI 命令**（开发者可以直接在 shell 中执行）。

### TUI 工具（对模型可见，AI 在对话中调用）

这些工具注册在 DeepSeek-TUI 的工具注册表中，通过系统提示告知模型。
模型根据任务上下文决定何时调用——例如，在进入一个新项目时调用
`deepmap_overview` 来了解代码布局，或者在追踪一个 bug 如何在系统中
传播时调用 `deepmap_call_chain`。

| 工具 | 描述 |
|------|------|
| `deepmap_overview` | 完整的项目地图报告：入口点、按 PageRank 排序的热点文件、扫描统计、推荐阅读顺序、模块摘要、按文件分组的关键符号。是 AI 了解陌生代码库的主要入口。 |
| `deepmap_call_chain` | 追踪任意符号的调用者和被调用者，支持配置深度。返回按深度分组、按 PageRank 排序的结构化列表，帮助 AI 优先关注最重要的节点。适用于调试、重构范围评估和影响分析。 |
| `deepmap_file_detail` | 列出单个文件中定义的每个符号，包括签名、类型、可见性、行范围和 PageRank 分数。AI 可以用它来了解某个文件的 API 面，而无需读取整个文件。 |
| `deepmap_query` | 基于主题的代码搜索，结合了关键词匹配、标识符拆分（驼峰式、蛇形式、烤串式）、文件角色分类和 IDF 加权。返回排序后的文件匹配结果、相关测试文件和高亮的关键符号。比纯 grep 更精确，因为它理解代码结构。 |

所有 TUI 工具都是**只读**且**可沙箱化**的，意味着它们从不修改文件系统，
可以在沙箱环境中运行。它们使用与现有 `project_map` 工具相同的**自动批准**
安全模型——不需要用户审批弹窗，因为这些工具不会产生副作用。

### CLI 命令（面向开发者，从终端调用）

这些命令让开发者可以不通过 AI 直接使用 DeepMap。它们尤其适用于 CI/CD
流水线、pre-commit 钩子和临时探索。

| 命令 | 用途 |
|------|------|
| `deepseek deepmap overview` | 生成并打印完整的项目地图到标准输出。等同于 `deepmap_overview` 工具。 |
| `deepseek deepmap call-chain --symbol <名字>` | 按名称追踪特定符号的调用链。支持可选的 `--direction`（调用者、被调用者或双向）和 `--depth` 参数。 |
| `deepseek deepmap file-detail --file <路径>` | 查看单个文件的符号表。以格式化表格展示所有符号及其类型、可见性、行范围和 PageRank 分数。 |
| `deepseek deepmap query --keywords <关键词>` | 在整个代码库中进行主题搜索。接受自然语言或代码术语，返回带相关测试文件的排序结果。 |
| `deepseek deepmap impact --files <a,b>` | 编辑前的影响分析：给定逗号分隔的变更文件路径列表，列出所有依赖它们的文件（直接和传递），并提供每个文件的指标。 |
| `deepseek deepmap diff-risk` | 完整的差异风险评估：结合影响分析和基于关键词的风险分类（auth、db、config 模式得分更高），以及验证建议，包括相关测试文件和推荐的测试命令。 |

### 会话缓存

DeepMap 使用两级缓存策略，使得重复调用非常快：

1. **内存级会话缓存**：会话中的第一次 `deepmap_overview` 调用会触发完整
   扫描，中等规模项目（5000-20000 个文件）通常需要 20-40 秒。扫描结果
   `RepoGraph`（符号、边、PageRank 分数）存储在以规范化的项目根路径为
   键的 `LazyLock<Mutex<HashMap>>` 中。同一会话中的所有后续工具调用——
   `deepmap_call_chain`、`deepmap_file_detail`、`deepmap_query` 等——
   复用这个内存缓存，返回时间在 10 毫秒以内。

2. **磁盘级持久缓存**：图结构序列化到
   `~/.cache/deepmap/{项目名}_{路径哈希}/symbol_cache.json`，带有模式版本
   守卫。下一次会话时，如果缓存存在且模式版本匹配，则完全跳过扫描，从
   磁盘加载图结构。缓存条目包含文件路径、大小和修改时间的 SHA-256 指纹，
   工作目录的变化会自动使缓存失效。写入是原子的（先写入临时文件，再重
   命名覆盖最终路径），并会自动备份之前的缓存。

这个设计意味着，AI 在某个项目中首次工作时只支付一次扫描成本，后续会话
几乎是瞬时的。

## 公开 API 面

DeepMap 通过 `deepmap` crate 暴露了一套聚焦的公开 API。以下类型和方法构成了 TUI 工具集成和 CLI 命令实现的主要接口。

### `RepoMapEngine` —— 主入口

核心调度器，拥有扫描流水线、图数据和分析查询的全部所有权。

| 方法 | 描述 |
|------|------|
| `new(project_root)` | 为指定的项目根路径创建新的引擎实例 |
| `get_or_scan(project_root, max_files, max_scan_secs)` | 带会话缓存的构造函数；如果该项目已在本进程中扫描过，返回缓存的引擎，否则执行完整扫描 |
| `scan(max_files, max_scan_secs)` | 执行完整的三阶段流水线：文件遍历、tree-sitter 解析和 PageRank 排名 |
| `query_symbol(name)` | 符号名的不区分大小写的子串搜索，返回按 PageRank 降序排列的匹配结果 |
| `call_chain(symbol_id, direction, max_depth)` | 调用图的 BFS 遍历；支持调用者、被调用者和双向遍历 |
| `hotspots(limit)` | 按 `symbol_count * average_PageRank` 排序的文件，识别最需要维护的关键文件 |
| `entry_points()` | 通过文件名干或路径模式启发式检测常见入口文件 |
| `suggested_reading_order(limit)` | 按接手优先级评分的文件列表，排除测试和噪音文件 |
| `module_summary(limit)` | 按顶层目录分组的符号，按总 PageRank 降序排列 |
| `is_scanned()` | 返回 `true` 如果扫描已完成 |
| `scan_summary_lines()` | 返回格式化的摘要文本，描述扫描时长、文件数量和符号数量 |

### `TreeSitterAdapter` —— 多语言解析器

管理 8 种语言的 tree-sitter 解析器和编译好的 S-expression 查询。

| 方法 | 描述 |
|------|------|
| `new()` | 初始化所有 8 种语言解析器（Rust、Python、JavaScript、TypeScript、Go、HTML、CSS、JSON） |
| `parse(content, lang)` | 使用相应的语言语法解析源代码内容 |
| `symbols()` | 从解析树中提取所有函数、类、导入和调用的捕获结果 |
| `imports()` | 提取导入声明，包含源模块和导入名称 |
| `calls()` | 提取调用表达式，包含被调用者名称和位置 |
| `import_bindings()`（仅 JS/TS）| ES 模块和 CommonJS 导入的结构化记录 |
| `exports()`（仅 JS/TS）| ES 模块和 CommonJS 导出的结构化记录 |

### `ImportResolver` —— 导入解析器

将导入语句解析为目标文件，并构建符号之间的依赖边。

| 方法 | 描述 |
|------|------|
| `new(project_root, graph)` | 使用项目根路径和图引用创建解析器 |
| `resolve_import_targets(source_file, import_path)` | 使用相对路径解析、tsconfig 别名和文件名干查找将导入路径解析为候选目标文件 |

### 关键数据类型

- **`Symbol`** —— 包含 `id`（`file:name:line` 复合键）、`name`、`kind`（function、class、struct、method、arrow_function 等）、`file_path`、`line_range`、`column`、`visibility`、`docstring`、`signature` 和 `pagerank_score`。
- **`Edge`** —— 两个符号 ID 之间的有向边，包含 `weight` 字段（调用为 0.50，导入为 0.35）。
- **`RepoGraph`** —— 完整的图容器：`symbols`、`outgoing`、`incoming`、`file_symbols`、`file_imports`、`file_calls`、`file_import_bindings`、`file_exports`。
- **`ScanStats`** —— 扫描元数据：`files_found`、`files_parsed`、`files_failed`、`filtered_large_files`、`total_symbols`、`total_edges`、`scan_duration_ms`、`timeout_triggered`、`failed_files`。

### 渲染器函数

`renderer` 模块中的六个 Markdown 报告生成器：

- `render_overview_report()` —— 包含入口点、热点文件、阅读顺序、模块摘要和关键符号的完整项目地图
- `render_call_chain_report()` —— 带深度分组的符号调用者/被调用者链
- `render_file_detail_report()` —— 以 Markdown 表格展示的每个文件的符号表
- `render_query_report()` —— 带相关测试发现的主题搜索结果
- `render_impact_report()` —— 带传递依赖列表的编辑前影响分析
- `render_diff_risk_report()` —— 带风险分类和验证建议的完整差异风险评估

## 失败模型

DeepMap 被设计为能够优雅地处理真实世界代码库中不完整、配置错误或包含病态文件的情况。以下契约描述了每种失败模式的处理方式。

| 场景 | 行为 |
|------|------|
| **语言没有可用的解析器** | 文件被静默跳过；继续扫描其他文件。不抛出错误。 |
| **文件过大（>10 MiB）** | 文件被跳过，计入 `filtered_large_files`。 |
| **AST 嵌套过深（>1,000 层）** | 作为对病态或生成文件的安全防护，文件被跳过。 |
| **解析错误 / 源码格式错误** | 文件被跳过；路径和错误信息记录在 `failed_files` 中（最多 5 条）。 |
| **扫描超时（默认 300 秒）** | 返回部分结果；`ScanStats` 中的 `timeout_triggered` 设为 `true`。 |
| **空项目 / 没有支持的文件** | 返回空的 `RepoGraph`；所有报告仍然有效但内容极少。 |
| **会话缓存未命中** | 自动执行完整扫描；结果会被缓存供后续调用使用。 |
| **符号未找到（query-symbol / call-chain）** | 返回空结果。这是正常行为，不是错误。 |
| **Git 操作不可用（impact / diff-risk）** | 在输出中明确报告"不是 git 仓库"；不会崩溃。 |
| **未安装 LSP 服务器** | `lsp doctor` 报告该服务器被跳过；其他命令中的 `--with-lsp` 优雅降级并给出明确提示。 |

在所有情况下，DeepMap 绝不会因为单个文件的处理失败而 panic 或崩溃。错误被收集、记录，并通过 `ScanStats` 元数据或报告输出呈现，由调用方决定如何处理。

## 架构

DeepMap 的架构遵循一个清晰的三阶段流水线，配合共享数据模型，每个阶段
都可以独立测试、优化和替换。

### 阶段一：文件遍历与过滤

入口点是 `engine.rs` 中的 `RepoMapEngine::scan()`。文件发现使用 `ignore`
crate（`ripgrep` 背后的库），在遍历项目树时遵循 `.gitignore` 规则，根
据 `SKIP_DIR_NAMES`（35 个常见噪音目录，如 `node_modules`、`.git`、
`target`、`venv`、`dist`、`build`、`vendor` 等）和 `SKIP_FILE_NAMES`
（锁文件等）过滤条目，只保留扩展名受支持的文件。

`types.rs::ext_to_lang()` 中的扩展名到语言的映射覆盖了 17 种扩展名、
9 个语言家族：Python（`.py`、`.pyi`）、JavaScript（`.js`、`.jsx`、`.mjs`、
`.cjs`）、TypeScript（`.ts`、`.tsx`、`.mts`、`.cts`）、Go（`.go`）、
Rust（`.rs`）、Java（`.java`）、Kotlin（`.kt`、`.kts`）、Swift（`.swift`）、
C/C++（`.cpp`、`.cc`、`.cxx`、`.hpp`、`.h`）、C#（`.cs`）、PHP（`.php`）、
Ruby（`.rb`）、HTML（`.html`、`.htm`）、CSS（`.css`）和 JSON（`.json`）。
大于 512 KB 的文件会被跳过（可通过 `DEEPMAP_MAX_FILE_BYTES` 环境变量配
置）。AST 嵌套深度超过 1000 的文件也会被跳过，作为对病态文件的安全防护。

遍历器尊重 `max_files` 限制和 `max_scan_secs` 超时（硬上限 300 秒），
因此即便是非常大的单体仓库也无法无限期地阻塞 AI。

### 阶段二：Tree-Sitter 解析与符号提取

每个候选文件使用相应的 tree-sitter 解析器进行解析。DeepMap 打包了 8 个
编译为原生 Rust 库的 tree-sitter 语法：

- `tree-sitter-rust` (0.24)
- `tree-sitter-python` (0.23)
- `tree-sitter-javascript` (0.23)
- `tree-sitter-typescript` (0.23，含 TypeScript 和 TSX)
- `tree-sitter-go` (0.23)
- `tree-sitter-html` (0.23)
- `tree-sitter-css` (0.23)
- `tree-sitter-json` (0.23)

`TreeSitterAdapter`（在 `parser.rs` 中）管理每种语言的解析器和编译好的
S-expression 查询。查询定义在 `queries.rs` 中，每种语言覆盖四种查询类型：

- **function**：函数声明、方法定义、箭头函数、lambda 表达式以及右侧为
  函数表达式的变量声明。
- **class**：类声明、结构体定义、枚举定义、trait 定义、接口声明和
  impl 块。
- **import**：导入语句、use 声明、require 调用和模块路径引用。Rust 导
  入提取器对 `scoped_use_list` 和嵌套的 `scoped_identifier` 路径有特殊
  处理，会拼接路径和名称段（例如 `std::collections::HashMap`）。
- **call**：调用表达式、方法调用、成员函数调用和作用域/限定调用。

每个查询产生包含名称、类型、字节范围和行范围的 `RawCapture` 条目。
对于 JavaScript 和 TypeScript，还额外运行三个提取步骤：

1. **导入绑定**：对每个 `import` 语句（ES 模块）和 `require()` 调用
   （CommonJS）的结构化记录，捕获本地名称、导入名称、源模块和导入类型
   （default、named、namespace、CJS default、CJS destructured）。这对
   调用链解析至关重要，因为它让引擎能够将代码中的 `foo()` 映射回 `foo`
   实际定义的位置，跨越重新导出链。

2. **导出绑定**：对每个 `export` 语句（ES 模块）和 `module.exports` /
   `exports.xxx` 赋值（CommonJS）的结构化记录，捕获导出名称、源名称、
   重新导出模块和导出类型。这使得引擎能够解析通过中间模块的
   `import { X } from 'Y'` 链。

3. **额外符号提取**：三次额外的提取
   （`extract_object_literal_methods`、`extract_anonymous_symbols`、
   `extract_exported_function_expressions`）捕获标准查询遗漏的符号：
   对象字面量中的方法定义、赋值给变量的匿名函数（例如 `const foo =
   function() {}`）以及默认导出的函数/类表达式。

提取的数据存储在 `RepoGraph` 中（定义在 `types.rs`）：

```
RepoGraph {
  symbols:          HashMap<symbol_id, Symbol>,
  outgoing:         HashMap<symbol_id, Vec<Edge>>,
  incoming:         HashMap<symbol_id, Vec<Edge>>,
  file_symbols:     HashMap<file_path, Vec<symbol_id>>,
  file_imports:     HashMap<file_path, Vec<import_string>>,
  file_calls:       HashMap<file_path, Vec<(call_name, line, kind)>>,
  file_import_bindings: HashMap<file_path, Vec<JsImportBinding>>,
  file_exports:     HashMap<file_path, Vec<JsExportBinding>>,
}
```

`Symbol` 包含其 id（由 `file:name:line` 组成的复合键）、名称、类型
（function、class、struct、method、arrow_function 等）、文件路径、行
范围、列、可见性、文档字符串、签名和 PageRank 分数。

### 阶段二 B：导入解析与边构建

解析完所有文件后，`ImportResolver`（在 `resolver.rs` 中）构建索引：

- **file_map**：文件名干 -> 候选文件路径（例如 `"helper"` ->
  `["src/utils/helper.ts", "tests/helper_test.ts"]`）。
- **name_index**：符号名 -> 符号 ID（例如 `"parse"` -> 项目中所有名为
  parse 的函数）。
- **known_paths**：所有已知文件路径的 `HashSet`，用于 O(1) 存在性检查。

解析器还会在项目根目录和一级子目录中发现 `tsconfig.json` 和
`jsconfig.json` 文件，解析它们（含 JSONC 注释和尾随逗号去除）以提取
`compilerOptions.baseUrl` 和 `compilerOptions.paths` 别名规则。

在构建边时，引擎处理两种关系：

1. **调用边**（权重：0.50）：对解析过程中发现的每个调用表达式，解析器
   尝试将其映射到目标符号。它首先查询本地名称映射表（从 JS/TS 导入绑定
   构建）以解析重命名和重新导出，然后在全局 `name_index` 中查找解析后
   的名称。调用符号通过找到包含调用行的符号来识别。

2. **导入边**（权重：0.35）：对每个导入语句，解析器尝试使用三步策略
   找到目标文件：首先尝试相对路径解析（针对以 `.` 开头的导入），然后
   尝试 tsconfig 别名模式匹配，最后回退到 `file_map` 中的文件名干查找。
   对每个解析到的目标文件，从源文件中的每个符号到目标文件中的每个符号
   都创建导入边。

去重集合 `HashSet<(source_id, target_id)>` 防止冗余边。常量控制边权重，
调用边权重高于导入边，因为调用代表更紧密的耦合。

### 阶段三：PageRank 计算与分析

图构建完成后，标准的幂迭代 PageRank 算法
（在 `ranking.rs::GraphAnalyzer::calculate_pagerank` 中）计算每个符号
的结构重要性：

1. 将所有节点的初始概率设为相等的 `1/N`。
2. 每轮迭代：将每个节点的当前概率按边权重比例分配给其出边邻居。悬空
   节点（没有出边的节点）贡献给均匀的跳转项。应用阻尼系数（默认 0.85，
   与经典 PageRank 论文一致）。
3. 当最大节点变化量低于 `1e-6` 时终止，或最多 50 轮迭代。
4. 归一化分数使其总和为 1.0。

阻尼系数和迭代次数在精度与计算时间之间取得平衡。项目规模的图通常在
20-30 轮迭代内收敛。

PageRank 计算完成后，`GraphAnalyzer` 提供以下分析查询：

- **query_symbol**：符号名的不区分大小写的子串搜索，过滤掉低信号类型
  （CSS 选择器、JSON 键、HTML 元素），按 PageRank 降序排列。
- **call_chain**：调用图的 BFS 遍历，深度受限（默认无限制，BFS 队列硬
  上限 10000 节点，结果上限 1000 个），支持调用者、被调用者或双向遍历。
- **hotspots**：按 `symbol_count * average_PageRank` 排序的文件，识别
  高密度、高重要性的文件，这些文件最可能是维护的关键点。
- **entry_points**：通过文件名干（`main`、`app`、`index`、`server`、
  `run`、`setup`、`cli`、`__main__`）或路径模式（`/src/main.tsx`、
  `/lib.rs`）启发式检测常见入口文件。
- **file_analysis**：每个文件的指标，包括符号数、出边数、入边数和平均
  PageRank。
- **module_summary**：按顶层目录分组的符号，按总 PageRank 降序排列，
  提供高层视图，展示哪些模块承载最多的结构权重。
- **suggested_reading_order**：按 `average_PR * ln(symbol_count) *
  entry_boost(2.0 如果入口文件)` 评分的文件，排除测试和噪音文件。这是
  AI 在接手项目时应该首先读取的列表。
- **summary_symbols**：对阅读顺序中前 N 个文件，取综合重要性排序的
  前 M 个符号（`入边数 * 3 + 出边数 * 2 + 类型权重`，其中函数和方法
  得 5 分，类和结构体得 4 分，模块得 3 分，变量得 2 分，其他得 1 分）。

### 报告渲染

所有分析结果由 `renderer.rs` 模块渲染成 Markdown。提供六种报告类型，
每种对应一个 TUI 工具或 CLI 命令：

1. **概览报告**（`render_overview_report`）：将扫描统计、入口点、推荐
   阅读顺序（前 20）、模块摘要（前 20）、热点（前 10）和关键符号（前
   30 个文件，每文件 5 个符号）组合成一份综合文档。按调用者指定的字符
   数限制截断，在单词边界处断开。

2. **调用链报告**（`render_call_chain_report`）：查询最佳匹配符号，然
   后按指定深度遍历调用者和被调用者。报告符号的类型、文件和 PageRank，
   随后是按 PageRank 排序的调用者和被调用者列表。

3. **文件详情报告**（`render_file_detail_report`）：以 Markdown 表格形
   式列出文件中的所有符号，包含行号、类型、名称、可见性、PageRank 分
   数和签名。按行号排序。

4. **查询报告**（`render_query_report`）：将主题评分、相关测试发现和关
   键符号高亮组合成一次响应。使用 `topic.rs` 模块中的 `topic_score` 函
   数，应用标识符拆分、文件角色分类、噪音惩罚、测试权重调整和 IDF 关
   键词加权。

5. **影响报告**（`render_impact_report`）：对每个变更文件，列出直接依
   赖方（导入该文件的文件）和每个文件的指标。汇总所有传递影响文件的
   总数。

6. **差异风险报告**（`render_diff_risk_report`）：结合影响分析与风险
   评估。`assess_risk` 启发式方法根据关键词模式对变更文件评分：
   `auth`/`login`/`token`（各 +3）、`db`/`sql`（各 +3）、`config`
   （+2）。分数映射到风险等级：0 = 低，1-2 = 中，3-5 = 高，6+ = 严重。
   报告还会建议验证命令和相关测试文件。

### 主题搜索引擎

`topic.rs` 模块实现了一个轻量级的代码搜索引擎，专为 AI 消费而设计。
它不使用嵌入或向量搜索，而是依赖一组经过调优的启发式策略：

- **标识符拆分**：将驼峰式、帕斯卡式、蛇形式和烤串式的标识符拆分为
   构成词素。例如 `getUserPermissions` 变成 `["get", "user",
   "permissions"]`。
- **文件角色分类**：基于路径模式将每个文件分类为 `test`、`frontend-ui`、
   `frontend-state`、`backend` 或 `config`，以便 AI 按角色过滤。
- **加权主题评分**：结合路径得分（30%）、符号名得分（25%）和符号类型/
   文档字符串得分（15%），加上噪音惩罚（生成/缓存文件减 5%）和测试权
   重调整（测试文件再减 45%）。
- **IDF 关键词加权**：出现在许多文件中的词权重低，稀有词权重高。计算
   方式为 `ln(N/df) + 0.5`，其中 N 是文件总数，df 是文档频率。
- **模糊符号建议**：莱文斯坦距离 <= 3 的容错符号查找，返回按距离再按
   PageRank 排序的候选结果。
- **相关测试发现**：依次应用三种策略：同目录测试文件（置信度 0.9）、
   命名约定匹配（置信度 0.75）和导入引用匹配（置信度 0.6）。

## 与 repomap 的关系

DeepMap 和 repomap 共享相同的设计理念，但服务于不同的目的，处于不同
的成熟度水平。

[repomap](https://github.com/gjczone/repomap) 是上游研究项目，使用
Python 编写，采用 MIT 许可证。它功能更完整，支持 15 种语言（DeepMap
目前是 8 种），提供 LSP 集成用于实时符号解析，实现了完整的"编辑 -> 验证"
工作流（可以自动检查 AI 的编辑是否达到了预期的目标），并为非常大的单体
仓库提供增量扫描支持。Repomap 暴露了 Python API 和 CLI，其设计优先考
虑可扩展性和实验性——新的语言支持、新的查询策略和新的分析步骤会首先
添加到 repomap。

DeepMap 是 repomap 的引擎，用 Rust 重写，原生集成到 DeepSeek-TUI 中。
它不是每个功能的移植，而是对 TUI 中 AI 工作流最重要的核心扫描、排序和
报告流水线的聚焦实现。通过作为编译库内嵌在 DeepSeek-TUI 二进制文件中，
DeepMap 消除了子进程调用、JSON 序列化和 Python 运行时依赖管理的开销。
代价是扩展 DeepMap 需要 Rust 编译步骤，比扩展 repomap 更复杂。

预期的工作流是双向的：

- 当 repomap 中验证了新的查询策略或语言语法后，它们会被移植到 DeepMap
  用于 DeepSeek-TUI 的生产环境。
- 当 DeepMap 的 Rust 引擎发现了更易于在 Python 中原型验证的边缘情况或
  性能瓶颈时，实验会在 repomap 中进行。

两个项目都处于活跃维护中，并同步演进。

## 语言支持

DeepMap 目前打包了 8 种语言的 tree-sitter 解析器：

| 语言 | 解析器 | 覆盖说明 |
|------|--------|----------|
| Rust | `tree-sitter-rust` | 函数、结构体、枚举、trait、impl 块、类型别名、模块、use 声明（作用域和通配符）、调用表达式 |
| Python | `tree-sitter-python` | 函数、带装饰器函数、类、带装饰器类、方法、lambda、导入（绝对、相对、别名）、调用表达式 |
| JavaScript | `tree-sitter-javascript` | 函数、箭头函数、类、方法、ES 模块导入/导出、CommonJS require、调用表达式、对象字面量方法、匿名函数 |
| TypeScript | `tree-sitter-typescript`（TypeScript + TSX） | 与 JavaScript 相同，外加 TSX 对 React/JSX 语法的支持 |
| Go | `tree-sitter-go` | 函数、方法、结构体、接口、导入（解释型字符串字面量）、调用表达式 |
| HTML | `tree-sitter-html` | 标签名作为符号 |
| CSS | `tree-sitter-css` | 类选择器、ID 选择器、标签名作为符号 |
| JSON | `tree-sitter-json` | 键值对作为符号 |

`types.rs` 中的 `ext_to_lang` 函数将 17 种文件扩展名映射到这 8 个解析
器加上 7 种附加语言（Java、Kotlin、Swift、C/C++、C#、PHP、Ruby），这
些附加语言的文件会被发现并计入文件计数，但尚未有 tree-sitter 查询——
它们会通过过滤但不产生符号或边。这些是未来解析器集成的候选语言。

### 关于 tree-sitter-tsx

TypeScript 解析器支持包含独立的 TSX 语法
（`tree_sitter_typescript::LANGUAGE_TSX`），除了标准的 TypeScript 语法
（`tree_sitter_typescript::LANGUAGE_TYPESCRIPT`）之外。这对 React/JSX
项目很重要，因为 TSX 语法能理解纯 TypeScript 语法无法理解的 JSX 语法节
点。没有 TSX 支持，`component.tsx` 文件会在任何 JSX 表达式上解析失败，
导致整个文件不产生任何符号。

## 设计决策

### 为什么选择 PageRank 而不是其他算法？

选择 PageRank 有三个原因。首先，它是一种经过充分研究的算法，具有已知
的收敛性质——50 轮迭代加上 0.85 的阻尼系数可以保证任何有向图的稳定排
序。其次，它自然地契合"重要性通过边流动"这一直觉，很好地映射到软件依
赖关系：如果一个文件被许多重要文件导入，那么它本身也很重要。第三，对
项目规模的图（通常 5000-50000 个节点），计算成本很低，图构建完成后只
需要几毫秒。

考虑的替代方案包括中心性度量（介数中心性、紧密中心性），它们的计算成
本更高；以及机器学习方法（节点嵌入、图神经网络），它们需要训练数据，
对于一个必须在任何代码库上无需预训练即可工作的通用工具来说不切实际。

### 为什么是三阶段而不是流式？

三阶段设计（遍历 -> 解析 -> 排序）要求在排序开始之前将所有解析数据保
存在内存中。增量排序的流式方法会使用更少的内存，但会牺牲 PageRank 所
需的全局视角——一个符号的重要性取决于整个图的结构，而不仅仅是局部属性。

对于一个典型项目，`RepoGraph` 使用 50-200 MB 内存，这对于一个临时运行
的开发工具来说是可以接受的。会话缓存确保每个工作空间在进程生命周期内
只需支付一次这个成本。

### 为什么不用系统自带的 LSP？

基于 LSP 的方法可以提供更丰富的符号信息（类型注解、文档、诊断），并且
不需要打包 tree-sitter 语法。然而，LSP 服务器是语言特定的，需要安装和
配置，并且在多语言项目中可能不是每种语言都可以使用。DeepMap 的
tree-sitter 方法离线工作，零配置，对每种打包的语言提供统一的支持。代价
是语义理解较浅——DeepMap 知道什么是定义的以及什么调用了什么，但它不知
道类型。

## 许可证

MIT —— 与 repomap 和 DeepSeek-TUI 相同。repomap 和 deepmap 都采用宽松
许可证，可免费用于任何项目，包括商业项目。欢迎通过标准的 GitHub fork
和 PR 工作流贡献代码。

## 未来方向

以下功能正在考虑在未来的版本中实现：

- **增量扫描**：只重新扫描变更的文件，而不是整个项目，使用 mtime 缓存
  和 SHA-256 指纹作为变更检测器。mtime 缓存基础设施已经在引擎中就位——
  需要的是"差异与合并"步骤，更新现有图而非从头重建。

- **LSP 集成插件**：一个可选的插件，在可用时连接一个或多个 LSP 服务器
  以获取更丰富的符号信息（类型注解、悬停文档、诊断），在不可用时回退
  到 tree-sitter。

- **更多语言解析器**：Java、Kotlin、Swift、C/C++、C#、PHP 和 Ruby 解析
  器将把语言数量提升到 15 种，与 repomap 的覆盖范围持平。

- **编辑后验证**：给定一组编辑过的文件和原始的依赖图，自动检查所有受
  影响文件是否仍然满足其结构不变量（导出匹配导入，调用目标仍然存在等）。

- **时间序列排名**：追踪跨 git 历史的 PageRank 分数，以识别结构重要性
  正在增长的模块，在它们成为维护瓶颈之前将其标记为重构候选。
