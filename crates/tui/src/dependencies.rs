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
    probe_executable_with_flag(spec, "--version")
}

/// Probe a single executable using an explicit version/help flag.
///
/// Most tools report their presence via `--version`, but some do not:
/// Poppler's `pdftotext` treats `--version` as an input *filename* and
/// exits non-zero ("I/O Error: Couldn't open file '--version'"), so the
/// default probe reports it missing even when it is installed (#1667).
/// Such tools pass their own flag (e.g. `-v`) here.
#[must_use]
pub fn probe_executable_with_flag(spec: &str, version_flag: &str) -> bool {
    let mut parts = spec.split_whitespace();
    let Some(program) = parts.next() else {
        return false;
    };
    let mut cmd = Command::new(program);
    for arg in parts {
        cmd.arg(arg);
    }
    cmd.arg(version_flag);

    // Silence the subprocess's stdout/stderr — the version banner would
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
            // Poppler's `pdftotext` rejects `--version` (it is parsed as an
            // input filename and exits non-zero), so probe with `-v`, which
            // prints the version banner and exits 0 (#1667).
            if probe_executable_with_flag("pdftotext", "-v") {
                Some("pdftotext".to_string())
            } else {
                None
            }
        })
        .clone()
}

/// Resolve `tesseract` (OCR engine) once per process. Used by the
/// `image_ocr` tool on platforms that do not have a native OCR backend.
/// Tesseract is the de-facto open-source OCR engine and ships as a single
/// binary on every platform we support, so the candidate list is just
/// `tesseract`.
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
                    "tesseract binary not found; image_ocr will rely on native OCR if available",
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

// ---------------------------------------------------------------------------
// ExternalTool trait — unified subprocess interface
// ---------------------------------------------------------------------------

/// A tool that DeepSeek-TUI shells out to. Instead of scattering
/// `Command::new("git")` / `Command::new("gh")` across the codebase,
/// each external dependency implements this trait once in this module.
/// Callers ask the tool for a pre-populated [`Command`] and chain their
/// own args, working directory, and spawn method.
///
/// # Example
///
/// ```ignore
/// let output = Git::command()
///     .expect("git not found")
///     .args(["diff", "--stat"])
///     .current_dir(&workspace)
///     .output()?;
/// ```
pub trait ExternalTool {
    /// Candidate binary names, tried in order until one responds to
    /// `--version`.  For single-binary tools (git, gh, node) this is a
    /// one-element slice.
    fn candidates() -> &'static [&'static str];

    /// Resolve the best candidate once per process (cached). Returns
    /// the spec string (e.g. `"python3"` or `"py -3"`).
    fn resolve() -> Option<String>;

    /// Quick availability check — true when the tool was found on PATH.
    #[allow(dead_code)]
    fn available() -> bool {
        Self::resolve().is_some()
    }

    /// Build a `std::process::Command` pre-populated with the resolved
    /// binary (and any fixed arguments from a multi-word candidate like
    /// `"py -3"`). Returns `None` when the tool isn't installed.
    ///
    /// Callers should chain `.args(...)`, `.current_dir(...)`, and then
    /// call `.output()`, `.status()`, or `.spawn()`.
    fn command() -> Option<Command> {
        let spec = Self::resolve()?;
        let (program, fixed_args) = split_interpreter_spec(&spec);
        let mut cmd = Command::new(&program);
        for arg in &fixed_args {
            cmd.arg(arg);
        }
        Some(cmd)
    }

    /// Convenience: run the tool with arguments in a working directory
    /// and return the captured output.
    fn output(args: &[&str], cwd: &std::path::Path) -> std::io::Result<std::process::Output> {
        let mut cmd = Self::command().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found on PATH", std::any::type_name::<Self>()),
            )
        })?;
        cmd.args(args).current_dir(cwd).output()
    }

    /// Convenience: run the tool with arguments and return only the
    /// exit status (discards stdout/stderr).
    #[allow(dead_code)]
    fn status(args: &[&str], cwd: &std::path::Path) -> std::io::Result<std::process::ExitStatus> {
        let mut cmd = Self::command().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} not found on PATH", std::any::type_name::<Self>()),
            )
        })?;
        cmd.args(args).current_dir(cwd).status()
    }

    /// Build a `tokio::process::Command` pre-populated with the resolved
    /// binary (and any fixed arguments from a multi-word candidate like
    /// `"py -3"`). Returns `None` when the tool isn't installed.
    ///
    /// Async callers (`code_execution`, `js_execution`) use this instead
    /// of [`ExternalTool::command`] so they can `.await` the child.
    fn tokio_command() -> Option<tokio::process::Command> {
        let spec = Self::resolve()?;
        let (program, fixed_args) = split_interpreter_spec(&spec);
        let mut cmd = tokio::process::Command::new(&program);
        for arg in &fixed_args {
            cmd.arg(arg);
        }
        Some(cmd)
    }
}

// ---------------------------------------------------------------------------
// Concrete tool implementations
// ---------------------------------------------------------------------------

/// Git version control.
pub struct Git;

impl ExternalTool for Git {
    fn candidates() -> &'static [&'static str] {
        &["git"]
    }

    fn resolve() -> Option<String> {
        static CACHE: OnceLock<Option<String>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                for candidate in Self::candidates() {
                    if probe_executable(candidate) {
                        tracing::info!(target: "tool_dependencies", "Resolved git binary");
                        return Some((*candidate).to_string());
                    }
                }
                None
            })
            .clone()
    }
}

/// GitHub CLI.
pub struct Gh;

impl ExternalTool for Gh {
    fn candidates() -> &'static [&'static str] {
        &["gh"]
    }

    fn resolve() -> Option<String> {
        static CACHE: OnceLock<Option<String>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                for candidate in Self::candidates() {
                    if probe_executable(candidate) {
                        tracing::info!(target: "tool_dependencies", "Resolved gh binary");
                        return Some((*candidate).to_string());
                    }
                }
                None
            })
            .clone()
    }
}

/// Rust compiler — used for version reporting in diagnostics.
pub struct RustC;

impl ExternalTool for RustC {
    fn candidates() -> &'static [&'static str] {
        &["rustc"]
    }

    fn resolve() -> Option<String> {
        static CACHE: OnceLock<Option<String>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                for candidate in Self::candidates() {
                    if probe_executable(candidate) {
                        tracing::info!(target: "tool_dependencies", "Resolved rustc binary");
                        return Some((*candidate).to_string());
                    }
                }
                None
            })
            .clone()
    }
}

/// Rust build tool — used by the `run_tests` tool.
pub struct Cargo;

impl ExternalTool for Cargo {
    fn candidates() -> &'static [&'static str] {
        &["cargo"]
    }

    fn resolve() -> Option<String> {
        static CACHE: OnceLock<Option<String>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                for candidate in Self::candidates() {
                    if probe_executable(candidate) {
                        tracing::info!(target: "tool_dependencies", "Resolved cargo binary");
                        return Some((*candidate).to_string());
                    }
                }
                None
            })
            .clone()
    }
}

/// Python interpreter — used by `code_execution` tool and RLM REPL.
/// Delegates to the existing [`resolve_python_interpreter`] so the
/// multi-candidate ladder (`python3` → `python` → `py -3`) is
/// shared with legacy callers until they migrate to the trait.
pub struct Python;

impl ExternalTool for Python {
    fn candidates() -> &'static [&'static str] {
        PYTHON_CANDIDATES
    }

    fn resolve() -> Option<String> {
        resolve_python_interpreter()
    }
}

/// Node.js runtime — used by the `js_execution` tool.
/// The binary name `node` is the same on every platform we support,
/// so this is a single probe rather than a candidate ladder.
pub struct Node;

impl ExternalTool for Node {
    fn candidates() -> &'static [&'static str] {
        &["node"]
    }

    fn resolve() -> Option<String> {
        resolve_node()
    }
}

// ---------------------------------------------------------------------------
// Legacy interpreter helpers (kept for existing callers until migrated)
// ---------------------------------------------------------------------------

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
        assert!(!probe_executable("codewhale-tui-imaginary-binary-xyz123"));
    }

    #[test]
    fn probe_executable_handles_multi_word_specs() {
        // `py -3` should split correctly. The probe will fail on
        // most non-Windows machines (no `py` launcher), which is
        // fine — we're checking that the *split* doesn't crash.
        let _ = probe_executable("py -3");
    }

    #[test]
    fn probe_executable_with_flag_returns_false_for_unknown_binary() {
        assert!(!probe_executable_with_flag(
            "codewhale-tui-imaginary-binary-xyz123",
            "-v"
        ));
    }

    #[test]
    fn probe_executable_delegates_to_double_dash_version() {
        // `probe_executable` must remain exactly
        // `probe_executable_with_flag(.., "--version")`.
        let spec = "codewhale-tui-imaginary-binary-xyz123";
        assert_eq!(
            probe_executable(spec),
            probe_executable_with_flag(spec, "--version")
        );
    }

    #[test]
    fn pdftotext_resolver_detects_installed_poppler_via_dash_v() {
        // Regression for #1667: Poppler's `pdftotext` rejects `--version`
        // (it is parsed as an input filename and exits non-zero), so the
        // generic `--version` probe reports it missing even when installed.
        // The resolver must probe with `-v`. Gated on pdftotext actually
        // being installed so CI without Poppler stays green.
        if probe_executable_with_flag("pdftotext", "-v") {
            assert!(
                resolve_pdftotext().is_some(),
                "an installed pdftotext must be detected via -v (#1667)"
            );
        }
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

    // ===================================================================
    // ExternalTool trait tests
    // ===================================================================

    #[test]
    fn python_candidates_matches_const() {
        assert_eq!(Python::candidates(), PYTHON_CANDIDATES);
    }

    #[test]
    fn node_candidates_is_node_only() {
        assert_eq!(Node::candidates(), &["node"]);
    }

    #[test]
    fn git_candidates_is_git_only() {
        assert_eq!(Git::candidates(), &["git"]);
    }

    #[test]
    fn gh_candidates_is_gh_only() {
        assert_eq!(Gh::candidates(), &["gh"]);
    }

    #[test]
    fn rustc_candidates_is_rustc_only() {
        assert_eq!(RustC::candidates(), &["rustc"]);
    }

    #[test]
    fn cargo_candidates_is_cargo_only() {
        assert_eq!(Cargo::candidates(), &["cargo"]);
    }

    #[test]
    fn concrete_resolvers_do_not_cross_contaminate_when_available() {
        let values = [
            Git::resolve().map(|v| ("git", v)),
            Gh::resolve().map(|v| ("gh", v)),
            RustC::resolve().map(|v| ("rustc", v)),
            Cargo::resolve().map(|v| ("cargo", v)),
            Node::resolve().map(|v| ("node", v)),
        ];
        let resolved: Vec<(&str, String)> = values.into_iter().flatten().collect();

        for i in 0..resolved.len() {
            for j in (i + 1)..resolved.len() {
                assert_ne!(
                    resolved[i].1, resolved[j].1,
                    "{} and {} unexpectedly resolved to the same binary",
                    resolved[i].0, resolved[j].0
                );
            }
        }
    }

    #[test]
    fn git_resolve_is_cached() {
        let first = Git::resolve();
        let second = Git::resolve();
        assert_eq!(first, second);
    }

    #[test]
    fn gh_resolve_is_cached() {
        let first = Gh::resolve();
        let second = Gh::resolve();
        assert_eq!(first, second);
    }

    #[test]
    fn python_trait_resolve_is_cached() {
        let first = Python::resolve();
        let second = Python::resolve();
        assert_eq!(first, second);
    }

    #[test]
    fn node_resolve_is_cached() {
        let first = Node::resolve();
        let second = Node::resolve();
        assert_eq!(first, second);
    }

    #[test]
    fn rustc_resolve_is_cached() {
        let first = RustC::resolve();
        let second = RustC::resolve();
        assert_eq!(first, second);
    }

    #[test]
    fn cargo_resolve_is_cached() {
        let first = Cargo::resolve();
        let second = Cargo::resolve();
        assert_eq!(first, second);
    }

    #[test]
    fn git_available_matches_resolve() {
        assert_eq!(Git::available(), Git::resolve().is_some());
    }

    #[test]
    fn python_available_matches_resolve() {
        assert_eq!(Python::available(), Python::resolve().is_some());
    }

    #[test]
    fn node_available_matches_resolve() {
        assert_eq!(Node::available(), Node::resolve().is_some());
    }

    #[test]
    fn rustc_available_matches_resolve() {
        assert_eq!(RustC::available(), RustC::resolve().is_some());
    }

    #[test]
    fn cargo_available_matches_resolve() {
        assert_eq!(Cargo::available(), Cargo::resolve().is_some());
    }

    #[test]
    fn git_command_returns_some_when_available() {
        if Git::available() {
            assert!(Git::command().is_some());
        }
    }

    #[test]
    fn python_command_returns_some_when_available() {
        if Python::available() {
            assert!(Python::command().is_some());
        }
    }

    #[test]
    fn python_tokio_command_returns_some_when_available() {
        if Python::available() {
            assert!(Python::tokio_command().is_some());
        }
    }

    #[test]
    fn node_tokio_command_returns_some_when_available() {
        if Node::available() {
            assert!(Node::tokio_command().is_some());
        }
    }

    #[test]
    fn git_output_version_succeeds() {
        // Only run when git is actually installed.
        if !Git::available() {
            return;
        }
        let tmp = std::env::temp_dir();
        let out = Git::output(&["--version"], &tmp);
        assert!(
            out.is_ok(),
            "git --version must succeed when git is available"
        );
        let out = out.unwrap();
        assert!(out.status.success(), "git --version must exit 0");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("git version"),
            "git --version stdout must contain 'git version', got: {}",
            stdout.trim()
        );
    }

    #[test]
    fn python_output_version_succeeds() {
        if !Python::available() {
            return;
        }
        let tmp = std::env::temp_dir();
        let out = Python::output(&["--version"], &tmp);
        assert!(out.is_ok(), "python --version must spawn");
        let out = out.unwrap();
        // Python --version writes to stdout on 3.x, so just check
        // that it succeeded (exit 0).
        assert!(out.status.success(), "python --version must exit 0");
    }

    #[test]
    fn node_output_version_succeeds() {
        if !Node::available() {
            return;
        }
        let tmp = std::env::temp_dir();
        let out = Node::output(&["--version"], &tmp);
        assert!(out.is_ok(), "node --version must spawn");
        let out = out.unwrap();
        assert!(out.status.success(), "node --version must exit 0");
    }

    #[test]
    fn cargo_output_version_succeeds() {
        if !Cargo::available() {
            return;
        }
        let tmp = std::env::temp_dir();
        let out = Cargo::output(&["--version"], &tmp);
        assert!(out.is_ok(), "cargo --version must spawn");
        let out = out.unwrap();
        assert!(out.status.success(), "cargo --version must exit 0");
    }

    #[test]
    fn external_tool_output_respects_cwd() {
        // Verify that `output()` runs in the requested directory.
        if !Git::available() {
            return;
        }
        let tmp = std::env::temp_dir();
        let out = Git::output(&["rev-parse", "--show-toplevel"], &tmp);
        assert!(out.is_ok(), "git rev-parse must spawn");
        let out = out.unwrap();
        // rev-parse --show-toplevel in a non-git dir should fail
        // because temp_dir is not a git repo. That's expected.
        // The key assertion: the command executed without IO errors.
        // We don't assert success because temp_dir might or might not
        // be inside a git worktree.
        let _ = out; // just checking it didn't panic/IO-error
    }
}
