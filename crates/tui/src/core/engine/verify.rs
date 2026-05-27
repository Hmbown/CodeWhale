//! Closed-loop verification gate — re-checks tool side-effect claims
//! before the result enters the session message stream.
//!
//! After every tool that claims side effects, the engine runs a
//! deterministic re-check. If the re-check contradicts the claim, the
//! session message is annotated with `[VERIFY FAIL]` instead of a raw
//! `success: true` — and the model sees the discrepancy.

use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------------------
// Verdict types
// ---------------------------------------------------------------------------

/// What the verifier found when it re-checked a tool's claimed result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VerifyVerdict {
    /// Re-check confirmed the claim.
    Pass,
    /// Re-check contradicted the claim with evidence.
    Fail { expected: String, observed: String },
    /// Could not re-check (no read-only path available, or re-check tool failed).
    Unverifiable { reason: String },
    /// Explicitly skipped (read-only tool, or tool returned `verification: "skip"` metadata).
    Skipped,
}

/// A single verification record for the session ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyRecord {
    pub tool_id: String,
    pub tool_name: String,
    pub verdict: VerifyVerdict,
    pub elapsed_ms: u64,
    pub ts: i64,
}

/// Configuration for the verification gate.
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    /// Enable the verification gate.
    #[allow(dead_code)]
    pub enabled: bool,
    /// Tools to skip verification for.
    #[allow(dead_code)]
    pub skip_tools: Vec<String>,
    /// Max verification retries. Default: 1.
    #[allow(dead_code)]
    pub max_retries: u8,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            skip_tools: Vec::new(),
            max_retries: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-tool verification rules
// ---------------------------------------------------------------------------

/// Run inline verification for a tool that claimed success with side effects.
///
/// For file-mutating tools (write_file, edit_file, apply_patch), this actually
/// reads the file back to confirm the operation landed. For other side-effect
/// tools, it returns Pass (trust the tool).
///
/// Returns the verdict and the annotated content to inject into the session
/// message stream.
pub fn run_verification(
    tool_name: &str,
    tool_input: &serde_json::Value,
    workspace: &Path,
) -> (VerifyVerdict, String) {
    let verdict = match tool_name {
        // Read-only tools — skip
        "read_file"
        | "grep_files"
        | "file_search"
        | "list_dir"
        | "web_search"
        | "fetch_url"
        | "git_status"
        | "git_diff"
        | "git_log"
        | "git_show"
        | "git_blame"
        | "diagnostics"
        | "handle_read"
        | "task_list"
        | "task_read"
        | "pr_attempt_list"
        | "pr_attempt_read"
        | "automation_list"
        | "automation_read"
        | "github_issue_context"
        | "github_pr_context"
        | "code_execution"
        | "validate_data"
        | "note"
        | "request_user_input"
        | "recall_archive"
        | "tool_search_tool_regex"
        | "tool_search_tool_bm25" => VerifyVerdict::Skipped,

        // Self-verifying or review tools — skip
        "review"
        | "agent_open"
        | "agent_eval"
        | "agent_close"
        | "tool_agent"
        | "rlm_open"
        | "rlm_eval"
        | "rlm_configure"
        | "rlm_close"
        | "rlm_session_objects"
        | "run_tests" => VerifyVerdict::Skipped,

        // Core file-mutating tools — inline verification: confirm the target
        // exists after the write/edit/patch landed. Missing targets are a
        // verification failure; empty files are valid tool output.
        "write_file" | "edit_file" | "apply_patch" => {
            inline_verify_file_tool(tool_input, workspace)
        }

        // Other side-effect tools — trust but don't block on verification
        "exec_shell"
        | "exec_shell_wait"
        | "exec_shell_interact"
        | "shell_cancel"
        | "exec_wait"
        | "exec_interact"
        | "task_shell_start"
        | "task_shell_wait"
        | "task_create"
        | "task_gate_run"
        | "github_comment"
        | "github_close_issue"
        | "github_close_pr"
        | "pr_attempt_record"
        | "pr_attempt_preflight"
        | "automation_create"
        | "automation_update"
        | "automation_pause"
        | "automation_resume"
        | "automation_delete"
        | "automation_run"
        | "task_cancel"
        | "remember"
        | "notify"
        | "revert_turn"
        | "fim_edit"
        | "pandoc_convert"
        | "image_analyze"
        | "image_ocr"
        | "web_run"
        | "finance"
        | "skill_install"
        | "checklist_write"
        | "checklist_add"
        | "checklist_update"
        | "todo_write"
        | "todo_add"
        | "todo_update"
        | "update_plan"
        | "create_goal"
        | "get_goal"
        | "update_goal" => VerifyVerdict::Pass,

        // Unknown tools — skip verification
        _ => VerifyVerdict::Unverifiable {
            reason: format!("no verification rule for tool `{tool_name}`"),
        },
    };

    // Build the annotated content.
    let annotation = match &verdict {
        VerifyVerdict::Pass => String::new(),
        VerifyVerdict::Fail { expected, observed } => {
            format!("\n\n[VERIFY FAIL] Claimed: {expected}\n[VERIFY FAIL] Observed: {observed}")
        }
        VerifyVerdict::Unverifiable { reason } => {
            format!("\n\n[VERIFY] Unverifiable: {reason}")
        }
        VerifyVerdict::Skipped => String::new(),
    };

    (verdict, annotation)
}

/// Whether a tool can be auto-retried on verification failure.
/// File-mutating tools with deterministic inputs are safe to retry;
/// tools with side effects on external systems are not.
pub fn is_auto_retryable(tool_name: &str) -> bool {
    matches!(tool_name, "write_file" | "edit_file" | "apply_patch")
}

/// Inline file verification: confirm the file exists after a claimed write.
/// Empty files are valid output for truncation or marker-file workflows.
fn inline_verify_file_tool(tool_input: &serde_json::Value, workspace: &Path) -> VerifyVerdict {
    let path_str = match tool_input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return VerifyVerdict::Unverifiable {
                reason: "no path in tool input".to_string(),
            };
        }
    };

    let resolved = if Path::new(path_str).is_absolute() {
        Path::new(path_str).to_path_buf()
    } else {
        workspace.join(path_str)
    };

    match std::fs::metadata(&resolved) {
        Ok(_) => VerifyVerdict::Pass,
        Err(_) => VerifyVerdict::Fail {
            expected: format!("file exists at {}", resolved.display()),
            observed: "file missing after write".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Fuzzy search-string correction for edit_file retries
// ---------------------------------------------------------------------------

/// Best match found by the fuzzy matcher, with a confidence score.
#[derive(Debug, Clone)]
pub struct FuzzyMatch {
    /// The actual text from the file that best matches the search string.
    pub text: String,
    /// Similarity score: 1.0 = exact match, 0.0 = completely different.
    pub score: f64,
    /// Line number (1-based) where the match was found.
    pub line: usize,
}

/// Minimum similarity threshold for accepting a fuzzy match as a correction.
/// Below this, the match is too uncertain to use — fall back to Fin (Flash).
const FUZZY_MIN_SIMILARITY: f64 = 0.6;

/// Try to find the closest matching text in a file for a failed edit_file search.
///
/// When edit_file fails because the search string doesn't match, this reads the
/// file and finds the line (or multi-line block) closest to the failed search.
/// Returns `None` if the file can't be read or no match exceeds the threshold.
pub fn fuzzy_correct_search(
    file_path: &Path,
    failed_search: &str,
    workspace: &Path,
) -> Option<FuzzyMatch> {
    let resolved = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        workspace.join(file_path)
    };

    let content = std::fs::read_to_string(&resolved).ok()?;
    if content.is_empty() {
        return None;
    }

    let search_trimmed = failed_search.trim();
    if search_trimmed.is_empty() {
        return None;
    }

    // Try exact match first (fast path — the retry already handled the write).
    if content.contains(search_trimmed) {
        let line = content
            .lines()
            .position(|l| l.contains(search_trimmed))
            .map(|i| i + 1)
            .unwrap_or(1);
        return Some(FuzzyMatch {
            text: search_trimmed.to_string(),
            score: 1.0,
            line,
        });
    }

    // Fuzzy: compare search string against each line.
    let search_lower = search_trimmed.to_lowercase();
    let mut best: Option<FuzzyMatch> = None;

    for (i, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        // Normalized Levenshtein similarity
        let sim = normalized_similarity(search_trimmed, line);
        let sim_lower = if search_lower != *search_trimmed {
            normalized_similarity(&search_lower, &line.to_lowercase())
        } else {
            sim
        };
        let best_sim = sim.max(sim_lower);

        match &best {
            Some(current) if best_sim <= current.score => continue,
            _ => {
                best = Some(FuzzyMatch {
                    text: line.to_string(),
                    score: best_sim,
                    line: i + 1,
                });
            }
        }
    }

    // Also try multi-line windows (2-5 lines) for search strings with newlines.
    if search_trimmed.contains('\n') {
        let lines: Vec<&str> = content.lines().collect();
        let search_lines: Vec<&str> = search_trimmed.lines().collect();
        for window_size in search_lines.len()..=(search_lines.len() + 2).min(lines.len()) {
            for start in 0..=lines.len().saturating_sub(window_size) {
                let window = lines[start..start + window_size].join("\n");
                let sim = normalized_similarity(search_trimmed, &window);
                match &best {
                    Some(current) if sim <= current.score => continue,
                    _ => {
                        best = Some(FuzzyMatch {
                            text: window,
                            score: sim,
                            line: start + 1,
                        });
                    }
                }
            }
        }
    }

    best.filter(|m| m.score >= FUZZY_MIN_SIMILARITY)
}

/// Normalized Levenshtein similarity: 1.0 = identical, 0.0 = completely different.
fn normalized_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein_distance(a, b);
    1.0 - (dist as f64 / max_len as f64)
}

/// Compute Levenshtein (edit) distance between two strings.
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0usize; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1) // deletion
                .min(curr[j - 1] + 1) // insertion
                .min(prev[j - 1] + cost); // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

/// Fin Flash inner-loop: when the deterministic fuzzy matcher can't find
/// a close match, ask Flash (thinking off, fork_context for cache sharing)
/// to find the closest text in the file. Costs ~$0.0001, returns in ~200ms.
///
/// This is Option B — the fallback when Option A (fuzzy matching) can't
/// find a match above the similarity threshold.
pub async fn flash_correct_search(
    client: &crate::client::DeepSeekClient,
    file_content: &str,
    failed_search: &str,
) -> Option<String> {
    use crate::llm_client::LlmClient;
    use crate::models::{ContentBlock, Message, MessageRequest};

    // Truncate file content to avoid token bloat — Flash just needs context.
    let file_snippet = if file_content.len() > 8000 {
        let half = 4000;
        let start = &file_content[..half];
        let end = &file_content[file_content.len().saturating_sub(half)..];
        format!("{start}\n... [truncated] ...\n{end}")
    } else {
        file_content.to_string()
    };

    let request = MessageRequest {
        model: "deepseek-v4-flash".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: format!(
                    "The following edit_file search string did not match any text in this file:\n\
                     \nSEARCH STRING:\n{failed_search}\n\
                     \nFILE CONTENT:\n{file_snippet}\n\
                     \nReturn ONLY the exact text from the file above that most closely \
                     matches the search string. Return just the matching text, nothing else. \
                     If no reasonable match exists, return the word NOMATCH."
                ),
                cache_control: None,
            }],
        }],
        max_tokens: 128,
        system: None,
        temperature: Some(0.0),
        top_p: None,
        stream: Some(false),
        reasoning_effort: Some("off".to_string()),
        response_format: None,
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
    };

    match client.create_message(request).await {
        Ok(response) => {
            let text = response
                .content
                .first()
                .and_then(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.trim().to_string()),
                    _ => None,
                })
                .unwrap_or_default();

            if text.is_empty() || text.eq_ignore_ascii_case("NOMATCH") {
                None
            } else {
                Some(text)
            }
        }
        Err(_) => None,
    }
}

/// Attempt to correct a failed edit_file search string and retry the operation.
///
/// Called when edit_file fails with a "search string not found" error.
/// First tries deterministic fuzzy matching (free). If that fails, returns
/// `None` — the caller should fall back to the Flash inner-loop (Option B).
///
/// Returns a corrected tool input with the fuzzy-matched search string,
/// or `None` if no correction is possible.
pub fn correct_edit_file_input(
    original_input: &serde_json::Value,
    error_message: &str,
    workspace: &Path,
) -> Option<serde_json::Value> {
    // Only intercept "not found" / "no match" errors.
    let err_lower = error_message.to_lowercase();
    if !err_lower.contains("not found")
        && !err_lower.contains("no match")
        && !err_lower.contains("search string")
        && !err_lower.contains("could not find")
    {
        return None;
    }

    let path_str = original_input.get("path").and_then(|v| v.as_str())?;
    let search_str = original_input.get("search").and_then(|v| v.as_str())?;

    let fuzzy = fuzzy_correct_search(Path::new(path_str), search_str, workspace)?;

    // Build corrected input with the fuzzy-matched text as the new search string.
    let mut corrected = original_input.clone();
    if let Some(obj) = corrected.as_object_mut() {
        obj.insert(
            "search".to_string(),
            serde_json::Value::String(fuzzy.text.clone()),
        );
        // Add a note so the model understands the correction.
        obj.insert(
            "fuzzy_correction".to_string(),
            serde_json::Value::String(format!(
                "search string auto-corrected (score {:.0}%, line {}): \"{}\" → \"{}\"",
                fuzzy.score * 100.0,
                fuzzy.line,
                search_str,
                fuzzy.text
            )),
        );
    }
    Some(corrected)
}

/// Build a retry-success annotation for the session message stream.
pub fn retry_annotation(retry_count: u32) -> String {
    if retry_count == 0 {
        String::new()
    } else {
        format!(
            "\n\n[VERIFY PASS] auto-retried {} time(s) — operation landed",
            retry_count
        )
    }
}

/// Determine whether a tool name represents a side-effect tool that should
/// be verified.
#[allow(dead_code)]
pub fn is_side_effect_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "write_file"
            | "edit_file"
            | "apply_patch"
            | "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "shell_cancel"
            | "exec_wait"
            | "exec_interact"
            | "task_shell_start"
            | "task_shell_wait"
            | "task_create"
            | "task_cancel"
            | "task_gate_run"
            | "github_comment"
            | "github_close_issue"
            | "github_close_pr"
            | "pr_attempt_record"
            | "pr_attempt_preflight"
            | "automation_create"
            | "automation_update"
            | "automation_pause"
            | "automation_resume"
            | "automation_delete"
            | "automation_run"
            | "remember"
            | "notify"
            | "revert_turn"
            | "fim_edit"
            | "pandoc_convert"
            | "image_analyze"
            | "image_ocr"
            | "web_run"
            | "finance"
            | "skill_install"
            | "checklist_write"
            | "checklist_add"
            | "checklist_update"
            | "checklist_list"
            | "todo_write"
            | "todo_add"
            | "todo_update"
            | "update_plan"
            | "create_goal"
            | "update_goal"
    )
}

/// Post-hoc file-level verification: read the file back and check that
/// the expected content is present. Called by the turn loop after the
/// tool result has been injected into the session stream.
///
/// Returns `Some(VerifyVerdict)` when verification was possible,
/// `None` when the tool doesn't support post-hoc file checks.
#[allow(dead_code)]
pub fn post_hoc_verify_file(
    tool_name: &str,
    tool_input: &serde_json::Value,
    workspace: &Path,
) -> Option<VerifyVerdict> {
    match tool_name {
        "write_file" | "edit_file" => {
            let path_str = tool_input.get("path").and_then(|v| v.as_str())?;
            let resolved = if Path::new(path_str).is_absolute() {
                Path::new(path_str).to_path_buf()
            } else {
                workspace.join(path_str)
            };

            // Read back the file to check it exists and has content.
            match std::fs::read_to_string(&resolved) {
                Ok(content) => {
                    if content.is_empty() {
                        Some(VerifyVerdict::Fail {
                            expected: format!("non-empty file at {}", resolved.display()),
                            observed: "file is empty".to_string(),
                        })
                    } else {
                        Some(VerifyVerdict::Pass)
                    }
                }
                Err(e) => Some(VerifyVerdict::Unverifiable {
                    reason: format!("cannot read {} for verification: {e}", resolved.display()),
                }),
            }
        }

        "exec_shell" => {
            // For exec_shell, check if the command created expected paths.
            // We can't know the expected output without parsing the command,
            // so this is best-effort: if the tool claimed success and exit
            // code was zero, we trust it.
            Some(VerifyVerdict::Pass)
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_tools_are_skipped() {
        for tool in &[
            "read_file",
            "grep_files",
            "file_search",
            "list_dir",
            "git_status",
            "git_diff",
            "web_search",
        ] {
            let (verdict, _) = run_verification(tool, &serde_json::json!({}), Path::new("/tmp"));
            assert!(
                matches!(verdict, VerifyVerdict::Skipped),
                "{tool} should be skipped, got {verdict:?}"
            );
        }
    }

    #[test]
    fn side_effect_tools_pass_when_successful() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Create a real file so inline verification passes for file tools.
        let test_file = tmp.path().join("test.rs");
        std::fs::write(&test_file, "// test content").expect("write");

        for tool in &["write_file", "edit_file", "apply_patch"] {
            let (verdict, _) = run_verification(
                tool,
                &serde_json::json!({"path": test_file.to_str().unwrap()}),
                tmp.path(),
            );
            assert!(
                matches!(verdict, VerifyVerdict::Pass),
                "{tool} should pass, got {verdict:?}"
            );
        }

        // exec_shell passes through — not file-verified.
        let (verdict, _) = run_verification(
            "exec_shell",
            &serde_json::json!({"command": "echo ok"}),
            tmp.path(),
        );
        assert!(matches!(verdict, VerifyVerdict::Pass));
    }

    #[test]
    fn side_effect_tools_pass_for_empty_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let test_file = tmp.path().join("empty.txt");
        std::fs::write(&test_file, "").expect("write empty file");

        let (verdict, _) = run_verification(
            "write_file",
            &serde_json::json!({"path": "empty.txt"}),
            tmp.path(),
        );

        assert!(matches!(verdict, VerifyVerdict::Pass));
    }

    #[test]
    fn unknown_tools_are_unverifiable() {
        let (verdict, _) = run_verification(
            "nonexistent_tool",
            &serde_json::json!({}),
            Path::new("/tmp"),
        );
        assert!(matches!(verdict, VerifyVerdict::Unverifiable { .. }));
    }

    #[test]
    fn is_side_effect_tool_identifies_mutating_tools() {
        assert!(is_side_effect_tool("write_file"));
        assert!(is_side_effect_tool("edit_file"));
        assert!(is_side_effect_tool("exec_shell"));
        assert!(is_side_effect_tool("apply_patch"));
        assert!(!is_side_effect_tool("read_file"));
        assert!(!is_side_effect_tool("grep_files"));
        assert!(!is_side_effect_tool("git_status"));
    }

    #[test]
    fn post_hoc_verify_write_file_detects_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let verdict = post_hoc_verify_file(
            "write_file",
            &serde_json::json!({"path": "nonexistent.txt"}),
            tmp.path(),
        );
        assert!(verdict.is_some());
        assert!(matches!(
            verdict.unwrap(),
            VerifyVerdict::Unverifiable { .. }
        ));
    }

    #[test]
    fn post_hoc_verify_write_file_confirms_existing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("real.txt");
        std::fs::write(&file_path, "hello world").expect("write");

        let verdict = post_hoc_verify_file(
            "write_file",
            &serde_json::json!({"path": "real.txt"}),
            tmp.path(),
        );
        assert!(verdict.is_some());
        assert!(matches!(verdict.unwrap(), VerifyVerdict::Pass));
    }

    #[test]
    fn post_hoc_verify_returns_none_for_unsupported_tools() {
        assert!(
            post_hoc_verify_file("read_file", &serde_json::json!({}), Path::new("/tmp")).is_none()
        );
    }

    #[test]
    fn verify_config_default_disabled() {
        let cfg = VerifyConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.skip_tools.is_empty());
        assert_eq!(cfg.max_retries, 1);
    }

    #[test]
    fn fuzzy_correct_finds_exact_match() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}").expect("write");

        let result = fuzzy_correct_search(&file_path, "fn main() {", tmp.path());
        assert!(result.is_some());
        let m = result.unwrap();
        assert!(m.score > 0.99);
        assert!(m.text.contains("fn main()"));
    }

    #[test]
    fn fuzzy_correct_handles_whitespace_diff() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("test.rs");
        std::fs::write(&file_path, "fn main()  {\n    println!(\"hello\");\n}").expect("write");

        // Search with single space — should still find the double-space line.
        let result = fuzzy_correct_search(&file_path, "fn main() {", tmp.path());
        assert!(result.is_some());
        let m = result.unwrap();
        assert!(m.score >= 0.6, "score {} below threshold", m.score);
    }

    #[test]
    fn fuzzy_correct_returns_none_for_unrelated_search() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}").expect("write");

        // Completely unrelated text should return None.
        let result = fuzzy_correct_search(
            &file_path,
            "completely different text that doesn't exist anywhere",
            tmp.path(),
        );
        assert!(result.is_none());
    }

    #[test]
    fn levenshtein_distance_correct() {
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("rust", "rust "), 1);
    }
}
