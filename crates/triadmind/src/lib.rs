//! # TriadMind — Native Architecture Governance for DeepSeek TUI
//!
//! This crate is a native Rust port of the triadmind-core TypeScript CLI.
//! It embeds architecture governance directly into the DeepSeek TUI agent runtime,
//! replacing the previous MCP bridge with zero-overhead native integration.
//!
//! ## Vertex (顶点)
//!
//! **Name**: `TriadMind.govern`
//!
//! **Responsibility**: 为 DeepSeek TUI Agent 提供原生架构治理能力——拓扑抽取、协议校验、
//! 规则注入、自愈分析、梦境守护。
//!
//! **Invariant**: 任何架构治理操作都必须先读取 triad-map.json 进行拓扑验证，
//! 优先复用现有节点，只在不破坏稳定拓扑的前提下进行 modify 或 create_child。
//!
//! ## Macro-Split
//!
//! - **Left Branch** (动态执行):
//!   - `sync` — 源码扫描 → 拓扑重建
//!   - `verify` — 拓扑一致性校验
//!   - `rules` — 架构规则注入
//!   - `dream` — 后台架构分析
//!   - `heal` — 运行时错误诊断
//!   - `navigate` — 事前架构推演
//!
//! - **Right Branch** (静态约束):
//!   - `protocol` — 升级协议数据类型与校验
//!   - `config` — TriadMind 配置模型
//!   - `triad_map` — topology graph 加载/序列化
//!
//! ## Meso-Split
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | `protocol` | UpgradeProtocol 类型定义、拓扑校验、节点引用的解析 |
//! | `rules` | AGENTS.md / Cursor 规则注入与卸载 |
//! | `sync` | 文件变更检测、拓扑重建触发 |
//! | `verify` | 拓扑质量指标计算、循环检测、抽象赤字分析 |
//! | `dream` | 后台架构腐化检测、重构提案生成 |
//! | `heal` | 运行时错误 → 拓扑节点匹配 → 修复协议 |
//! | `navigate` | 前瞻性架构冲击地图生成 |
//! | `visualizer` | 交互式 HTML 知识图谱渲染 |

pub mod protocol;
pub mod rules;
pub mod config;
pub mod sync;
pub mod verify;

// ── Phase 1: New module skeletons ──────────────────────────────────

/// Multi-language source parser (tree-sitter based).
/// Extracts topology leaf/capability nodes from source files.
pub mod parser;
pub mod navigate;
pub mod heal;

/// Background architecture health analysis ("Dream Engine").
/// Detects abstraction deficit, ghost nodes, and generates refactoring proposals.
pub mod dream;

/// CI/CD governance gate checks.
/// Enforces coverage, ghost ratio, execute-like ratio thresholds.
pub mod govern;
pub mod generator;
pub mod visualizer;
pub mod workflow;
pub mod runtime;
pub mod abstraction_memory;
