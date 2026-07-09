//! Minimal PTY/frame-capture harness for TUI integration tests.
//!
//! Spawns the `codewhale-tui` binary in a real pseudo-terminal, sends scripted
//! keystrokes / paste, and parses the ANSI output stream into terminal
//! frames so tests can assert on visible text and on the filesystem.
//!
//! Tests opt in via:
//! ```ignore
//! #[path = "support/qa_harness/mod.rs"]
//! mod qa_harness;
//! use qa_harness::harness::Harness;
//! use qa_harness::keys;
//! ```
//!
//! Design notes live in `README.md` next to this module.

#![allow(dead_code)]

pub mod frame;
pub mod harness;
pub mod keys;
pub mod pty;

pub use frame::Frame;
pub use keys::paste;
pub use pty::PtySession;
