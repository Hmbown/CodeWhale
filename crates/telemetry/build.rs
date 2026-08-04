//! Emits `CODEWHALE_RELEASE_BUILD_SHA`, and only that.
//!
//! `option_env!` reads the environment of the crate being compiled, so this has
//! to live here rather than in a consumer's build script: a `rustc-env` emitted
//! by `crates/tui`'s build script is not visible to `crates/telemetry`.

fn main() {
    codewhale_build_support::declare_release_sha_rerun();
    codewhale_build_support::emit_release_build_sha();
}
