//! Consolidated harness for Gherkin/Cucumber runners.
//!
//! Cucumber (`cucumber = "0.23"`) registers steps per `World` type via
//! inventory. Each `World` is scoped to its own `mod`, so merging the six
//! feature runners into one binary keeps `cargo test -p codewhale-tui
//! tool_lifecycle` etc. filtering via the module path while cutting six link
//! jobs to one. See `crates/tui/tests/README.md`.

#[cfg(all(unix, feature = "long-running-tests"))]
#[path = "../support/qa_harness/mod.rs"]
mod qa_harness;

#[cfg(all(unix, feature = "long-running-tests"))]
mod active_composer_pointer_pty;
mod core_session_command_extraction;
mod directory_listing_acceptance;
mod epic_acceptance_harness;
mod eval_smoke_acceptance;
mod plugin_e2e_acceptance;
#[cfg(all(unix, feature = "long-running-tests"))]
mod screen_mode_inline_pty;
mod tool_lifecycle_acceptance;
