use super::*;

fn report(error: &anyhow::Error) -> String {
    report_compaction_failure("Auto-compaction failed", "compact_fixture", true, error)
}

#[test]
fn strip_compaction_summaries_removes_only_summary_blocks() {
    let base = SystemBlock {
        block_type: "text".to_string(),
        text: "stable base prompt".to_string(),
        cache_control: None,
    };
    let summary = SystemBlock {
        block_type: "text".to_string(),
        text: format!("{COMPACTION_SUMMARY_MARKER} and its body"),
        cache_control: None,
    };
    let legacy = SystemBlock {
        block_type: "text".to_string(),
        text: format!("{LEGACY_COMPACTION_SUMMARY_MARKER}\nold-format body"),
        cache_control: None,
    };

    let stripped = strip_compaction_summaries(Some(&SystemPrompt::Blocks(vec![
        base.clone(),
        summary.clone(),
        legacy,
    ])))
    .expect("base block survives");
    match stripped {
        SystemPrompt::Blocks(blocks) => {
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0].text, "stable base prompt");
        }
        SystemPrompt::Text(_) => panic!("blocks stay blocks"),
    }

    // A prompt that is nothing but a summary strips to None.
    assert!(strip_compaction_summaries(Some(&SystemPrompt::Text(summary.text))).is_none());
    // A prompt without summaries is unchanged.
    assert_eq!(
        strip_compaction_summaries(Some(&SystemPrompt::Text("plain".to_string()))),
        Some(SystemPrompt::Text("plain".to_string()))
    );
}

#[test]
fn persisted_summary_carrier_round_trips_without_losing_the_base_prompt() {
    let carrier = format!(
        "stable base prompt\n\n{COMPACTION_SUMMARY_BEGIN}\n{COMPACTION_SUMMARY_MARKER}\nnew summary\n{COMPACTION_SUMMARY_END}"
    );

    assert_eq!(
        extract_compaction_summary(Some(&SystemPrompt::Text(carrier.clone()))),
        Some(SystemPrompt::Text(format!(
            "{COMPACTION_SUMMARY_MARKER}\nnew summary"
        )))
    );
    assert_eq!(
        strip_compaction_summaries(Some(&SystemPrompt::Text(carrier))),
        Some(SystemPrompt::Text("stable base prompt".to_string()))
    );
}

#[test]
fn combined_block_carrier_preserves_block_metadata_and_base_text() {
    let carrier = SystemBlock {
        block_type: "text".to_string(),
        text: format!(
            "stable block\n\n{COMPACTION_SUMMARY_BEGIN}\n{COMPACTION_SUMMARY_MARKER}\nblock summary\n{COMPACTION_SUMMARY_END}"
        ),
        cache_control: Some(CacheControl {
            cache_type: "ephemeral".to_string(),
        }),
    };

    let extracted = extract_compaction_summary(Some(&SystemPrompt::Blocks(vec![carrier.clone()])))
        .expect("checkpoint");
    let SystemPrompt::Blocks(extracted) = extracted else {
        panic!("blocks stay blocks");
    };
    assert_eq!(extracted.len(), 1);
    assert_eq!(
        extracted[0].text,
        format!("{COMPACTION_SUMMARY_MARKER}\nblock summary")
    );
    assert_eq!(extracted[0].cache_control, carrier.cache_control);

    let stripped =
        strip_compaction_summaries(Some(&SystemPrompt::Blocks(vec![carrier]))).expect("base block");
    let SystemPrompt::Blocks(stripped) = stripped else {
        panic!("blocks stay blocks");
    };
    assert_eq!(stripped.len(), 1);
    assert_eq!(stripped[0].text, "stable block");
    assert_eq!(
        stripped[0]
            .cache_control
            .as_ref()
            .map(|c| c.cache_type.as_str()),
        Some("ephemeral")
    );
}

#[test]
fn untyped_usage_limit_text_never_becomes_quota_exhaustion() {
    let error = anyhow::anyhow!(
        "[auth] Authorization failed: You've reached your usage limit for this billing cycle"
    );
    let message = report(&error);
    assert!(message.contains("provider rate limit blocked compaction"));
    assert!(!message.contains("quota exhausted"));
}

#[test]
fn typed_quota_renders_quota_and_is_not_transient() {
    let error = anyhow::Error::new(crate::llm_client::LlmError::from_http_response(
        429,
        r#"{"error":{"code":"insufficient_quota"}}"#,
    ))
    .context("summary request failed");
    assert_eq!(
        report(&error),
        "Auto-compaction failed: provider plan quota exhausted — switch provider/model or renew the provider plan"
    );
    assert!(!is_transient_error(&error));
}

#[test]
fn typed_rate_limit_stays_transient_and_does_not_become_quota() {
    let error = anyhow::Error::new(crate::llm_client::LlmError::RateLimited {
        message: "Too Many Requests".into(),
        retry_after: None,
    });
    assert!(report(&error).contains("provider rate limit blocked compaction"));
    assert!(is_transient_error(&error));
}

#[test]
fn unknown_diagnostic_is_preserved_safely() {
    let error = anyhow::anyhow!("summary response was structurally empty");
    assert_eq!(
        report(&error),
        "Auto-compaction failed: summary response was structurally empty"
    );
}

#[test]
fn untyped_transient_and_deterministic_classification_remains_compatible() {
    for message in [
        "Connection timeout",
        "429 Too Many Requests",
        "503 Service Unavailable",
        "network error: connection refused",
    ] {
        assert!(is_transient_error(&anyhow::anyhow!(message)), "{message}");
    }
    for message in [
        "401 Unauthorized: Invalid API key",
        "Failed to parse JSON response",
        "Invalid request: missing required field",
    ] {
        assert!(!is_transient_error(&anyhow::anyhow!(message)), "{message}");
    }
    assert_eq!(
        classify_compaction_failure(&anyhow::anyhow!(
            "prompt is too long for this model's context window"
        )),
        CompactionFailureKind::ContextOverflow
    );
}

fn pressure_fixture() -> Vec<Message> {
    (0..30)
        .map(|index| Message {
            role: if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            content: vec![ContentBlock::Text {
                text: "x".repeat(8_000),
                cache_control: None,
            }],
        })
        .collect()
}

fn oversized_tool_pair(id: &str, content: String) -> Vec<Message> {
    vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "src/compaction.rs"}),
                caller: None,
                thought_signature: None,
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content,
                is_error: None,
                content_blocks: None,
            }],
        },
    ]
}

#[test]
fn pinned_tool_result_local_pruning_is_reclaimable() {
    let mut messages =
        oversized_tool_pair("old-read", "error: ".to_string() + &"x".repeat(300_000));
    messages.extend(pressure_fixture());
    let full_pressure = estimate_input_tokens_for_pressure(&messages, None);
    let mut projected = messages.clone();
    let pruned_bytes = prune_tool_results_until(&mut projected, KEEP_RECENT_MESSAGES, |_, _| false);
    let projected_pressure = estimate_input_tokens_for_pressure(&projected, None);
    assert!(
        pruned_bytes > 250_000,
        "fixture must prune the pinned result"
    );
    assert!(projected_pressure < full_pressure);

    let config = CompactionConfig {
        token_threshold: projected_pressure + (full_pressure - projected_pressure) / 2,
        ..Default::default()
    };
    assert!(compaction_pressure_reached(&messages, None, &config));
    assert!(!compaction_pressure_reached(&projected, None, &config));
    assert!(should_compact(
        &messages,
        None,
        &PreparedCompactionEnvelope::new(config),
    ));
}

#[test]
fn local_pruning_removes_nested_tool_result_images() {
    let mut messages = oversized_tool_pair("image-read", "screenshot captured".to_string());
    let ContentBlock::ToolResult { content_blocks, .. } = &mut messages[1].content[0] else {
        panic!("tool result fixture");
    };
    *content_blocks = Some(vec![serde_json::json!({
        "type": "image",
        "mime_type": "image/png",
        "data": "A".repeat(300_000),
    })]);
    messages.extend(pressure_fixture());

    let before = estimate_input_tokens_for_pressure(&messages, None);
    let pruned = prune_tool_results_until(&mut messages, KEEP_RECENT_MESSAGES, |_, _| false);
    let after = estimate_input_tokens_for_pressure(&messages, None);
    let ContentBlock::ToolResult { content_blocks, .. } = &messages[1].content[0] else {
        panic!("tool result fixture");
    };

    assert!(pruned > 250_000, "nested image bytes must be reclaimable");
    assert!(after < before);
    assert!(content_blocks.is_none(), "base64 must not survive pruning");
}

#[test]
fn successor_floor_counts_retained_user_messages_not_tool_results() {
    let mut messages = pressure_fixture();
    messages.extend(oversized_tool_pair(
        "recent-read",
        "z".repeat(RETAINED_TOOL_RESULT_MAX_CHARS * 4),
    ));
    let base_config = CompactionConfig::default();
    let prepared = PreparedCompactionEnvelope::new(base_config.clone());
    let retained_floor = estimate_retained_floor_conservative(&messages, None, &prepared);
    let full_pressure = estimate_input_tokens_conservative(&messages, None);
    assert!(
        retained_floor < full_pressure,
        "user-only retention must reclaim the giant tool result"
    );

    let config = CompactionConfig {
        token_threshold: retained_floor + 1,
        ..base_config
    };
    assert!(compaction_pressure_reached(&messages, None, &config));
    assert!(should_compact(
        &messages,
        None,
        &PreparedCompactionEnvelope::new(config),
    ));
}
