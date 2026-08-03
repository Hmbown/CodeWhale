//! TOML execpolicy rules consumed by the shell tool.
//!
//! The legacy Starlark policy engine (`PolicyParser`, `Policy`, `Rule`, the
//! `execpolicy check` CLI verb) was deleted for v0.9.4: the runtime never
//! enforced it, so a green `check` meant nothing. The live policy surface is
//! the TOML `execpolicy.toml` rules here plus the `codewhale-execpolicy`
//! crate's permission engine.

pub mod matcher;
pub mod rules;

pub use rules::{ExecPolicyDecision, load_default_policy};
