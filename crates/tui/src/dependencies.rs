//! External-binary dependency resolution for tools that shell out to
//! locally-installed programs (Python for `code_execution` / RLM REPL,
//! `pdftotext` for PDF reading in `read_file`, future tools as added).
//!
//! Before v0.8.31, tools that called external binaries hardcoded the
//! command name and failed at execution time when the binary wasn't on
//! `PATH`. The most-cited example was `code_execution`, which spawned
//! `python3` directly — Windows users (where the launcher is `py` or
//! `python`, not `python3`) saw `Failed to execute tool: program not
//! found` with no upstream hint of what was wrong.
//!
//! This module centralises the probe-then-decide pattern. The supported
//! callers today are:
//!
//! - Tool catalog construction (`core::engine::tool_catalog`): for
//!   tools that should be advertised to the model only when the
//!   required runtime is present.
//! - Doctor command (`run_doctor` in `main.rs`): for surfacing the
//!   resolved state to the user so missing dependencies aren't an
//!   invisible failure.
//! - Long-lived REPL runtime (`repl::runtime`): for RLM and inline `repl`
//!   blocks that need to spawn Python on every supported platform.
//!
//! Results are cached for the process lifetime via [`std::sync::OnceLock`]
//! — probing a binary involves a `Command::output` per candidate and
//! we'd rather not pay that on every model turn.

use std::process::Command;
use std::sync::OnceLock;

/// Candidate executable names for the Python interpreter, in the
/// order we try them. On Windows the launcher convention is `py -3`,
/// so we add it as a third option; the resolver splits on whitespace
/// at execution time so `py -3 /tmp/code.py` runs correctly.
///
/// Order matters: `python3` first because it's the unambiguous v3
/// binary on Unix and rules out Python 2 leftovers. `python` second
/// covers Windows installations that drop the version suffix and
/// modern macOS where Homebrew installs both. `py -3` last as a
/// Windows-launcher fallback.
pub const PYTHON_CANDIDATES: &[&str] = &["python3", "python", "py -3"];

/// Probe a single executable. Returns `true` when the candidate
/// responds to `--version` with a successful exit. Splits on
/// whitespace so `"py -3"` works as a candidate.
///
/// We deliberately use `--version` rather than `which` so the probe
/// is portable across Unix, Windows (no `which` by default), and
/// containers. The downside is that we spawn a subprocess per
/// candidate; the resolver caches the result so this only fires
/// once per process.
#[must_use]
pub fn probe_executable(spec: &str) -> bool {
    let mut parts = spec.split_whitespace();
    let Some(program) = parts.next() else {
        return false;
    };
    let mut cmd = Command::new(program);
    for arg in parts {
        cmd.arg(arg);
    }
    cmd.arg("--version");

    // Silence the subprocess's stdout/stderr — `--version` would
    // otherwise print to our terminal during startup, which is
    // confusing on the TUI's first frame.
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    matches!(cmd.status(), Ok(status) if status.success())
}

/// Resolve the Python interpreter once per process. Returns the
/// candidate spec (e.g. `"python3"` or `"py -3"`) that succeeded,
/// or `None` when every candidate failed.
///
/// Callers that need to spawn the interpreter should split this
/// string on whitespace — see [`split_interpreter_spec`].
pub fn resolve_python_interpreter() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            for candidate in PYTHON_CANDIDATES {
                if probe_executable(candidate) {
                    tracing::info!(
                        target: "tool_dependencies",
                        candidate = candidate,
                        "Resolved Python interpreter",
                    );
                    return Some((*candidate).to_string());
                }
            }
            tracing::warn!(
                target: "tool_dependencies",
                tried = ?PYTHON_CANDIDATES,
                "No Python interpreter found",
            );
            None
        })
        .clone()
}

/// Resolve `pdftotext` (from Poppler) once per process. Used by
/// `read_file`'s PDF path for graceful fallback messaging. Unlike
/// the Python case, `read_file` itself still works for text files
/// when `pdftotext` is missing — this resolver exists so the doctor
/// command can surface the miss explicitly rather than the user
/// hitting "PDF unsupported" on a read attempt.
pub fn resolve_pdftotext() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if probe_executable("pdftotext") {
                Some("pdftotext".to_string())
            } else {
                None
            }
        })
        .clone()
}

/// Resolve `tesseract` (OCR engine) once per process. Used by
/// the `image_ocr` tool to decide whether to register itself with
/// the model. Tesseract is the de-facto open-source OCR engine and
/// ships as a single binary on every platform we support, so the
/// candidate list is just `tesseract`.
pub fn resolve_tesseract() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if probe_executable("tesseract") {
                tracing::info!(
                    target: "tool_dependencies",
                    "Resolved tesseract binary for image_ocr",
                );
                Some("tesseract".to_string())
            } else {
                tracing::warn!(
                    target: "tool_dependencies",
                    "tesseract binary not found; image_ocr tool will not be registered",
                );
                None
            }
        })
        .clone()
}

/// Resolve `pandoc` (universal document converter) once per
/// process. Used by the `pandoc_convert` tool to decide whether
/// to register itself with the model. Pandoc is a single-binary
/// install, so the candidate list is just `pandoc` — no platform
/// fallback path.
pub fn resolve_pandoc() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if probe_executable("pandoc") {
                tracing::info!(
                    target: "tool_dependencies",
                    "Resolved pandoc binary for pandoc_convert",
                );
                Some("pandoc".to_string())
            } else {
                tracing::warn!(
                    target: "tool_dependencies",
                    "pandoc binary not found; pandoc_convert tool will not be registered",
                );
                None
            }
        })
        .clone()
}

/// Resolve the Node.js runtime once per process. Used by the
/// `js_execution` tool to decide whether to advertise itself in
/// the catalog. Unlike Python, the executable name `node` is the
/// same across every platform we ship to — there's no `node3` or
/// `node.exe` variant to fall through to — so this is a single
/// probe rather than a candidate ladder.
pub fn resolve_node() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if probe_executable("node") {
                tracing::info!(
                    target: "tool_dependencies",
                    "Resolved Node.js runtime for js_execution",
                );
                Some("node".to_string())
            } else {
                tracing::warn!(
                    target: "tool_dependencies",
                    "Node.js runtime not found; js_execution tool will not be advertised",
                );
                None
            }
        })
        .clone()
}

/// Split an interpreter spec like `"py -3"` into the program name
/// and any initial arguments. Returns `("py", vec!["-3"])` for the
/// example; returns `("python3", vec![])` for a bare name.
///
/// Callers spawn `Command::new(program).args(args).arg(script_path)`.
#[must_use]
pub fn split_interpreter_spec(spec: &str) -> (String, Vec<String>) {
    let mut parts = spec.split_whitespace();
    let program = parts.next().unwrap_or("").to_string();
    let args = parts.map(str::to_string).collect();
    (program, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_executable_returns_false_for_unknown_binary() {
        // Pick a name we're confident isn't on any developer's PATH.
        // If this ever starts failing locally, rename it.
        assert!(!probe_executable("deepseek-tui-imaginary-binary-xyz123"));
    }

    #[test]
    fn probe_executable_handles_multi_word_specs() {
        // `py -3` should split correctly. The probe will fail on
        // most non-Windows machines (no `py` launcher), which is
        // fine — we're checking that the *split* doesn't crash.
        let _ = probe_executable("py -3");
    }

    #[test]
    fn split_interpreter_spec_strips_args() {
        assert_eq!(
            split_interpreter_spec("python3"),
            ("python3".to_string(), Vec::<String>::new())
        );
        assert_eq!(
            split_interpreter_spec("py -3"),
            ("py".to_string(), vec!["-3".to_string()])
        );
        assert_eq!(
            split_interpreter_spec("  python3  "),
            ("python3".to_string(), Vec::<String>::new()),
            "leading/trailing whitespace must be tolerated"
        );
    }

    #[test]
    fn split_interpreter_spec_handles_empty_string() {
        assert_eq!(
            split_interpreter_spec(""),
            (String::new(), Vec::<String>::new())
        );
    }

    #[test]
    fn python_resolver_is_cached_across_calls() {
        // Whatever the first call returns, subsequent calls return
        // the same value (cached). If this test ever flakes, the
        // OnceLock semantics changed and we need to rethink the
        // resolver.
        let first = resolve_python_interpreter();
        let second = resolve_python_interpreter();
        assert_eq!(first, second);
    }

    #[test]
    fn python_resolver_returns_some_on_developer_machines() {
        // CI hosts have Python; developer machines have Python.
        // The one environment where this returns None is bare-bones
        // Windows / minimal CI containers — fine, those just don't
        // get code_execution registered, which is the whole point.
        // We don't assert Some() because we don't want this test
        // to fail in those environments. Instead we just confirm
        // the resolver doesn't panic and returns a stable value.
        let resolved = resolve_python_interpreter();
        if let Some(name) = resolved {
            assert!(
                !name.is_empty(),
                "resolved interpreter name must be non-empty"
            );
            // The resolved name must be one of our candidates.
            assert!(
                PYTHON_CANDIDATES.contains(&name.as_str()),
                "resolved {name:?} is not in PYTHON_CANDIDATES {PYTHON_CANDIDATES:?}"
            );
        }
    }
}
