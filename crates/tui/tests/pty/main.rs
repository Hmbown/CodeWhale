//! Consolidated harness for real-PTY scenarios.
//!
//! Every test here boots the real `codewhale-tui` binary inside a
//! `portable-pty` session and drives it through `support/qa_harness`. They are
//! `#[cfg(unix)]` and contend for the terminal, so they serialize on a
//! per-harness mutex already. One binary links `rio-vt` + `portable-pty` once
//! instead of four times. See `crates/tui/tests/README.md`.

// The qa_harness module is declared exactly once at the crate root; the
// scenario modules below reference it via `use qa_harness::...`. Declaring
// it per-scenario would load the same files as separate modules in one
// binary (clippy::duplicate_mod).
#[cfg(unix)]
#[path = "../support/qa_harness/mod.rs"]
mod qa_harness;

mod agent_focus_pty;
mod fleet_setup_pty;
mod qa_pty;
mod release_runtime_qa;
mod terminal_matrix_qa;
mod work_bar_subagents_pty;
