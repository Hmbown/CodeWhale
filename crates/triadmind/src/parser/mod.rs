//! # Parser — Multi-language topology extraction
//!
//! Ported from triadmind-core:
//! - `parser.ts` — parser coordination
//! - `treeSitterParser.ts` — tree-sitter multi-language parser
//! - `typescriptParser.ts` — TypeScript adapter
//! - `capability-topology-spec.md` — capability aggregation rules
//!
//! Parses source files into `TriadNodeDefinition` nodes, supporting:
//! - `leaf` mode: every function/method becomes a node
//! - `capability` mode: aggregates leaves into capability nodes (default)
//! - `module` / `domain` modes: higher-level projections
//!
//! @LeftBranch: scan_project, build_triad_map
//! @RightBranch: ParserOptions, ScanMode, LeafNode

mod parser_core;
mod tree_sitter_engine;

pub use parser_core::*;
pub use tree_sitter_engine::*;
