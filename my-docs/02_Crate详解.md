# 02 — Crate 详解

> 逐个 crate 讲清楚：它解决什么问题、用了什么 Rust 技巧、关键类型在哪、从哪个文件开始读。

---

## 1. `codewhale-core` — 核心引擎

**路径:** `crates/core/`  
**就是它干的:** 把所有东西串起来的"大脑"。`Runtime` 是整个程序的中心。

### 用了什么 Rust 技巧

**`Arc<T>` — 共享所有权**
```rust
// 多个模块需要同一份 ToolRegistry，但不希望各自 clone 一份
// Arc = Atomic Reference Counted，引用计数式的共享指针
// 类比：JS 中多个变量指向同一个 Object，但 Rust 需要显式管理
use std::sync::Arc;
let tools = Arc::new(ToolRegistry::new());
let tools_for_agent = Arc::clone(&tools);  // 增加引用计数，不拷贝数据
```

**`async fn` — 异步函数**
```rust
// 几乎所有核心方法都是 async 的
// 因为调用 LLM API (HTTP)、读写文件、执行命令都有 I/O 等待
// 类比：JS 的 async/await，但 Rust 需要 tokio 作为"事件循环"
pub async fn handle_prompt(&self, prompt: &str) -> Result<Turn> {
    // 这里会 await LLM 的 HTTP 响应，期间线程可以处理其他事
}
```

**Builder 模式**
```rust
// Runtime 构造时需要很多配置，用 Builder 逐步组装
// 而不是一个几十个参数的函数
let runtime = Runtime::builder()
    .config(config)
    .tools(tools)
    .hooks(hooks)
    .build()?;
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `Runtime` | `crates/core/src/runtime.rs` | 核心入口。持有所有子系统的 Arc |
| `Turn` | 同上 | 一轮对话的结果 |
| `Session` | `crates/core/src/session.rs` | 一次会话的完整状态 |
| `ToolRegistry` | `crates/core/src/tools.rs` | 工具注册表。通过它找到并调用工具 |

### 依赖的关键外部库

- **tokio** — 异步运行时。所有 async 代码都跑在它上面
- **serde / serde_json** — JSON 序列化。和 LLM API 通信全靠 JSON
- **tracing** — 结构化日志

### 推荐阅读顺序

1. `crates/core/src/lib.rs` — 看 pub export 了哪些东西
2. `crates/core/src/runtime.rs` — Runtime 结构体定义，看字段就知道它管什么
3. `crates/core/src/session.rs` — 会话是怎么存的

---

## 2. `codewhale-agent` — Agent 定义

**路径:** `crates/agent/`  
**就是它干的:** 定义"Agent 是什么"以及它的生命周期。

### 用了什么 Rust 技巧

**`trait` 定义接口**
```rust
// Agent 是一个 trait，不同的实现可以有不同行为
// 类比：Go 的 interface, Java 的 interface
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, ctx: AgentContext) -> Result<AgentOutput>;
}
// Send + Sync 表示这个 trait 对象可以安全地跨线程传递
```

**`enum` 承载状态机**
```rust
// Agent 的状态用 enum 建模，每种状态可以携带不同数据
pub enum AgentState {
    Idle,
    Working { turn_count: u32 },
    WaitingApproval { tool_name: String },
    Completed(AgentOutput),
    Failed(String),
}
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `Agent` trait | `crates/agent/src/lib.rs` | Agent 的抽象接口 |
| `AgentContext` | 同上 | Agent 执行时需要的上下文 |
| `SubAgentConfig` | 同上 | 子 Agent 的配置（并发数、模型选择等） |

### 依赖的关键外部库

- **async-trait** — Rust 原生不支持 async trait 方法，这个宏补上了这个能力
- **tokio** — 异步执行

---

## 3. `codewhale-cli` — 命令行入口

**路径:** `crates/cli/`  
**就是它干的:** 解析命令行参数，然后交给 core 去干正事。

### 用了什么 Rust 技巧

**`clap` 声明式参数解析**
```rust
// clap 是 Rust 最主流的 CLI 框架
// 用 #[derive(Parser)] 自动从 struct 生成参数解析代码
use clap::Parser;

#[derive(Parser)]
#[command(name = "codewhale")]
struct Cli {
    /// 设置 API provider
    #[arg(long)]
    provider: Option<String>,
    
    /// 无头模式：直接执行一条指令
    #[arg(long)]
    exec: Option<String>,
}
// clap 会在编译时生成代码，自动处理 --help、参数校验等
```

**`anyhow` — 简化错误处理**
```rust
// 标准库的 Result<T, E> 需要明确 E 是什么类型
// anyhow::Result<T> 是 Result<T, anyhow::Error> 的别名
// anyhow::Error 可以装任何错误，适合上层（CLI、main）使用
use anyhow::Result;
fn main() -> Result<()> {  // 不用标注具体错误类型
    do_stuff()?;  // ? 自动把错误转成 anyhow::Error
    Ok(())
}
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `Cli` | `crates/cli/src/main.rs` | 命令行参数定义 |
| `Commands` enum | 同上或 `commands.rs` | 子命令枚举：`auth`, `exec`, `server`, `doctor` 等 |

### 依赖的关键外部库

- **clap** — CLI 参数解析
- **anyhow** — 简化错误处理
- **tokio** — main 函数是 `#[tokio::main]`，启动异步运行时

### 推荐阅读

`crates/cli/src/main.rs` 就是最好的起点。从 `main` 函数开始追踪。

---

## 4. `codewhale-tui` — 终端界面

**路径:** `crates/tui/`  
**就是它干的:** 画那个漂亮的终端交互界面。

### 用了什么 Rust 技巧

**`ratatui` — 终端 UI 框架**
```rust
// ratatui 是 Rust 生态的终端 UI 库
// 它用"即时模式"渲染：每帧重新画整个界面
// 而不是"保留模式"（像 HTML DOM 那样只改变化的部分）
// 优点：状态管理简单，渲染逻辑直观
```

**事件循环**
```rust
// TUI 的核心是一个死循环：读事件 → 更新状态 → 重绘
loop {
    // 1. 读键盘事件（异步，不阻塞）
    let event = event_stream.next().await;
    // 2. 更新应用状态
    app.handle_event(event);
    // 3. 画界面
    terminal.draw(|frame| {
        app.render(frame);
    })?;
}
```

**组件化布局**
```rust
// ratatui 用 Layout 把终端分成多个区域
let layout = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Min(3),     // 对话区
        Constraint::Length(3),  // 输入区
        Constraint::Length(1),  // 状态栏
    ])
    .split(frame.area());
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `App` | `crates/tui/src/app.rs` | TUI 应用的状态 |
| `Event` | `crates/tui/src/event.rs` | 键盘/鼠标/系统事件 |
| `UI 组件` | `crates/tui/src/ui/` | 各个渲染组件 |

### 依赖的关键外部库

- **ratatui** — 终端 UI 框架
- **crossterm** — 跨平台终端控制（光标、颜色、事件）
- **tokio** — 事件流的异步读取

### 推荐阅读

`crates/tui/src/app.rs` — 看 `App` 怎么管理状态。

---

## 5. `codewhale-tools` — 工具系统

**路径:** `crates/tools/`  
**就是它干的:** 定义 Agent 能用的所有工具（读文件、跑命令、Git 操作等）。

### 用了什么 Rust 技巧

**`trait` 多态 — 工具接口**
```rust
// 所有工具实现同一个 trait
// dyn Tool = 运行时多态，可以存不同类型的工具在同一个 Vec 里
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON Schema
    async fn execute(&self, args: Value) -> Result<ToolOutput>;
}

// 使用时：
let tools: Vec<Box<dyn Tool>> = vec![
    Box::new(ReadFileTool::new()),
    Box::new(ExecShellTool::new()),
    Box::new(GitTool::new()),
];
```

**JSON Schema 描述工具参数**
```rust
// LLM 需要知道工具的"签名"才能正确调用
// CodeWhale 用 JSON Schema 描述每个工具的参数
// 这个 schema 会被填入 LLM 请求的 tools 字段
fn parameters(&self) -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "文件路径"},
            "start_line": {"type": "integer", "description": "起始行"}
        },
        "required": ["path"]
    })
}
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `Tool` trait | `crates/tools/src/lib.rs` | 工具接口 |
| `ToolOutput` | 同上 | 工具执行的返回结果 |
| `ReadFileTool` | `crates/tools/src/file.rs` 或类似文件 | 读文件工具 |
| `ExecShellTool` | `crates/tools/src/shell.rs` | 执行 Shell 命令 |

### 依赖的关键外部库

- **serde_json** — JSON 参数解析
- **tokio** — 异步执行命令（`tokio::process::Command`）

### 推荐阅读

`crates/tools/src/lib.rs` — 看 `Tool` trait 长什么样。

---

## 6. `codewhale-mcp` — MCP 协议

**路径:** `crates/mcp/`  
**就是它干的:** 实现 Model Context Protocol（MCP）。CodeWhale 既可以作为 MCP 客户端连别人的工具服务器，也可以自己作为 MCP 服务器暴露出去。

### 用了什么 Rust 技巧

**双向通信 — `tokio::sync::mpsc` 通道**
```rust
// mpsc = Multiple Producer, Single Consumer
// 多生产者、单消费者通道。用于异步任务间通信
// 类比：Go 的 channel
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel(32);  // 缓冲区大小 32

// 生产者端
tx.send("hello").await?;

// 消费者端
while let Some(msg) = rx.recv().await {
    println!("got: {}", msg);
}
```

**客户端-服务器模式**
```rust
// MCP 客户端：发现并调用外部服务器提供的工具
let client = McpClient::connect("http://localhost:3000").await?;
let tools = client.list_tools().await?;
let result = client.call_tool("search", args).await?;

// MCP 服务器：把 CodeWhale 的工具暴露出去
let server = McpServer::new(tool_registry);
server.serve("0.0.0.0:3000").await?;
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `McpClient` | `crates/mcp/src/client.rs` | MCP 客户端 |
| `McpServer` | `crates/mcp/src/server.rs` | MCP 服务器 |
| `ToolDefinition` | 同上 | MCP 工具的定义格式 |

### 依赖的关键外部库

- **tokio** — 异步网络通信
- **serde_json** — JSON-RPC 消息格式
- **reqwest** — HTTP 客户端（连外部 MCP 服务器）

---

## 7. `codewhale-config` — 配置管理

**路径:** `crates/config/`  
**就是它干的:** 解析 `~/.codewhale/config.toml` 配置文件。

### 用了什么 Rust 技巧

**`serde` 反序列化**
```rust
// 定义 Rust struct，用 #[derive(Deserialize)] 自动支持 TOML 解析
use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    provider: Option<String>,
    model: Option<String>,
    
    #[serde(default)]
    subagents: SubAgentsConfig,  // 如果没写就用 Default
}

// 然后用 toml crate 把文件内容变成 Rust 结构体
let config: Config = toml::from_str(&file_content)?;
```

**默认值处理**
```rust
// serde(default) 会在字段缺失时用类型的 Default 实现
// serde(default = "function_name") 调用自定义函数生成默认值
#[derive(Deserialize)]
struct SubAgentsConfig {
    #[serde(default = "default_max_concurrency")]
    max_concurrency: usize,
}

fn default_max_concurrency() -> usize { 5 }
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `Config` | `crates/config/src/lib.rs` | 顶层配置结构体 |
| `ProviderConfig` | 同上 | 单个 Provider 的配置 |

### 依赖的关键外部库

- **toml** — TOML 格式解析
- **serde** — 序列化/反序列化框架
- **dirs** — 获取系统标准目录（如 `~/.codewhale/`）

---

## 8. `codewhale-state` — 会话持久化

**路径:** `crates/state/`  
**就是它干的:** 用 SQLite 把对话历史、任务状态存到磁盘上。

### 用了什么 Rust 技巧

**`rusqlite` — SQLite 绑定**
```rust
// rusqlite 是 Rust 对 SQLite C 库的安全封装
// 支持参数化查询（防止 SQL 注入）
use rusqlite::Connection;

let conn = Connection::open("sessions.db")?;

// 创建表
conn.execute(
    "CREATE TABLE IF NOT EXISTS sessions (
        id TEXT PRIMARY KEY,
        data TEXT NOT NULL,
        created_at TEXT NOT NULL
    )",
    [],  // 空参数列表
)?;

// 参数化插入
conn.execute(
    "INSERT INTO sessions (id, data, created_at) VALUES (?1, ?2, ?3)",
    rusqlite::params![session_id, json_data, timestamp],
)?;
```

**JSON 序列化存储**
```rust
// 复杂的状态对象 → JSON 字符串 → 存进 SQLite TEXT 字段
// 读出来 → JSON 字符串 → 反序列化回 Rust 对象
let json = serde_json::to_string(&session)?;
conn.execute("INSERT INTO sessions ...", params![json])?;
// 这就是为什么依赖里同时有 rusqlite 和 serde_json
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `StateStore` | `crates/state/src/lib.rs` | 状态存储的抽象 |
| `Session` | 同上 | 会话的持久化表示 |

### 依赖的关键外部库

- **rusqlite** — SQLite 绑定（`bundled` feature 表示编译时自带 SQLite C 源码，不需要系统安装）
- **serde_json** — 对象到 JSON 的转换

---

## 9. `codewhale-hooks` — 钩子系统

**路径:** `crates/hooks/`  
**就是它干的:** 在工具执行前后插入自定义逻辑。比如：工具被调用前先问你、某个命令永远禁止、某些操作自动放行。

### 用了什么 Rust 技巧

**策略模式 — 钩子链**
```rust
// 多个钩子组成一条链，依次执行
// 任何一个返回 Deny 就立即中断
enum HookDecision {
    Allow,
    Deny(String),      // 带拒绝原因
    Ask(String),       // 带提示信息，等待用户决定
}

// 钩子链：遍历执行，Deny 优先
async fn run_hooks(hooks: &[Box<dyn Hook>], call: &ToolCall) -> HookDecision {
    for hook in hooks {
        match hook.before_call(call).await {
            HookDecision::Deny(reason) => return HookDecision::Deny(reason),
            HookDecision::Ask(msg) => return HookDecision::Ask(msg),
            HookDecision::Allow => continue,  // 继续下一个钩子
        }
    }
    HookDecision::Allow  // 全部通过
}
```

**TOML 配置驱动**
```rust
// 钩子规则写在 .codewhale/hooks.toml 里
// 不需要写 Rust 代码就能定制安全策略
// 示例：
// [[hooks]]
// type = "deny"
// pattern = "rm -rf /*"
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `Hook` trait | `crates/hooks/src/lib.rs` | 钩子接口 |
| `HookDecision` enum | 同上 | 允许 / 拒绝 / 询问 |

---

## 10. `codewhale-execpolicy` — 执行策略

**路径:** `crates/execpolicy/`  
**就是它干的:** 更细粒度的命令执行控制。什么样的 shell 命令可以跑、在什么条件下可以跑。

### 用了什么 Rust 技巧

**规则引擎**
```rust
// 不是简单的黑白名单，而是条件匹配
// 可以有：允许读任意文件但只允许写 /tmp
//         允许 cargo test 但禁止 cargo publish
struct ExecPolicy {
    rules: Vec<Rule>,
}

enum Rule {
    Allow { command_pattern: String, in_path: Option<String> },
    Deny { command_pattern: String, reason: String },
}
```

---

## 11. `codewhale-secrets` — 密钥管理

**路径:** `crates/secrets/`  
**就是它干的:** 安全存储 API Key。从配置文件读出来，或从环境变量读。

### 用了什么 Rust 技巧

**多层密钥来源**
```rust
// API Key 来源有优先级：
// 1. 环境变量 (DEEPSEEK_API_KEY)
// 2. 配置文件 (~/.codewhale/config.toml)
// 3. 交互式输入（auth set 命令）
fn resolve_api_key(provider: &str) -> Option<String> {
    // 先查环境变量
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        return Some(key);
    }
    // 再查配置文件
    // ...
}
```

---

## 12. `codewhale-protocol` — 通信协议

**路径:** `crates/protocol/`  
**就是它干的:** 定义消息的 Rust 类型。各个 crate 之间通信用的数据结构都在这里。

### 用了什么 Rust 技巧

**纯数据类型 crate**
```rust
// 这个 crate 没有业务逻辑，只有类型定义
// 好处：其他 crate 可以依赖它而不引入重依赖
// 这是一种常见的 Rust 项目组织模式

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
}
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| `Message` | `crates/protocol/src/lib.rs` | 一条对话消息 |
| `ToolCall` | 同上 | 工具调用请求 |
| `ToolResult` | 同上 | 工具执行结果 |

---

## 13. `codewhale-app-server` — HTTP 服务器

**路径:** `crates/app-server/`  
**就是它干的:** 提供 HTTP/SSE 接口，让外部程序可以调用 CodeWhale。

### 用了什么 Rust 技巧

**`axum` — Web 框架**
```rust
// axum 是基于 tokio 和 tower 的 Web 框架
// 用宏定义路由，类型安全
use axum::{Router, routing::post};

let app = Router::new()
    .route("/api/chat", post(handle_chat))
    .route("/api/health", get(handle_health));

// 启动服务器
let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
axum::serve(listener, app).await?;
```

**SSE (Server-Sent Events)**
```rust
// SSE 用于流式输出：模型一边生成一边推送给客户端
// 类比：HTTP 长连接 + 逐条推送
async fn handle_chat() -> Sse<impl Stream<Item = Result<Event, ...>>> {
    // 返回一个 Stream，每次模型吐出 token 就推送一条事件
}
```

### 关键类型

| 类型 | 在哪 | 做什么 |
|------|------|--------|
| Router | `crates/app-server/src/lib.rs` | HTTP 路由定义 |

### 依赖的关键外部库

- **axum** — Web 框架
- **tokio** — 异步 HTTP 服务器
- **tower-http** — HTTP 中间件（CORS 等）

---

## 14. `codewhale-whaleflow` — 工作流引擎

**路径:** `crates/whaleflow/`  
**就是它干的:** 复杂任务编排。把大任务拆成子任务，按分支/叶子的拓扑结构调度执行。

### 用了什么 Rust 技巧

**DAG（有向无环图）工作流**
```rust
// 工作流是一个 DAG：节点是任务，边是依赖关系
// 只有所有依赖完成，当前节点才能执行
struct Workflow {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

// 调度器找出所有依赖已满足的节点并行执行
// 这需要 tokio 的并发原语（JoinSet、Semaphore 等）
```

---

## 你读代码时最常用的 Rust 模式速查

### `?` 操作符

```rust
// ? 是 "如果出错就提前返回" 的简写
// 实际上做两件事：1) 如果是 Err，把错误转成当前函数的错误类型并 return
//                 2) 如果是 Ok，把 Ok 里的值取出来
fn read_config() -> Result<Config, Error> {
    let content = std::fs::read_to_string("config.toml")?;  // 如果读文件失败就 return Err
    let config: Config = toml::from_str(&content)?;         // 如果解析失败就 return Err
    Ok(config)
}
```

### `#[tokio::main]` 宏

```rust
// 这个宏把普通 main 函数变成异步的
// 展开后相当于：创建 tokio 运行时 → 运行 async main → 等待完成
#[tokio::main]
async fn main() {
    // 这里可以用 .await
    do_stuff().await;
}

// 等价于手写：
// fn main() {
//     tokio::runtime::Runtime::new().unwrap().block_on(async {
//         do_stuff().await;
//     });
// }
```

### `#[derive(...)]` 宏

```rust
// Rust 编译器可以自动生成一些常见 trait 的实现
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MyStruct {
    field: String,
}
// Debug   → 可以用 {:?} 打印
// Clone   → 可以 .clone()
// Serialize   → 可以序列化成 JSON/TOML
// Deserialize → 可以从 JSON/TOML 反序列化
```

---

## 各 Crate 依赖关系图

```
cli ──────┐
tui ──────┼──→ core ──→ agent
app-server┘       │
                  ├──→ tools
                  ├──→ hooks
                  ├──→ execpolicy
                  ├──→ mcp
                  ├──→ config
                  ├──→ state
                  ├──→ secrets
                  ├──→ protocol
                  └──→ whaleflow
```

`core` 是枢纽，所有其他 crate 要么调用它（上层），要么被它调用（下层）。`protocol` 是纯类型 crate，被几乎所有 crate 依赖。

---

## 下一步

理解了各 crate 的职责和它们用的 Rust 技巧后，看 [03_快速上手.md](03_快速上手.md) 来实际跑起来。