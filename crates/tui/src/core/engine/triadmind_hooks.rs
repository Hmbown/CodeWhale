//! # TriadMind Hooks — Architecture governance integration for the agent engine
//!
//! Post-edit hooks that run TriadMind sync + verify after every file modification,
//! following the same pattern as `lsp_hooks.rs`.
//!
//! When the agent edits a source file (via `edit_file`, `apply_patch`, or `write_file`),
//! this hook:
//! 1. Checks if the edited file is a recognized source file (`is_source_file`)
//! 2. Runs `sync_triad_map` to detect changes
//! 3. If changes detected, runs `run_topology_verify`
//! 4. If verify fails, queues diagnostic messages for the model's next request
//!
//! @See: `lsp_hooks.rs` for the integration pattern this module follows.

use std::path::PathBuf;

use super::*;

/// Check whether a tool call edited files that are source files warranting
/// TriadMind sync. Reuses the LSP hook's path extraction logic.
pub(super) fn triadmind_relevant_paths(
    tool_name: &str,
    tool_input: &serde_json::Value,
    workspace_root: &PathBuf,
) -> Vec<PathBuf> {
    // Reuse the path extraction from lsp_hooks — same tools, same paths.
    let all_paths = super::lsp_hooks::edited_paths_for_tool(tool_name, tool_input);
    all_paths
        .into_iter()
        .filter(|p| {
            let abs = if p.is_absolute() {
                p.clone()
            } else {
                workspace_root.join(p)
            };
            let rel = abs
                .strip_prefix(workspace_root)
                .unwrap_or(&abs)
                .to_string_lossy()
                .replace('\\', "/");
            deepseek_triadmind::sync::is_source_file(&rel)
        })
        .collect()
}

impl Engine {
    /// Post-edit TriadMind hook. After a successful file edit, runs sync + verify
    /// on the project's triad map. Diagnostic messages are queued via
    /// `pending_triadmind_messages` and flushed before the next API request.
    pub(super) async fn run_post_edit_triadmind_hook(
        &mut self,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) {
        // ── Guard: check config ────────────────────────────────────
        // TriadMind is enabled by default; can be disabled via config
        // TODO: read from user config when triadmind settings are available
        let triadmind_enabled = true; // Enable for now
        if !triadmind_enabled {
            return;
        }

        // ── Extract relevant paths ────────────────────────────────
        let workspace = self.session.workspace.clone();
        let source_paths =
            triadmind_relevant_paths(tool_name, tool_input, &workspace);

        if source_paths.is_empty() {
            return;
        }

        // ── Run sync ──────────────────────────────────────────────
        let paths = deepseek_triadmind::config::WorkspacePaths::new(&workspace);
        match deepseek_triadmind::sync::sync_triad_map(&paths, false) {
            Ok(sync_result) => {
                if !sync_result.changed {
                    return;
                }

                // ── Run verify if changed ─────────────────────────
                let map_path = paths.map_file;
                if map_path.exists() {
                    match std::fs::read_to_string(&map_path) {
                        Ok(content) => {
                            let trimmed = content.trim().trim_start_matches('\u{FEFF}');
                            if let Ok(nodes) = serde_json::from_str::<
                                Vec<deepseek_triadmind::protocol::TriadNodeDefinition>,
                            >(trimmed)
                            {
                                let report = deepseek_triadmind::verify::run_topology_verify(
                                    &map_path.to_string_lossy(),
                                    &workspace.to_string_lossy(),
                                    &nodes,
                                    &Default::default(),
                                );
                                if !report.passed {
                                    let msg = format_triadmind_diagnostic(&report);
                                    self.pending_triadmind_messages.push(msg);
                                }
                            }
                        }
                        Err(_e) => {
                            // Map file read error — skip silently
                        }
                    }
                }
            }
            Err(_e) => {
                // Sync error — skip silently
            }
        }
    }

    /// Drain pending TriadMind diagnostic messages into the session message stream,
    /// so the model sees architecture warnings on its next request.
    pub(super) async fn flush_pending_triadmind_diagnostics(&mut self) {
        let messages = std::mem::take(&mut self.pending_triadmind_messages);
        for msg in messages {
            self.add_session_message(self.user_text_message_with_turn_metadata(msg))
                .await;
        }
    }
}

/// Format a verify report into a human-readable diagnostic message for the model.
fn format_triadmind_diagnostic(report: &deepseek_triadmind::verify::VerifyReport) -> String {
    let mut lines = vec![
        "── TriadMind Architecture Check ──".to_string(),
        format!(
            "  Nodes: {} | Execute-like: {:.1}% | Ghost: {:.1}% | Empty: {}",
            report.metrics.triad_nodes,
            report.metrics.execute_like_ratio * 100.0,
            report.metrics.ghost_ratio * 100.0,
            report.metrics.empty_vertices,
        ),
    ];

    // Add failing checks
    let failures: Vec<_> = report.checks.iter().filter(|c| c.status == "fail").collect();
    if !failures.is_empty() {
        lines.push(String::new());
        lines.push("  Issues:".to_string());
        for check in failures {
            lines.push(format!(
                "    - {}: {} (threshold: {}, actual: {})",
                check.key, check.detail, check.expected, check.actual
            ));
        }
        lines.push(String::new());
        lines.push(
            "  Recommendation: Run `triadmind sync --force` and review the topology map."
                .to_string(),
        );
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepseek_triadmind::verify::{
        VerifyCheckResult, VerifyMetrics, VerifyReport, VerifyThresholds,
    };

    #[test]
    fn test_triadmind_relevant_paths_filters_non_source() {
        let paths = triadmind_relevant_paths(
            "write_file",
            &serde_json::json!({"path": "README.md"}),
            &PathBuf::from("/project"),
        );
        assert!(paths.is_empty());
    }

    #[test]
    fn test_triadmind_relevant_paths_accepts_rust() {
        let paths = triadmind_relevant_paths(
            "edit_file",
            &serde_json::json!({"path": "src/main.rs"}),
            &PathBuf::from("/project"),
        );
        // In test context, the path passes through and is_source_file
        // on "src/main.rs" returns true.
        assert!(!paths.is_empty());
    }

    #[test]
    fn test_format_diagnostic_with_failures() {
        let report = VerifyReport {
            generated_at: "2026-01-01T00:00:00Z".into(),
            project_root: "/test".into(),
            strict: false,
            thresholds: VerifyThresholds::default(),
            map_file: "triad-map.json".into(),
            passed: false,
            metrics: VerifyMetrics {
                triad_nodes: 50,
                execute_like_ratio: 0.3,
                ghost_ratio: 0.5,
                empty_vertices: 3,
                ..Default::default()
            },
            checks: vec![
                VerifyCheckResult {
                    key: "execute_like_ratio".into(),
                    status: "fail".into(),
                    expected: "<= 0.1".into(),
                    actual: "0.3".into(),
                    detail: "Too many execute-like nodes".into(),
                },
                VerifyCheckResult {
                    key: "ghost_ratio".into(),
                    status: "pass".into(),
                    expected: "<= 0.4".into(),
                    actual: "0.5".into(),
                    detail: "Ghost ratio within limits".into(),
                },
            ],
        };

        let msg = format_triadmind_diagnostic(&report);
        assert!(msg.contains("TriadMind Architecture Check"));
        assert!(msg.contains("execute_like_ratio"));
        assert!(msg.contains("Recommendation"));
    }

    #[test]
    fn test_format_diagnostic_all_pass() {
        let report = VerifyReport {
            generated_at: "2026-01-01T00:00:00Z".into(),
            project_root: "/test".into(),
            strict: false,
            thresholds: VerifyThresholds::default(),
            map_file: "triad-map.json".into(),
            passed: true,
            metrics: VerifyMetrics::default(),
            checks: vec![VerifyCheckResult {
                key: "execute_like_ratio".into(),
                status: "pass".into(),
                expected: "<= 0.1".into(),
                actual: "0.05".into(),
                detail: "All good".into(),
            }],
        };

        let msg = format_triadmind_diagnostic(&report);
        assert!(msg.contains("TriadMind Architecture Check"));
        assert!(!msg.contains("Issues:"));
    }
}
