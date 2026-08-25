//! Observer-hook projection: subagent and turn-end hook payload construction,
//! preview bounding, and completion classification (TUI_MODULARIZATION.md
//! slice 4). The executor lives in `crate::hooks`; this module builds the
//! payloads the UI submits and classifies completion results for display.

use super::*;

pub(super) fn execute_subagent_observer_hook(
    app: &App,
    event: HookEvent,
    agent_id: &str,
    text_field: &str,
    text: &str,
) -> Result<(), String> {
    let (preview, truncated) = bounded_subagent_hook_preview(text);

    // Lifecycle outbox (`[lifecycle_outbox]`): fires even when no shell hook
    // is configured for this event — the outbox is independent of the hook
    // command list. Preview is bounded (preview ceiling) and only ever the
    // preview text, never the raw prompt/result. No-op when disabled.
    match &event {
        HookEvent::SubagentSpawn => {
            app.lifecycle_outbox.emit(codewhale_hooks::LifecycleEvent {
                event: "subagent_spawn".to_string(),
                kind: "subagent.spawned".to_string(),
                thread_id: app.hooks.session_id().to_string(),
                turn_id: app.runtime_turn_id.clone(),
                item_id: None,
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "subagent": agent_id,
                    "workspace": app.workspace.display().to_string(),
                    "prompt_preview": codewhale_hooks::bounded_text(
                        &preview,
                        codewhale_hooks::OUTBOX_PREVIEW_MAX_CHARS,
                    ),
                    "prompt_truncated": truncated,
                }),
            });
        }
        HookEvent::SubagentComplete => {
            let status = subagent_completion_status(text).unwrap_or_else(|| "unknown".to_string());
            app.lifecycle_outbox.emit(codewhale_hooks::LifecycleEvent {
                event: "subagent_complete".to_string(),
                kind: "subagent.completed".to_string(),
                thread_id: app.hooks.session_id().to_string(),
                turn_id: app.runtime_turn_id.clone(),
                item_id: None,
                payload: serde_json::json!({
                    "agent_id": agent_id,
                    "subagent": agent_id,
                    "workspace": app.workspace.display().to_string(),
                    "status": status,
                    "result_preview": codewhale_hooks::bounded_text(
                        &preview,
                        codewhale_hooks::OUTBOX_PREVIEW_MAX_CHARS,
                    ),
                    "result_truncated": truncated,
                }),
            });
        }
        _ => {}
    }

    if !app.hooks.has_hooks_for_event(event) {
        return Ok(());
    }

    let context = app.base_hook_context().with_message(&preview);
    let mut payload = serde_json::json!({
        "event": event.as_str(),
        "agent_id": agent_id,
        "session_id": context.session_id.as_deref(),
        "workspace": context.workspace.as_ref().map(|path| path.display().to_string()),
        "mode": context.mode.as_deref(),
        "model": context.model.as_deref(),
        "total_tokens": context.total_tokens,
    });
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            format!("{text_field}_preview"),
            serde_json::Value::String(preview),
        );
        object.insert(
            format!("{text_field}_truncated"),
            serde_json::Value::Bool(truncated),
        );
    }

    if event == HookEvent::SubagentComplete {
        payload["status"] = serde_json::Value::String(
            subagent_completion_status(text).unwrap_or_else(|| "unknown".to_string()),
        );
    }

    app.hooks.submit_json_observer(event, context, payload)
}

pub(super) fn execute_turn_end_observer_hook(
    app: &App,
    turn: Option<&ActiveTurnMetadata>,
    usage: &Usage,
    billing_surface: Option<&str>,
    duration: Duration,
    error: Option<&str>,
) -> Result<(), String> {
    if !app.hooks.has_hooks_for_event(HookEvent::TurnEnd) {
        return Ok(());
    }

    let metadata = turn_end_observer_metadata(turn);
    let context = app.base_hook_context();
    let payload = crate::hooks::turn_end_payload(TurnEndPayloadInput {
        context: &context,
        created_at: metadata.created_at,
        model_backed: metadata.route.is_some(),
        provider: metadata.route.map(|route| route.provider_identity.as_str()),
        billing_surface: metadata.route.and(billing_surface),
        model: metadata.route.map(|route| route.model.as_str()),
        turn_id: metadata.turn_id.as_ref(),
        status: app.runtime_turn_status.as_deref().unwrap_or("unknown"),
        error,
        duration,
        usage,
        totals: TurnEndTotals {
            session_tokens: app.session.total_tokens,
            conversation_tokens: app.session.total_conversation_tokens,
            input_tokens: app.session.total_input_tokens,
            output_tokens: app.session.total_output_tokens,
        },
        tool_count: app.tool_evidence.len(),
        queued_message_count: app.queued_message_count(),
    });
    app.hooks
        .submit_json_observer(HookEvent::TurnEnd, context, payload)
}

pub(super) fn surface_observer_hook_submission_failure(app: &mut App, error: String) {
    app.surface_observer_hook_submission_failure(error);
}

pub(super) struct TurnEndObserverMetadata<'a> {
    pub(super) turn_id: std::borrow::Cow<'a, str>,
    pub(super) created_at: chrono::DateTime<chrono::Utc>,
    pub(super) route: Option<&'a crate::core::events::TurnRoute>,
}

pub(super) fn turn_end_observer_metadata(
    turn: Option<&ActiveTurnMetadata>,
) -> TurnEndObserverMetadata<'_> {
    turn.map_or_else(
        || TurnEndObserverMetadata {
            // Manual compaction, purge, and shell-only completions predate the
            // TurnStarted lifecycle event. Preserve their observer contract
            // with a distinct non-model identity instead of borrowing a stale
            // model turn id.
            turn_id: std::borrow::Cow::Owned(format!("lifecycle_{}", uuid::Uuid::new_v4())),
            created_at: chrono::Utc::now(),
            route: None,
        },
        |turn| TurnEndObserverMetadata {
            turn_id: std::borrow::Cow::Borrowed(&turn.turn_id),
            created_at: turn.created_at,
            route: turn.route.as_ref(),
        },
    )
}

pub(super) fn bounded_subagent_hook_preview(text: &str) -> (String, bool) {
    if text.len() <= SUBAGENT_HOOK_PREVIEW_LIMIT {
        return (text.to_string(), false);
    }
    let safe_end = text
        .char_indices()
        .take_while(|(idx, ch)| idx + ch.len_utf8() <= SUBAGENT_HOOK_PREVIEW_LIMIT)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    (format!("{}...[truncated]", &text[..safe_end]), true)
}

pub(super) fn subagent_completion_status(result: &str) -> Option<String> {
    const START: &str = "<codewhale:subagent.done>";
    const END: &str = "</codewhale:subagent.done>";

    if let Some(start) = result.find(START).map(|idx| idx + START.len())
        && let Some(end) = result[start..].find(END).map(|idx| idx + start)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&result[start..end])
        && let Some(status) = value.get("status").and_then(serde_json::Value::as_str)
    {
        return Some(status.to_string());
    }

    let summary = result.lines().find_map(|line| {
        let trimmed = line.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })?;
    let summary = summary.to_ascii_lowercase();
    if matches!(summary.as_str(), "cancelled" | "canceled")
        || summary.starts_with("cancelled:")
        || summary.starts_with("canceled:")
    {
        Some("cancelled".to_string())
    } else if summary == "failed" || summary.starts_with("failed:") {
        Some("failed".to_string())
    } else if summary == "interrupted" || summary.starts_with("interrupted:") {
        Some("interrupted".to_string())
    } else {
        None
    }
}

pub(super) fn subagent_failure_notice(result: &str) -> Option<String> {
    const START: &str = "<codewhale:subagent.done>";
    const END: &str = "</codewhale:subagent.done>";
    let start = result.find(START)? + START.len();
    let end = result[start..].find(END)? + start;
    let value = serde_json::from_str::<serde_json::Value>(&result[start..end]).ok()?;
    (value.get("event").and_then(serde_json::Value::as_str) == Some("subagent.failed"))
        .then(|| {
            let name = value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let agent_id = value
                .get("agent_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let class = value
                .get("failure_class")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unavailable");
            let steps = value
                .get("steps")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "?".to_string(), |steps| steps.to_string());
            let elapsed_ms = value
                .get("elapsed_ms")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "?".to_string(), |elapsed| elapsed.to_string());
            let transcript_handle = value
                .get("transcript_handle")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unavailable");
            format!(
                "{name} ({agent_id}) · {class} · {steps} steps · {elapsed_ms} ms · inspect {transcript_handle}"
            )
        })
}

pub(super) fn subagent_status_from_completion_result(result: &str) -> SubAgentStatus {
    let reason = result
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty() && !trimmed.starts_with("<codewhale:subagent.done>"))
                .then_some(trimmed.to_string())
        })
        .unwrap_or_else(|| "sub-agent finished".to_string());
    match subagent_completion_status(result).as_deref() {
        Some("completed") => SubAgentStatus::Completed,
        Some("cancelled" | "canceled") => SubAgentStatus::Cancelled,
        Some("failed") => SubAgentStatus::Failed(reason),
        Some("interrupted") => SubAgentStatus::Interrupted(reason),
        Some("budget_exhausted") => SubAgentStatus::BudgetExhausted,
        _ => SubAgentStatus::Completed,
    }
}
