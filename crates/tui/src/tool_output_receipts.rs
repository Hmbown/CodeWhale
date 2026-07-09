//! Compact receipts for oversized tool outputs in saved session history.

use serde_json::Value;

use crate::artifacts::{ArtifactKind, ArtifactRecord, format_artifact_relative_path};
use crate::fast_hash::FastHashMap;
use crate::models::{ContentBlock, Message};
use crate::tools::truncate;

/// Match the provider-wire budget so persisted/resumed history does not keep a
/// larger raw body than the model would receive on a fresh request.
pub const RAW_TOOL_OUTPUT_RECEIPT_THRESHOLD_CHARS: usize = 12_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutputReceiptStats {
    pub compacted_count: usize,
    pub artifact_receipts: usize,
    pub sha_receipts: usize,
    pub unavailable_receipts: usize,
    pub original_chars: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutputStatus {
    pub raw_large_count: usize,
    pub raw_large_chars: usize,
    pub receipt_count: usize,
    pub artifact_count: usize,
    pub artifact_bytes: u64,
}

#[derive(Debug, Clone)]
struct ToolUseInfo {
    name: String,
    input: Value,
}

#[derive(Debug, Clone)]
enum DetailHandle {
    Artifact(ArtifactRecord),
    Sha { sha: String, persisted: bool },
}

/// Return a copy of `messages` with oversized raw tool-result bodies replaced
/// by compact receipts. Full output is kept behind existing session artifacts
/// when available; otherwise a SHA-addressed spillover copy is written for
/// `retrieve_tool_result`.
pub fn compact_messages_for_persistence(
    messages: &[Message],
    artifacts: &[ArtifactRecord],
) -> (Vec<Message>, ToolOutputReceiptStats) {
    // Tool-call IDs here come from engine transcript blocks and artifact records,
    // making this save/resume bookkeeping a safe FastHashMap target.
    let artifacts_by_call = artifacts_by_tool_call(artifacts);
    let mut tool_uses: FastHashMap<String, ToolUseInfo> = FastHashMap::default();
    let mut stats = ToolOutputReceiptStats::default();
    let mut compacted = Vec::with_capacity(messages.len());

    for message in messages {
        let mut next = message.clone();
        for block in &mut next.content {
            match block {
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    tool_uses.insert(
                        id.clone(),
                        ToolUseInfo {
                            name: name.clone(),
                            input: input.clone(),
                        },
                    );
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } => {
                    let char_count = content.chars().count();
                    if char_count <= RAW_TOOL_OUTPUT_RECEIPT_THRESHOLD_CHARS
                        || looks_like_receipt(content)
                    {
                        continue;
                    }

                    let tool_info = tool_uses.get(tool_use_id);
                    let handle = artifacts_by_call
                        .get(tool_use_id.as_str())
                        .cloned()
                        .map(|artifact| DetailHandle::Artifact((*artifact).clone()))
                        .unwrap_or_else(|| DetailHandle::Sha {
                            sha: sha256_hex(content.as_bytes()),
                            persisted: persist_sha_tool_result(content),
                        });
                    let source = match &handle {
                        DetailHandle::Artifact(_) => ReceiptSource::Artifact,
                        DetailHandle::Sha {
                            persisted: true, ..
                        } => ReceiptSource::Sha,
                        DetailHandle::Sha {
                            persisted: false, ..
                        } => ReceiptSource::Unavailable,
                    };

                    *content = render_tool_output_receipt(
                        tool_use_id,
                        tool_info,
                        content,
                        *is_error,
                        &handle,
                    );
                    stats.compacted_count += 1;
                    stats.original_chars = stats.original_chars.saturating_add(char_count);
                    match source {
                        ReceiptSource::Artifact => stats.artifact_receipts += 1,
                        ReceiptSource::Sha => stats.sha_receipts += 1,
                        ReceiptSource::Unavailable => stats.unavailable_receipts += 1,
                    }
                }
                _ => {}
            }
        }
        compacted.push(next);
    }

    (compacted, stats)
}

pub fn tool_output_status(messages: &[Message], artifacts: &[ArtifactRecord]) -> ToolOutputStatus {
    let mut status = ToolOutputStatus {
        artifact_count: artifacts.len(),
        artifact_bytes: artifacts
            .iter()
            .map(|artifact| artifact.byte_size)
            .sum::<u64>(),
        ..ToolOutputStatus::default()
    };

    for message in messages {
        for block in &message.content {
            if let ContentBlock::ToolResult { content, .. } = block {
                if looks_like_receipt(content) {
                    status.receipt_count += 1;
                } else {
                    let chars = content.chars().count();
                    if chars > RAW_TOOL_OUTPUT_RECEIPT_THRESHOLD_CHARS {
                        status.raw_large_count += 1;
                        status.raw_large_chars = status.raw_large_chars.saturating_add(chars);
                    }
                }
            }
        }
    }

    status
}

pub fn format_tool_output_status(status: &ToolOutputStatus) -> String {
    let mut parts = Vec::new();
    if status.raw_large_count > 0 {
        parts.push(format!(
            "{} raw over cap (~{} chars) adding context pressure",
            status.raw_large_count,
            format_count(status.raw_large_chars)
        ));
    }
    if status.receipt_count > 0 {
        parts.push(format!("{} compact receipt(s)", status.receipt_count));
    }
    if status.artifact_count > 0 {
        parts.push(format!(
            "{} artifact(s), {} stored",
            status.artifact_count,
            crate::artifacts::format_byte_size(status.artifact_bytes)
        ));
    }
    if parts.is_empty() {
        "no large outputs tracked".to_string()
    } else {
        parts.join("; ")
    }
}

fn artifacts_by_tool_call(artifacts: &[ArtifactRecord]) -> FastHashMap<&str, &ArtifactRecord> {
    artifacts
        .iter()
        .filter(|artifact| artifact.kind == ArtifactKind::ToolOutput)
        .map(|artifact| (artifact.tool_call_id.as_str(), artifact))
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum ReceiptSource {
    Artifact,
    Sha,
    Unavailable,
}

fn render_tool_output_receipt(
    tool_call_id: &str,
    tool_info: Option<&ToolUseInfo>,
    original_content: &str,
    is_error: Option<bool>,
    handle: &DetailHandle,
) -> String {
    let original_chars = original_content.chars().count();
    let original_bytes = original_content.len() as u64;
    let tool_name = match handle {
        DetailHandle::Artifact(record) if !record.tool_name.trim().is_empty() => {
            record.tool_name.as_str()
        }
        _ => tool_info
            .map(|info| info.name.as_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("unknown"),
    };
    let command_or_query = tool_info
        .map(|info| summarize_input(&info.input, 300))
        .unwrap_or_else(|| "unknown".to_string());
    let status = if is_error.unwrap_or(false) {
        "error"
    } else {
        "success"
    };
    let exit_status = infer_exit_status(original_content).unwrap_or_else(|| "unknown".to_string());
    let preview = preview_for_receipt(handle, original_content);
    let (detail_handle, retrieve, storage) = match handle {
        DetailHandle::Artifact(record) => (
            record.id.clone(),
            format!("retrieve_tool_result ref={}", record.id),
            format_artifact_relative_path(&record.storage_path),
        ),
        DetailHandle::Sha { sha, persisted } => {
            let handle = format!("sha:{sha}");
            let storage = if *persisted {
                "content-addressed spillover".to_string()
            } else {
                "unavailable; spillover write failed".to_string()
            };
            (
                handle.clone(),
                format!("retrieve_tool_result ref={handle}"),
                storage,
            )
        }
    };

    format!(
        "[TOOL_OUTPUT_RECEIPT]\n\
         tool: {tool_name}\n\
         tool_call_id: {tool_call_id}\n\
         status: {status}\n\
         exit_status: {exit_status}\n\
         elapsed: unknown\n\
         output: {bytes} ({chars} chars, ~{tokens} tokens)\n\
         truncation: raw output omitted from saved/resumed context\n\
         detail_handle: {detail_handle}\n\
         retrieve: {retrieve}\n\
         storage: {storage}\n\
         command_or_query: {command_or_query}\n\
         preview: {preview}\n\
         [/TOOL_OUTPUT_RECEIPT]",
        bytes = crate::artifacts::format_byte_size(original_bytes),
        chars = format_count(original_chars),
        tokens = format_count(approx_tokens(original_chars)),
    )
}

fn persist_sha_tool_result(content: &str) -> bool {
    let sha = sha256_hex(content.as_bytes());
    match truncate::write_sha_spillover(&sha, content) {
        Ok(_) => true,
        Err(err) => {
            crate::logging::warn(format!(
                "tool-output receipt SHA spillover write failed for sha={sha}: {err}"
            ));
            false
        }
    }
}

fn preview_for_receipt(handle: &DetailHandle, original_content: &str) -> String {
    let preview = match handle {
        DetailHandle::Artifact(record) if !record.preview.trim().is_empty() => {
            record.preview.as_str()
        }
        _ => original_content,
    };
    summarize_text(preview, 240)
}

fn looks_like_receipt(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("[TOOL_OUTPUT_RECEIPT]")
        || trimmed.starts_with("[artifact:")
        || trimmed.starts_with("[TOOL_RESULT_TRUNCATED]")
        || trimmed.starts_with("<TOOL_RESULT_REF")
}

fn infer_exit_status(content: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        for key in ["exit_code", "exit_status", "status", "code"] {
            if let Some(value) = value.get(key) {
                return Some(summarize_input(value, 120));
            }
        }
    }

    for line in content.lines().take(40) {
        let trimmed = line.trim();
        for prefix in ["Exit code:", "exit code:", "Exit status:", "exit status:"] {
            if let Some(value) = trimmed.strip_prefix(prefix) {
                return Some(summarize_text(value.trim(), 120));
            }
        }
    }
    None
}

fn summarize_input(value: &Value, max_chars: usize) -> String {
    let raw = value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    summarize_text(&raw, max_chars)
}

fn summarize_text(text: &str, max_chars: usize) -> String {
    let escaped = text.replace('\n', "\\n");
    let mut summary: String = escaped.chars().take(max_chars).collect();
    if escaped.chars().count() > max_chars {
        summary.push_str("...");
    }
    summary
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::hashing::sha256_hex(bytes)
}

fn approx_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

fn format_count(value: usize) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::Utc;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn tool_use_message(id: &str, name: &str, input: Value) -> Message {
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
                caller: None,
            }],
        }
    }

    fn tool_result_message(id: &str, content: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    fn artifact_record(tool_call_id: &str, raw: &str) -> ArtifactRecord {
        ArtifactRecord {
            id: crate::artifacts::artifact_id_for_tool_call(tool_call_id),
            kind: ArtifactKind::ToolOutput,
            session_id: "session-123".to_string(),
            tool_call_id: tool_call_id.to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: raw.len() as u64,
            preview: "checking crate ... error[E0425]".to_string(),
            storage_path: PathBuf::from("artifacts").join("art_call-big.txt"),
        }
    }

    #[test]
    fn compacts_large_tool_result_to_artifact_receipt() {
        let raw = "RAW_SENTINEL\n".repeat(2_000);
        let messages = vec![
            tool_use_message(
                "call-big",
                "exec_shell",
                json!({"command": "cargo test -p codewhale-tui"}),
            ),
            tool_result_message("call-big", &raw),
        ];
        let artifacts = vec![artifact_record("call-big", &raw)];

        let (compacted, stats) = compact_messages_for_persistence(&messages, &artifacts);
        let ContentBlock::ToolResult { content, .. } = &compacted[1].content[0] else {
            panic!("expected tool result");
        };

        assert_eq!(stats.compacted_count, 1);
        assert_eq!(stats.artifact_receipts, 1);
        assert!(!content.contains("RAW_SENTINEL"));
        assert!(content.contains("[TOOL_OUTPUT_RECEIPT]"));
        assert!(content.contains("tool: exec_shell"));
        assert!(content.contains("detail_handle: art_call-big"));
        assert!(content.contains("retrieve: retrieve_tool_result ref=art_call-big"));
        assert!(
            content.contains("command_or_query: {\"command\":\"cargo test -p codewhale-tui\"}")
        );
    }

    #[test]
    fn compacts_large_tool_result_to_sha_receipt_when_no_artifact_exists() {
        let _guard = crate::tools::truncate::TEST_SPILLOVER_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempdir().expect("tempdir");
        let prior = crate::tools::truncate::set_test_spillover_root(Some(
            tmp.path().join(".deepseek").join("tool_outputs"),
        ));
        struct Restore(Option<PathBuf>);
        impl Drop for Restore {
            fn drop(&mut self) {
                crate::tools::truncate::set_test_spillover_root(self.0.take());
            }
        }
        let _restore = Restore(prior);

        let raw = format!("{}\n{}", "H".repeat(320), "NO_ARTIFACT_RAW\n".repeat(2_000));
        let sha = sha256_hex(raw.as_bytes());
        let messages = vec![
            tool_use_message("call-big", "grep_files", json!({"pattern": "TODO"})),
            tool_result_message("call-big", &raw),
        ];

        let (compacted, stats) = compact_messages_for_persistence(&messages, &[]);
        let ContentBlock::ToolResult { content, .. } = &compacted[1].content[0] else {
            panic!("expected tool result");
        };

        assert_eq!(stats.compacted_count, 1);
        assert_eq!(stats.sha_receipts, 1);
        assert!(!content.contains("NO_ARTIFACT_RAW"));
        assert!(content.contains(&format!("detail_handle: sha:{sha}")));
        assert!(content.contains(&format!("retrieve: retrieve_tool_result ref=sha:{sha}")));
        let path = crate::tools::truncate::sha_spillover_path(&sha).expect("sha path");
        assert_eq!(std::fs::read_to_string(path).expect("read sha"), raw);
    }

    #[test]
    fn small_tool_results_remain_inline() {
        let messages = vec![
            tool_use_message("call-small", "exec_shell", json!({"command": "pwd"})),
            tool_result_message("call-small", "ok"),
        ];

        let (compacted, stats) = compact_messages_for_persistence(&messages, &[]);
        let ContentBlock::ToolResult { content, .. } = &compacted[1].content[0] else {
            panic!("expected tool result");
        };

        assert_eq!(content, "ok");
        assert_eq!(stats.compacted_count, 0);
    }

    #[test]
    fn status_reports_raw_large_receipts_and_artifacts() {
        let raw = "RAW_STATUS\n".repeat(2_000);
        let receipt = "[TOOL_OUTPUT_RECEIPT]\ndetail_handle: art_call-big";
        let messages = vec![
            tool_result_message("call-raw", &raw),
            tool_result_message("call-receipt", receipt),
        ];
        let artifacts = vec![ArtifactRecord {
            storage_path: Path::new("artifacts/art_call-big.txt").to_path_buf(),
            ..artifact_record("call-big", &raw)
        }];

        let status = tool_output_status(&messages, &artifacts);
        assert_eq!(status.raw_large_count, 1);
        assert_eq!(status.receipt_count, 1);
        assert_eq!(status.artifact_count, 1);

        let rendered = format_tool_output_status(&status);
        assert!(rendered.contains("raw over cap"));
        assert!(rendered.contains("compact receipt"));
        assert!(rendered.contains("artifact"));
    }
}
