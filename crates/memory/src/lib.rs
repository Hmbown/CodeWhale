//! # Codewhale Hippocampal Memory System
//!
//! A structured, SQLite-backed memory store that enables the agent to remember
//! facts, entities, and relationships across sessions — the foundation for
//! true infinite-context and cross-session recall.
//!
//! ## Core Concepts
//!
//! - **Entities**: Files, PRs, issues, concepts, people, decisions — anything
//!   the model might need to reference later.
//! - **Relations**: Directed edges connecting entities (e.g. `dispatch.rs` is
//!   `part_of` `PR #2933`).
//! - **Facts**: Standalone factual statements, optionally bound to an entity.
//!   Stored with an importance score (0.0–1.0) for active forgetting.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use codewhale_memory::MemoryStore;
//!
//! let store = MemoryStore::open(&path)?;
//! store.insert_fact(None, "user prefers 4-space indentation", "user", 0.9, None)?;
//! let facts = store.search_facts("indentation", 10)?;
//! ```

pub mod schema;
pub mod store;

pub use store::{Entity, Fact, MemoryStore, Relation};
