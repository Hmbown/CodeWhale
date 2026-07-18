# CodeWhale 源码阅读指南

> 写给会 Rust 基本语法，但不太熟悉标准库 / 常用库 / 设计模式的朋友。

## 怎么用这套文档

一共 3 份文档，建议按顺序看：

| 顺序 | 文件 | 内容 |
|------|------|------|
| 1 | [01_架构概览.md](01_架构概览.md) | 整体架构：有哪些 crate，它们怎么连起来，一次对话的数据流是怎样的 |
| 2 | [02_Crate详解.md](02_Crate详解.md) | 每个 crate 逐一讲解：它解决什么问题、用了什么 Rust 技巧、关键类型在哪 |
| 3 | [03_快速上手.md](03_快速上手.md) | 编译好的 `codewhale.exe` 怎么用：配置 API Key、快捷键、常用命令 |

## 一些有助于开图的外部链接

- 项目主页：<https://codewhale.net/>
- GitHub 仓库：<https://github.com/Hmbown/CodeWhale>
- DeepWiki 自动生成的项目索引：<https://deepwiki.com/Hmbown/CodeWhale>
- crates.io 发布页：<https://crates.io/crates/codewhale-cli>
- npm 发布页：<https://www.npmjs.com/package/codewhale>
- Release 下载：<https://github.com/Hmbown/CodeWhale/releases>
- CNB 镜像（国内更快）：<https://cnb.cool/codewhale.net/codewhale>

## 需要用到的 Rust 知识速查

以下是你读代码时大概率会遇到的、超出"基础语法"的东西，我把它们和文档里出现的位置列在一起：

| 概念 | 一句话解释 | 在哪里出现 |
|------|-----------|-----------|
| `async` / `await` | 让函数可以"暂停等待 I/O"，不阻塞线程 | 几乎所有 crate |
| `tokio` | Rust 最主流的异步运行时，相当于 JS 的事件循环 | `core`, `agent`, `cli`, `app-server` |
| `Arc<T>` | 多个所有者共享同一份数据，引用计数 | `core`（Runtime、ToolRegistry） |
| `trait` + `dyn` | 定义接口，运行时动态分发（类似 interface） | `tools`, `hooks`, `protocol` |
| `serde` | 序列化/反序列化框架，`#[derive(Serialize, Deserialize)]` | 全项目 |
| `clap` | CLI 参数解析框架，`#[derive(Parser)]` | `cli` |
| `ratatui` | 终端 UI 框架，画 TUI 界面 | `tui` |
| `axum` | HTTP 服务器框架，基于 tokio | `app-server` |
| `rusqlite` | SQLite 绑定，用于会话持久化 | `state` |
| `tracing` | 结构化日志框架（比 `println!` 更专业） | 全局 |

## 许可证

CodeWhale 本身是 MIT 协议。这套文档也是。