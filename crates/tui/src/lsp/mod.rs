//! LSP integration: post-edit diagnostics injection (#136).
//!
//! After the agent performs a successful file edit (`edit_file`,
//! `apply_patch`, or `write_file`) the engine asks the [`LspManager`] for
//! diagnostics on that file. The manager spawns the appropriate LSP server
//! lazily on first use, sends `didOpen`/`didChange`, waits up to a bounded
//! timeout for `publishDiagnostics`, normalizes the result, and returns it
//! to the engine.
//!
//! Failure modes are non-blocking by design: a missing LSP binary, a
//! crashed server, or a timeout all degrade to "no diagnostics this turn"
//! rather than stalling the agent. We log a one-time warning per language
//! when the binary is missing.
//!
//! # Wiring
//!
//! ```text
//! Engine  ── after successful edit ──▶  LspManager.diagnostics_for(path, seq)
//!                                              │
//!                                              ▼
//!                                       per-language LspClient
//!                                              │
//!                                              ▼
//!                                      LspTransport (stdio)
//! ```
//!
//! # Configuration
//!
//! The `[lsp]` table in `~/.deepseek/config.toml` controls behavior:
//! `enabled`, `poll_after_edit_ms`, `max_diagnostics_per_file`,
//! `include_warnings`, and an optional `servers` override. See
//! [`LspConfig`] for defaults and `config.example.toml` for documentation.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

pub mod client;
pub mod diagnostics;
pub mod registry;

pub use client::{LspTransport, StdioLspTransport};
pub use diagnostics::{Diagnostic, DiagnosticBlock, Severity, render_blocks};
pub use registry::Language;

/// `[lsp]` config schema. Mirrors the TOML keys documented in
/// `config.example.toml`. Unknown keys are ignored.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LspConfig {
    /// Master switch. When `false`, the manager skips every operation and
    /// returns an empty diagnostics list.
    pub enabled: bool,
    /// Maximum time in milliseconds to wait for the LSP server to publish
    /// diagnostics after a `didOpen`/`didChange`. Default 5000 ms.
    pub poll_after_edit_ms: u64,
    /// Maximum diagnostics to keep per file. Excess items are dropped after
    /// sorting by severity. Default 20.
    pub max_diagnostics_per_file: usize,
    /// When `true`, warnings (severity 2) are kept in the output. When
    /// `false` (default), only errors (severity 1) are surfaced.
    pub include_warnings: bool,
    /// Optional override for the `Language -> (cmd, args)` table. Keys use
    /// [`Language::as_key`] (e.g. `"rust"`).
    pub servers: HashMap<String, Vec<String>>,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_after_edit_ms: 5_000,
            max_diagnostics_per_file: 20,
            include_warnings: false,
            servers: HashMap::new(),
        }
    }
}

impl LspConfig {
    /// Resolve `(command, args)` for `lang`. User-supplied overrides take
    /// precedence over the built-in registry.
    fn resolve_command(&self, lang: Language) -> Option<(String, Vec<String>)> {
        if let Some(parts) = self.servers.get(lang.as_key())
            && let Some((first, rest)) = parts.split_first()
        {
            return Some((first.clone(), rest.to_vec()));
        }
        let (cmd, args) = registry::server_for(lang)?;
        Some((
            cmd.to_string(),
            args.iter().map(|a| (*a).to_string()).collect(),
        ))
    }
}

/// The LspManager holds a lazily populated map of `Language -> Transport`.
/// One transport is reused across files of the same language for the
/// session's lifetime.
pub struct LspManager {
    config: LspConfig,
    workspace: PathBuf,
    /// Per-language transports. Wrapped in `Arc` so we can release the outer
    /// lock before driving I/O on a single transport.
    transports: AsyncMutex<HashMap<Language, Arc<dyn LspTransport>>>,
    /// Per-language "we already warned the user that the binary is missing"
    /// guard so we do not spam the audit log on every edit.
    missing_warned: AsyncMutex<HashSet<Language>>,
    /// Test seam: when set, `diagnostics_for` uses these instead of spawning
    /// real LSP processes. Keyed by language.
    test_transports: AsyncMutex<HashMap<Language, Arc<dyn LspTransport>>>,
}

impl LspManager {
    /// Build a new manager. Does not spawn any LSP servers — that is lazy.
    #[must_use]
    pub fn new(config: LspConfig, workspace: PathBuf) -> Self {
        Self {
            config,
            workspace,
            transports: AsyncMutex::new(HashMap::new()),
            missing_warned: AsyncMutex::new(HashSet::new()),
            test_transports: AsyncMutex::new(HashMap::new()),
        }
    }

    /// Read-only access to the resolved config. Used by the engine to skip
    /// the post-edit hook entirely when `enabled = false`.
    #[must_use]
    pub fn config(&self) -> &LspConfig {
        &self.config
    }

    /// Inject a fake transport for a language. Used by tests so we never
    /// fork a real LSP server in CI.
    #[cfg(test)]
    pub async fn install_test_transport(&self, lang: Language, transport: Arc<dyn LspTransport>) {
        self.test_transports.lock().await.insert(lang, transport);
    }

    /// Poll the LSP server for diagnostics on `file`. Returns the rendered
    /// [`DiagnosticBlock`] (already truncated to the configured per-file
    /// max) or `None` when the manager is disabled / has no server / the
    /// poll times out.
    ///
    /// The `_edit_seq` argument is currently a no-op; it exists in the
    /// signature so the engine can correlate diagnostics back to a specific
    /// edit when we add request batching in v0.7.x.
    pub async fn diagnostics_for(&self, file: &Path, _edit_seq: u64) -> Option<DiagnosticBlock> {
        if !self.config.enabled {
            return None;
        }
        let lang = registry::detect_language(file);
        if lang == Language::Other {
            return None;
        }

        let text = match tokio::fs::read_to_string(file).await {
            Ok(text) => text,
            Err(err) => {
                tracing::debug!(?err, file = %file.display(), "lsp: read file failed");
                return None;
            }
        };

        let transport = match self.transport_for(lang).await {
            Some(t) => t,
            None => return None,
        };

        let wait = Duration::from_millis(self.config.poll_after_edit_ms);
        let inner_wait = wait;
        let raw = match timeout(wait, transport.diagnostics_for(file, &text, inner_wait)).await {
            Ok(Ok(items)) => items,
            Ok(Err(err)) => {
                tracing::debug!(?err, file = %file.display(), "lsp: diagnostics call failed");
                return None;
            }
            Err(_) => {
                tracing::debug!(file = %file.display(), "lsp: diagnostics timed out");
                return None;
            }
        };

        // Filter, sort, and truncate.
        let include_warnings = self.config.include_warnings;
        let mut items: Vec<Diagnostic> = raw
            .into_iter()
            .filter(|d| match d.severity {
                Severity::Error => true,
                Severity::Warning => include_warnings,
                _ => false,
            })
            .collect();
        items.sort_by_key(|d| match d.severity {
            Severity::Error => 0u8,
            Severity::Warning => 1u8,
            Severity::Information => 2u8,
            Severity::Hint => 3u8,
        });
        let mut block = DiagnosticBlock {
            file: relative_to_workspace(&self.workspace, file),
            items,
        };
        block.truncate(self.config.max_diagnostics_per_file);
        if block.items.is_empty() {
            None
        } else {
            Some(block)
        }
    }

    /// Resolve (and lazily spawn) the transport for `lang`. Tests can
    /// short-circuit this via `install_test_transport` (cfg-test only).
    async fn transport_for(&self, lang: Language) -> Option<Arc<dyn LspTransport>> {
        if let Some(t) = self.test_transports.lock().await.get(&lang) {
            return Some(t.clone());
        }

        if let Some(t) = self.transports.lock().await.get(&lang) {
            return Some(t.clone());
        }

        let (cmd, args) = self.config.resolve_command(lang)?;
        match StdioLspTransport::spawn(&cmd, &args, lang, self.workspace.clone()).await {
            Ok(transport) => {
                let arc: Arc<dyn LspTransport> = Arc::new(transport);
                self.transports.lock().await.insert(lang, arc.clone());
                Some(arc)
            }
            Err(err) => {
                self.warn_missing_once(lang, &cmd, &err).await;
                None
            }
        }
    }

    async fn warn_missing_once(&self, lang: Language, cmd: &str, err: &anyhow::Error) {
        let mut warned = self.missing_warned.lock().await;
        if warned.insert(lang) {
            tracing::warn!(
                language = %lang.as_key(),
                command = %cmd,
                error = %err,
                "lsp: server unavailable; diagnostics disabled for this language"
            );
        }
    }

    /// Best-effort shutdown of every spawned transport. Called when the
    /// session ends.
    #[allow(dead_code)] // Wired in engine.rs:1734 (see #3490)
    pub async fn shutdown_all(&self) {
        let transports: Vec<Arc<dyn LspTransport>> =
            self.transports.lock().await.values().cloned().collect();
        for transport in transports {
            transport.shutdown().await;
        }
    }
}

/// Render `path` relative to the workspace when possible. Falls back to
/// `path.file_name()` (per the issue's hard rule about not using
/// `display().to_string()` on the bare path) when relativization fails.
fn relative_to_workspace(workspace: &Path, path: &Path) -> PathBuf {
    if let Ok(rel) = path.strip_prefix(workspace) {
        return rel.to_path_buf();
    }
    PathBuf::from(
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("unknown")),
    )
}

/// Used for tests / no-op runs. Builds an empty manager that always returns
/// `None`. Needed because the engine constructs an `LspManager` even when
/// the user has disabled LSP, so the field is always present.
impl LspManager {
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(
            LspConfig {
                enabled: false,
                ..LspConfig::default()
            },
            PathBuf::new(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fake transport: returns a fixed list of diagnostics. Used by
    /// integration tests so we never spawn a real LSP server in CI.
    pub(crate) struct FakeTransport {
        items: Vec<Diagnostic>,
        calls: AtomicUsize,
    }

    impl FakeTransport {
        pub(crate) fn new(items: Vec<Diagnostic>) -> Self {
            Self {
                items,
                calls: AtomicUsize::new(0),
            }
        }

        pub(crate) fn call_count(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl LspTransport for FakeTransport {
        async fn diagnostics_for(
            &self,
            _path: &Path,
            _text: &str,
            _wait: Duration,
        ) -> anyhow::Result<Vec<Diagnostic>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.items.clone())
        }

        async fn shutdown(&self) {}
    }

    #[tokio::test]
    async fn returns_none_when_disabled() {
        let mgr = LspManager::new(
            LspConfig {
                enabled: false,
                ..LspConfig::default()
            },
            PathBuf::from("/tmp"),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();
        assert!(mgr.diagnostics_for(&path, 1).await.is_none());
    }

    #[tokio::test]
    async fn returns_none_for_unknown_language() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(LspConfig::default(), dir.path().to_path_buf());
        let path = dir.path().join("notes.txt");
        tokio::fs::write(&path, b"hi").await.unwrap();
        assert!(mgr.diagnostics_for(&path, 1).await.is_none());
    }

    #[tokio::test]
    async fn forwards_errors_through_fake_transport() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(LspConfig::default(), dir.path().to_path_buf());
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"let x: i32 = \"oops\";")
            .await
            .unwrap();

        let fake = Arc::new(FakeTransport::new(vec![Diagnostic {
            line: 1,
            column: 14,
            severity: Severity::Error,
            message: "expected i32, found &str".to_string(),
        }]));
        mgr.install_test_transport(Language::Rust, fake.clone())
            .await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        let rendered = block.render();
        assert!(rendered.contains("ERROR [1:14] expected i32, found &str"));
        assert!(rendered.contains("foo.rs"));
        assert_eq!(fake.call_count(), 1);
    }

    #[tokio::test]
    async fn drops_warnings_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(LspConfig::default(), dir.path().to_path_buf());
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let fake = Arc::new(FakeTransport::new(vec![
            Diagnostic {
                line: 1,
                column: 1,
                severity: Severity::Warning,
                message: "unused import".to_string(),
            },
            Diagnostic {
                line: 2,
                column: 1,
                severity: Severity::Error,
                message: "type error".to_string(),
            },
        ]));
        mgr.install_test_transport(Language::Rust, fake).await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        assert_eq!(block.items.len(), 1);
        assert_eq!(block.items[0].severity, Severity::Error);
    }

    #[tokio::test]
    async fn keeps_warnings_when_opted_in() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(
            LspConfig {
                include_warnings: true,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let fake = Arc::new(FakeTransport::new(vec![
            Diagnostic {
                line: 1,
                column: 1,
                severity: Severity::Warning,
                message: "unused".to_string(),
            },
            Diagnostic {
                line: 2,
                column: 1,
                severity: Severity::Error,
                message: "broken".to_string(),
            },
        ]));
        mgr.install_test_transport(Language::Rust, fake).await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        assert_eq!(block.items.len(), 2);
        // Errors come first after sorting.
        assert_eq!(block.items[0].severity, Severity::Error);
        assert_eq!(block.items[1].severity, Severity::Warning);
    }

    #[tokio::test]
    async fn truncates_to_max_per_file() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LspManager::new(
            LspConfig {
                max_diagnostics_per_file: 3,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let path = dir.path().join("foo.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let fake = Arc::new(FakeTransport::new(
            (0..10)
                .map(|i| Diagnostic {
                    line: i + 1,
                    column: 1,
                    severity: Severity::Error,
                    message: format!("err {i}"),
                })
                .collect(),
        ));
        mgr.install_test_transport(Language::Rust, fake).await;

        let block = mgr.diagnostics_for(&path, 1).await.expect("has block");
        assert_eq!(block.items.len(), 3);
    }

    #[tokio::test]
    async fn render_blocks_concatenates() {
        let blocks = vec![
            DiagnosticBlock {
                file: PathBuf::from("a.rs"),
                items: vec![Diagnostic {
                    line: 1,
                    column: 1,
                    severity: Severity::Error,
                    message: "err in a".to_string(),
                }],
            },
            DiagnosticBlock {
                file: PathBuf::from("b.rs"),
                items: vec![Diagnostic {
                    line: 2,
                    column: 2,
                    severity: Severity::Error,
                    message: "err in b".to_string(),
                }],
            },
        ];
        let rendered = render_blocks(&blocks);
        assert!(rendered.contains("file=\"a.rs\""));
        assert!(rendered.contains("file=\"b.rs\""));
    }

    #[test]
    fn relative_path_falls_back_to_filename_when_outside_workspace() {
        let workspace = PathBuf::from("/foo/bar");
        let path = PathBuf::from("/baz/qux.rs");
        assert_eq!(
            relative_to_workspace(&workspace, &path),
            PathBuf::from("qux.rs")
        );
    }

    #[test]
    fn config_resolve_uses_overrides() {
        let mut cfg = LspConfig::default();
        cfg.servers.insert(
            "rust".to_string(),
            vec!["custom-rls".to_string(), "--lsp".to_string()],
        );
        let (cmd, args) = cfg.resolve_command(Language::Rust).unwrap();
        assert_eq!(cmd, "custom-rls");
        assert_eq!(args, vec!["--lsp".to_string()]);
    }

    #[test]
    fn config_resolve_falls_back_to_registry() {
        let cfg = LspConfig::default();
        let (cmd, _) = cfg.resolve_command(Language::Rust).unwrap();
        assert_eq!(cmd, "rust-analyzer");
    }
}
