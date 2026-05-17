//! Main streaming turn loop for the engine.
//!
//! Extracted from `core/engine.rs` for issue #74. This module keeps the
//! existing per-turn orchestration intact: request construction, streaming
//! event handling, tool planning/execution, LSP post-edit hooks, capacity
//! checkpoints, and loop termination.

use super::*;

fn loop_guard_block_tool_result(message: String) -> ToolResult {
    ToolResult::error(message).with_metadata(json!({"loop_guard": "identical_tool_call"}))
}

impl Engine {
    pub(super) async fn handle_deepseek_turn(
        &mut self,
        turn: &mut TurnContext,
        tool_registry: Option<&crate::tools::ToolRegistry>,
        tools: Option<Vec<Tool>>,
        mode: AppMode,
        force_update_plan_first: bool,
    ) -> (TurnOutcomeStatus, Option<String>) {
        let client = self
            .deepseek_client
            .clone()
            .expect("DeepSeek client should be configured");

        let mut consecutive_tool_error_steps = 0u32;
        let mut turn_error: Option<String> = None;
        let mut context_recovery_attempts = 0u8;
        let mut tool_catalog = tools.unwrap_or_default();
        if !tool_catalog.is_empty() {
            ensure_advanced_tooling(&mut tool_catalog, mode);
        }
        let mut active_tool_names = initial_active_tools(&tool_catalog);
        let mut loop_guard = LoopGuard::default();

        // Transparent stream-retry counter: when the chunked-transfer
        // connection dies mid-stream and we got nothing useful out of it
        // (no tool calls, no completed text), we silently re-issue the
        // SAME request up to MAX_STREAM_RETRIES times before surfacing
        // the failure to the user. This is the #103 Phase 3 retry that
        // keeps long V4 thinking turns from being killed by transient
        // proxy disconnects.
        const MAX_STREAM_RETRIES: u32 = 3;
        let mut stream_retry_attempts: u32 = 0;

        loop {
            if self.cancel_token.is_cancelled() {
                let _ = self.tx_event.send(Event::status("Request cancelled")).await;
                return (TurnOutcomeStatus::Interrupted, None);
            }

            while let Ok(steer) = self.rx_steer.try_recv() {
                let steer = steer.trim().to_string();
                if steer.is_empty() {
                    continue;
                }
                self.session
                    .working_set
                    .observe_user_message(&steer, &self.session.workspace);
                self.add_session_message(self.user_text_message_with_turn_metadata(steer.clone()))
                    .await;
                let _ = self
                    .tx_event
                    .send(Event::status(format!(
                        "Steer input accepted: {}",
                        summarize_text(&steer, 120)
                    )))
                    .await;
            }

            // Ensure system prompt is up to date with latest session states
            self.refresh_system_prompt(mode);

            if turn.at_max_steps() {
                let _ = self
                    .tx_event
                    .send(Event::status("Reached maximum steps"))
                    .await;
                break;
            }

            let compaction_pins = self
                .session
                .working_set
                .pinned_message_indices(&self.session.messages, &self.session.workspace);
            let compaction_paths = self.session.working_set.top_paths(24);

            if self.config.compaction.enabled
                && should_compact(
                    &self.session.messages,
                    &self.config.compaction,
                    Some(&self.session.workspace),
                    Some(&compaction_pins),
                    Some(&compaction_paths),
                )
            {
                let compaction_id = format!("compact_{}", &uuid::Uuid::new_v4().to_string()[..8]);
                self.emit_compaction_started(
                    compaction_id.clone(),
                    true,
                    "Auto context compaction started".to_string(),
                )
                .await;
                let _ = self
                    .tx_event
                    .send(Event::status("Auto-compacting context...".to_string()))
                    .await;
                let auto_messages_before = self.session.messages.len();
                match compact_messages_safe(
                    &client,
                    &self.session.messages,
                    &self.config.compaction,
                    Some(&self.session.workspace),
                    Some(&compaction_pins),
                    Some(&compaction_paths),
                )
                .await
                {
                    Ok(result) => {
                        // Only update if we got valid messages (never corrupt state)
                        if !result.messages.is_empty() || self.session.messages.is_empty() {
                            let auto_messages_after = result.messages.len();
                            self.session.messages = result.messages;
                            self.merge_compaction_summary(result.summary_prompt);
                            self.emit_session_updated().await;
                            let removed = auto_messages_before.saturating_sub(auto_messages_after);
                            let status = if result.retries_used > 0 {
                                format!(
                                    "Auto-compaction complete: {auto_messages_before} → {auto_messages_after} messages ({removed} removed, {} retries)",
                                    result.retries_used
                                )
                            } else {
                                format!(
                                    "Auto-compaction complete: {auto_messages_before} → {auto_messages_after} messages ({removed} removed)"
                                )
                            };
                            self.emit_compaction_completed(
                                compaction_id.clone(),
                                true,
                                status.clone(),
                                Some(auto_messages_before),
                                Some(auto_messages_after),
                            )
                            .await;
                            let _ = self.tx_event.send(Event::status(status)).await;
                        } else {
                            let message = "Auto-compaction skipped: empty result".to_string();
                            self.emit_compaction_failed(
                                compaction_id.clone(),
                                true,
                                message.clone(),
                            )
                            .await;
                            let _ = self.tx_event.send(Event::status(message)).await;
                        }
                    }
                    Err(err) => {
                        // Log error but continue with original messages (never corrupt)
                        let message = format!("Auto-compaction failed: {err}");
                        self.emit_compaction_failed(compaction_id, true, message.clone())
                            .await;
                        let _ = self.tx_event.send(Event::status(message)).await;
                    }
                }
            }

            if self
                .run_capacity_pre_request_checkpoint(turn, Some(&client), mode)
                .await
            {
                continue;
            }

            if let Some(input_budget) =
                context_input_budget(&self.session.model, TURN_MAX_OUTPUT_TOKENS)
            {
                let estimated_input = self.estimated_input_tokens();
                if estimated_input > input_budget {
                    if context_recovery_attempts >= MAX_CONTEXT_RECOVERY_ATTEMPTS {
                        let message = format!(
                            "Context remains above model limit after {} recovery attempts \
                             (~{} token estimate, ~{} budget). Please run /compact or /clear.",
                            MAX_CONTEXT_RECOVERY_ATTEMPTS, estimated_input, input_budget
                        );
                        turn_error = Some(message.clone());
                        let _ = self
                            .tx_event
                            .send(Event::error(ErrorEnvelope::context_overflow(message)))
                            .await;
                        return (TurnOutcomeStatus::Failed, turn_error);
                    }

                    if self
                        .recover_context_overflow(
                            &client,
                            "preflight token budget",
                            TURN_MAX_OUTPUT_TOKENS,
                        )
                        .await
                    {
                        context_recovery_attempts = context_recovery_attempts.saturating_add(1);
                        continue;
                    }
                }
            }

            // #136: drain any LSP diagnostics collected since the last
            // request and inject them as a synthetic user message so the
            // model sees compile errors before its next reasoning step.
            self.flush_pending_lsp_diagnostics().await;

            // #159: layered context seam checkpoint. This is opt-in for
            // v0.7.5 while #200 audits cache-hit behavior; when enabled it
            // appends <archived_context> blocks rather than replacing history.
            self.layered_context_checkpoint().await;

            // Build the request
            let force_update_plan_this_step = force_update_plan_first && turn.tool_calls.is_empty();
            let mut active_tools = if tool_catalog.is_empty() {
                None
            } else {
                Some(active_tools_for_step(
                    &tool_catalog,
                    &active_tool_names,
                    force_update_plan_this_step,
                ))
            };
            if self.config.strict_tool_mode
                && let Some(tools) = active_tools.as_mut()
            {
                crate::tools::schema_sanitize::prepare_tools_for_strict_mode(tools);
            }

            // Resolve `auto` reasoning_effort to a concrete tier (#663).
            let effective_reasoning_effort = resolve_auto_effort(
                self.session.reasoning_effort.as_deref(),
                &self.session.messages,
            );

            // Check prefix-cache stability before building the request.
            // This detects system-prompt or tool-set drift that would
            // invalidate DeepSeek's KV prefix cache for this turn.
            // Sends an event on EVERY check so the TUI can maintain
            // its own counter for the stable-checks tally.
            if let Some(pm) = self.session.prefix_stability.as_mut() {
                let system_text =
                    crate::prefix_cache::system_prompt_text(self.session.system_prompt.as_ref());
                let tools_ref: Option<&[crate::models::Tool]> = active_tools.as_deref();
                match pm.check_and_update(&system_text, tools_ref) {
                    Err(change) => {
                        tracing::debug!(
                            target: "prefix_cache",
                            "{}",
                            change.description()
                        );
                        let _ = self
                            .tx_event
                            .send(Event::PrefixCacheChange {
                                description: change.description(),
                                system_prompt_changed: change.system_changed,
                                tools_changed: change.tools_changed,
                                stability_pct: (pm.stability_ratio() * 100.0).round() as u32,
                                changed: true,
                            })
                            .await;
                    }
                    Ok(_) => {
                        // Stable check — keep the TUI counter in sync.
                        let _ = self
                            .tx_event
                            .send(Event::PrefixCacheChange {
                                description: String::new(),
                                system_prompt_changed: false,
                                tools_changed: false,
                                stability_pct: (pm.stability_ratio() * 100.0).round() as u32,
                                changed: false,
                            })
                            .await;
                    }
                }
            }

            let request = MessageRequest {
                model: self.session.model.clone(),
                messages: self.messages_with_turn_metadata(),
                max_tokens: effective_max_output_tokens(&self.session.model),
                system: self.session.system_prompt.clone(),
                tools: active_tools.clone(),
                tool_choice: if active_tools.is_some() {
                    if self.config.strict_tool_mode {
                        Some(json!("required"))
                    } else {
                        Some(json!({ "type": "auto" }))
                    }
                } else {
                    None
                },
                metadata: None,
                thinking: None,
                reasoning_effort: effective_reasoning_effort,
                stream: Some(true),
                temperature: None,
                top_p: None,
            };

            // Stream the response. Keep the request around (cloned into the
            // first call) so we can resend it on a transparent retry below
            // when the wire dies before any content was streamed (#103).
            let stream_request = request;
            let stream_result = client.create_message_stream(stream_request.clone()).await;
            let stream = match stream_result {
                Ok(s) => {
                    context_recovery_attempts = 0;
                    s
                }
                Err(e) => {
                    let message = self.decorate_auth_error_message(e.to_string());
                    if is_context_length_error_message(&message)
                        && context_recovery_attempts < MAX_CONTEXT_RECOVERY_ATTEMPTS
                        && self
                            .recover_context_overflow(
                                &client,
                                "provider context-length rejection",
                                TURN_MAX_OUTPUT_TOKENS,
                            )
                            .await
                    {
                        context_recovery_attempts = context_recovery_attempts.saturating_add(1);
                        continue;
                    }
                    turn_error = Some(message.clone());
                    let _ = self
                        .tx_event
                        .send(Event::error(ErrorEnvelope::classify(message, true)))
                        .await;
                    return (TurnOutcomeStatus::Failed, turn_error);
                }
            };
            // The stream value is itself `Pin<Box<dyn Stream + Send>>`, which
            // is `Unpin`, so we can rebind it on a transparent retry without
            // breaking the existing pin invariants.
            let mut stream = stream;

            // Track content blocks
            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            let mut current_text_raw = String::new();
            let mut current_text_visible = String::new();
            let mut current_thinking = String::new();
            let mut tool_uses: Vec<ToolUseState> = Vec::new();
            let mut usage = Usage {
                input_tokens: 0,
                output_tokens: 0,
                ..Usage::default()
            };
            let mut current_block_kind: Option<ContentBlockKind> = None;
            // Map block_index → tool_uses position. Required because the
            // OpenAI-compatible streaming parser emits multiple
            // ContentBlockStart::ToolUse events back-to-back (one per
            // tool_call in a batch) before any ContentBlockStop arrives —
            // all Stops are flushed together at `finish_reason`. A single
            // Option<usize> gets overwritten by each new Start; the first
            // Stop then takes the last index, and every subsequent Stop
            // takes `None`, dropping ToolCallStarted events for every
            // tool call except the last one in the batch.
            let mut current_tool_indices: std::collections::HashMap<u32, usize> =
                std::collections::HashMap::new();
            let mut in_tool_call_block = false;
            let mut fake_wrapper_notice_emitted = false;
            let mut pending_message_complete = false;
            let mut last_text_index: Option<usize> = None;
            let mut stream_errors = 0u32;
            // #103 transparent retry bookkeeping. `any_content_received` flips
            // on the first non-MessageStart event so we know whether DeepSeek
            // billed us / the user has seen any output for this turn yet.
            // This is distinct from the outer `stream_retry_attempts` (which
            // restarts the whole turn-step when a stream died with no
            // content-block delta delivered to the consumer).
            let mut any_content_received = false;
            let mut transparent_stream_retries = 0u32;
            let mut pending_steers: Vec<String> = Vec::new();
            // `stream_start` is reset on a transparent retry so the wall-clock
            // budget restarts with the fresh stream.
            let mut stream_start = Instant::now();
            let mut stream_content_bytes: usize = 0;
            let chunk_timeout_secs = stream_chunk_timeout_secs();
            let chunk_timeout = Duration::from_secs(chunk_timeout_secs);
            let max_duration = Duration::from_secs(STREAM_MAX_DURATION_SECS);

            // Process stream events
            loop {
                let poll_outcome = tokio::select! {
                    _ = self.cancel_token.cancelled() => None,
                    result = tokio::time::timeout(chunk_timeout, stream.next()) => {
                        match result {
                            Ok(Some(event_result)) => Some(event_result),
                            Ok(None) => None, // stream ended normally
                            Err(_) => {
                                let envelope = StreamError::Stall {
                                    timeout_secs: chunk_timeout_secs,
                                }
                                .into_envelope();
                                crate::logging::warn(&envelope.message);
                                let _ = self.tx_event.send(Event::error(envelope)).await;
                                None
                            }
                        }
                    }
                };
                let Some(event_result) = poll_outcome else {
                    break;
                };
                while let Ok(steer) = self.rx_steer.try_recv() {
                    let steer = steer.trim().to_string();
                    if steer.is_empty() {
                        continue;
                    }
                    pending_steers.push(steer.clone());
                    let _ = self
                        .tx_event
                        .send(Event::status(format!(
                            "Steer input queued: {}",
                            summarize_text(&steer, 120)
                        )))
                        .await;
                }

                if self.cancel_token.is_cancelled() {
                    break;
                }

                // Guard: max wall-clock duration
                if stream_start.elapsed() > max_duration {
                    let envelope = StreamError::DurationLimit {
                        limit_secs: STREAM_MAX_DURATION_SECS,
                    }
                    .into_envelope();
                    crate::logging::warn(&envelope.message);
                    turn_error.get_or_insert(envelope.message.clone());
                    let _ = self.tx_event.send(Event::error(envelope)).await;
                    break;
                }

                // Guard: max accumulated content bytes
                if stream_content_bytes > STREAM_MAX_CONTENT_BYTES {
                    let envelope = StreamError::Overflow {
                        limit_bytes: STREAM_MAX_CONTENT_BYTES,
                    }
                    .into_envelope();
                    crate::logging::warn(&envelope.message);
                    turn_error.get_or_insert(envelope.message.clone());
                    let _ = self.tx_event.send(Event::error(envelope)).await;
                    break;
                }

                let event = match event_result {
                    Ok(e) => {
                        // Flip on the first non-MessageStart event — that's
                        // the moment we cross from "stream not yet productive"
                        // (eligible for transparent retry) into "DeepSeek has
                        // billed us / user has seen output" (must surface).
                        if !any_content_received && !matches!(e, StreamEvent::MessageStart { .. }) {
                            any_content_received = true;
                        }
                        e
                    }
                    Err(e) => {
                        stream_errors = stream_errors.saturating_add(1);
                        let message = self.decorate_auth_error_message(e.to_string());
                        // #103: when the stream errors before any content was
                        // streamed AND we still have retry budget, transparently
                        // resend the request. DeepSeek has not billed for any
                        // output and the user has seen nothing — re-trying is
                        // the right user-visible behavior.
                        if should_transparently_retry_stream(
                            any_content_received,
                            transparent_stream_retries,
                            self.cancel_token.is_cancelled(),
                        ) {
                            transparent_stream_retries =
                                transparent_stream_retries.saturating_add(1);
                            crate::logging::info(format!(
                                "Transparent stream retry {}/{} (no content received yet): {}",
                                transparent_stream_retries, MAX_TRANSPARENT_STREAM_RETRIES, message,
                            ));
                            // Drop the failed stream before issuing the new
                            // request to release the underlying connection.
                            drop(stream);
                            match client.create_message_stream(stream_request.clone()).await {
                                Ok(fresh) => {
                                    stream = fresh;
                                    stream_start = Instant::now();
                                    // Roll back the error counter — this one
                                    // didn't surface to the user.
                                    stream_errors = stream_errors.saturating_sub(1);
                                    continue;
                                }
                                Err(retry_err) => {
                                    let retry_msg = self.decorate_auth_error_message(format!(
                                        "Stream retry failed: {retry_err}"
                                    ));
                                    turn_error.get_or_insert(retry_msg.clone());
                                    let _ = self
                                        .tx_event
                                        .send(Event::error(ErrorEnvelope::classify(
                                            retry_msg, true,
                                        )))
                                        .await;
                                    break;
                                }
                            }
                        }
                        turn_error.get_or_insert(message.clone());
                        let _ = self
                            .tx_event
                            .send(Event::error(ErrorEnvelope::classify(message, true)))
                            .await;
                        if stream_errors >= MAX_STREAM_ERRORS_BEFORE_FAIL {
                            break;
                        }
                        continue;
                    }
                };

                match event {
                    StreamEvent::MessageStart { message } => {
                        usage = message.usage;
                    }
                    StreamEvent::ContentBlockStart {
                        index,
                        content_block,
                    } => match content_block {
                        ContentBlockStart::Text { text } => {
                            current_text_raw = text;
                            current_text_visible.clear();
                            in_tool_call_block = false;
                            let filtered =
                                filter_tool_call_delta(&current_text_raw, &mut in_tool_call_block);
                            if !fake_wrapper_notice_emitted
                                && filtered.len() < current_text_raw.len()
                                && contains_fake_tool_wrapper(&current_text_raw)
                            {
                                let _ =
                                    self.tx_event.send(Event::status(FAKE_WRAPPER_NOTICE)).await;
                                fake_wrapper_notice_emitted = true;
                            }
                            current_text_visible.push_str(&filtered);
                            current_block_kind = Some(ContentBlockKind::Text);
                            last_text_index = Some(index as usize);
                            let _ = self
                                .tx_event
                                .send(Event::MessageStarted {
                                    index: index as usize,
                                })
                                .await;
                        }
                        ContentBlockStart::Thinking { thinking } => {
                            current_thinking = thinking;
                            current_block_kind = Some(ContentBlockKind::Thinking);
                            let _ = self
                                .tx_event
                                .send(Event::ThinkingStarted {
                                    index: index as usize,
                                })
                                .await;
                        }
                        ContentBlockStart::ToolUse {
                            id,
                            name,
                            input,
                            caller,
                        } => {
                            crate::logging::info(format!(
                                "Tool '{}' block start. Initial input: {:?}",
                                name, input
                            ));
                            current_block_kind = Some(ContentBlockKind::ToolUse);
                            current_tool_indices.insert(index, tool_uses.len());
                            // ToolCallStarted is deferred to ContentBlockStop —
                            // see `final_tool_input`. Emitting here would ship
                            // the placeholder `{}` and the cell would render
                            // `<command>` / `<file>` literals to the user.
                            tool_uses.push(ToolUseState {
                                id,
                                name,
                                input,
                                caller,
                                input_buffer: String::new(),
                            });
                        }
                        ContentBlockStart::ServerToolUse { id, name, input } => {
                            crate::logging::info(format!(
                                "Server tool '{}' block start. Initial input: {:?}",
                                name, input
                            ));
                            current_block_kind = Some(ContentBlockKind::ToolUse);
                            current_tool_indices.insert(index, tool_uses.len());
                            tool_uses.push(ToolUseState {
                                id,
                                name,
                                input,
                                caller: None,
                                input_buffer: String::new(),
                            });
                        }
                    },
                    StreamEvent::ContentBlockDelta { index, delta } => match delta {
                        Delta::TextDelta { text } => {
                            stream_content_bytes = stream_content_bytes.saturating_add(text.len());
                            current_text_raw.push_str(&text);
                            let filtered = filter_tool_call_delta(&text, &mut in_tool_call_block);
                            if !fake_wrapper_notice_emitted
                                && filtered.len() < text.len()
                                && contains_fake_tool_wrapper(&text)
                            {
                                let _ =
                                    self.tx_event.send(Event::status(FAKE_WRAPPER_NOTICE)).await;
                                fake_wrapper_notice_emitted = true;
                            }
                            if !filtered.is_empty() {
                                current_text_visible.push_str(&filtered);
                                let _ = self
                                    .tx_event
                                    .send(Event::MessageDelta {
                                        index: index as usize,
                                        content: filtered,
                                    })
                                    .await;
                            }
                        }
                        Delta::ThinkingDelta { thinking } => {
                            stream_content_bytes =
                                stream_content_bytes.saturating_add(thinking.len());
                            current_thinking.push_str(&thinking);
                            if !thinking.is_empty() {
                                let _ = self
                                    .tx_event
                                    .send(Event::ThinkingDelta {
                                        index: index as usize,
                                        content: thinking,
                                    })
                                    .await;
                            }
                        }
                        Delta::InputJsonDelta { partial_json } => {
                            if let Some(&tool_idx) = current_tool_indices.get(&index)
                                && let Some(tool_state) = tool_uses.get_mut(tool_idx)
                            {
                                tool_state.input_buffer.push_str(&partial_json);
                                crate::logging::info(format!(
                                    "Tool '{}' input delta: {} (buffer now: {})",
                                    tool_state.name, partial_json, tool_state.input_buffer
                                ));
                                if let Some(value) = parse_tool_input(&tool_state.input_buffer) {
                                    tool_state.input = value.clone();
                                    crate::logging::info(format!(
                                        "Tool '{}' input parsed: {:?}",
                                        tool_state.name, value
                                    ));
                                }
                            }
                        }
                    },
                    StreamEvent::ContentBlockStop { index } => {
                        let stopped_kind = current_block_kind.take();
                        match stopped_kind {
                            Some(ContentBlockKind::Text) => {
                                pending_message_complete = true;
                                last_text_index = Some(index as usize);
                            }
                            Some(ContentBlockKind::Thinking) => {
                                let _ = self
                                    .tx_event
                                    .send(Event::ThinkingComplete {
                                        index: index as usize,
                                    })
                                    .await;
                            }
                            Some(ContentBlockKind::ToolUse) | None => {}
                        }
                        // Route the Stop using event.index (via
                        // `current_tool_indices`) rather than the single
                        // `current_block_kind` slot. In an OpenAI batch
                        // tool-call stream every Stop after the first sees
                        // `stopped_kind = None` because `take()` cleared the
                        // slot, so the original `matches!(stopped_kind, …)`
                        // check would skip every tool except the last.
                        if let Some(tool_idx) = current_tool_indices.remove(&index)
                            && let Some(tool_state) = tool_uses.get_mut(tool_idx)
                        {
                            crate::logging::info(format!(
                                "Tool '{}' block stop. Buffer: '{}', Current input: {:?}",
                                tool_state.name, tool_state.input_buffer, tool_state.input
                            ));
                            if !tool_state.input_buffer.trim().is_empty() {
                                if let Some(value) = parse_tool_input(&tool_state.input_buffer) {
                                    tool_state.input = value;
                                    crate::logging::info(format!(
                                        "Tool '{}' final input: {:?}",
                                        tool_state.name, tool_state.input
                                    ));
                                } else {
                                    crate::logging::warn(format!(
                                        "Tool '{}' failed to parse final input buffer: '{}'",
                                        tool_state.name, tool_state.input_buffer
                                    ));
                                    let _ = self
                                        .tx_event
                                        .send(Event::status(format!(
                                            "⚠ Tool '{}' received malformed arguments from model",
                                            tool_state.name
                                        )))
                                        .await;
                                }
                            } else {
                                crate::logging::warn(format!(
                                    "Tool '{}' input buffer is empty, using initial input: {:?}",
                                    tool_state.name, tool_state.input
                                ));
                            }

                            // Now that the input is finalized, announce the
                            // tool call to the UI. Deferring to here is what
                            // keeps the cell from rendering `<command>` /
                            // `<file>` placeholders during the brief window
                            // between block start and the last InputJsonDelta.
                            let _ = self
                                .tx_event
                                .send(Event::ToolCallStarted {
                                    id: tool_state.id.clone(),
                                    name: tool_state.name.clone(),
                                    input: final_tool_input(tool_state),
                                })
                                .await;
                        }
                    }
                    StreamEvent::MessageDelta {
                        usage: delta_usage, ..
                    } => {
                        if let Some(u) = delta_usage {
                            usage = u;
                        }
                    }
                    StreamEvent::MessageStop | StreamEvent::Ping => {}
                }
            }

            // #103 Phase 3 — transparent retry. The inner loop above bails
            // when reqwest yields chunk decode errors three times in a row;
            // most of the time those are recoverable proxy / HTTP/2 issues
            // and the request can simply be re-issued. Re-issue silently up
            // to MAX_STREAM_RETRIES, but only when the stream produced
            // nothing actionable — if any tool call landed or text was
            // streamed, ship the partial state to the rest of the turn
            // pipeline so we don't double-bill the user by re-running it.
            let stream_died_with_nothing = stream_errors > 0
                && tool_uses.is_empty()
                && current_text_visible.trim().is_empty()
                && current_thinking.trim().is_empty()
                && !pending_message_complete;
            if stream_died_with_nothing {
                if stream_retry_attempts < MAX_STREAM_RETRIES {
                    stream_retry_attempts = stream_retry_attempts.saturating_add(1);
                    crate::logging::warn(format!(
                        "Stream died with no content (attempt {}/{}); retrying request",
                        stream_retry_attempts, MAX_STREAM_RETRIES
                    ));
                    let _ = self
                        .tx_event
                        .send(Event::status(format!(
                            "Connection interrupted; retrying ({}/{})",
                            stream_retry_attempts, MAX_STREAM_RETRIES
                        )))
                        .await;
                    // Don't preserve the per-stream `turn_error` — we're
                    // about to retry, and a successful retry should not
                    // surface the transient error as the turn outcome.
                    turn_error = None;
                    continue;
                }
                crate::logging::warn(format!(
                    "Stream retry budget exhausted ({} attempts); failing turn",
                    stream_retry_attempts
                ));
            } else if stream_errors == 0 {
                // Healthy round → reset retry budget so we don't carry over
                // state from a previous bad round.
                stream_retry_attempts = 0;
            }

            // Update turn usage
            turn.add_usage(&usage);

            // Build content blocks. If this assistant turn produced tool
            // calls, ensure a Thinking block is present even when the model
            // didn't stream any reasoning text — DeepSeek's thinking-mode
            // API requires `reasoning_content` to accompany every tool-call
            // assistant message in the conversation history. Saving a
            // placeholder here keeps the on-disk session structurally
            // correct so subsequent requests won't 400.
            let needs_thinking_block =
                !tool_uses.is_empty() || tool_parser::has_tool_call_markers(&current_text_raw);
            let thinking_to_persist = if !current_thinking.is_empty() {
                Some(current_thinking.clone())
            } else if needs_thinking_block {
                Some(String::from("(reasoning omitted)"))
            } else {
                None
            };
            if let Some(thinking) = thinking_to_persist {
                content_blocks.push(ContentBlock::Thinking { thinking });
            }
            let mut final_text = current_text_visible.clone();
            if tool_uses.is_empty() && tool_parser::has_tool_call_markers(&current_text_raw) {
                let parsed = tool_parser::parse_tool_calls(&current_text_raw);
                final_text = parsed.clean_text;
                for call in parsed.tool_calls {
                    let _ = self
                        .tx_event
                        .send(Event::ToolCallStarted {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            input: call.args.clone(),
                        })
                        .await;
                    tool_uses.push(ToolUseState {
                        id: call.id,
                        name: call.name,
                        input: call.args,
                        caller: None,
                        input_buffer: String::new(),
                    });
                }
            }

            if !final_text.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: final_text,
                    cache_control: None,
                });
            }
            for tool in &tool_uses {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    input: tool.input.clone(),
                    caller: tool.caller.clone(),
                });
            }

            if pending_message_complete {
                let index = last_text_index.unwrap_or(0);
                let _ = self.tx_event.send(Event::MessageComplete { index }).await;
            }

            // RLM is a structured tool call (`rlm_query`) handled by the
            // normal tool dispatch path; inline ```repl blocks (paper §2)
            // are executed below when tool_uses is empty.
            // DeepSeek chat API rejects assistant messages that contain only
            // Keep thinking for UI stream events, but persist only sendable
            // assistant turns in the conversation state.
            let has_sendable_assistant_content = content_blocks.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::Text { .. } | ContentBlock::ToolUse { .. }
                )
            });

            // Add assistant message to session
            if has_sendable_assistant_content {
                self.add_session_message(Message {
                    role: "assistant".to_string(),
                    content: content_blocks,
                })
                .await;
            }

            // If no tool uses, check for inline REPL blocks (paper §2) or
            // finish the turn.
            if tool_uses.is_empty() {
                if !pending_steers.is_empty() {
                    for steer in pending_steers.drain(..) {
                        self.session
                            .working_set
                            .observe_user_message(&steer, &self.session.workspace);
                        self.add_session_message(self.user_text_message_with_turn_metadata(steer))
                            .await;
                    }
                    turn.next_step();
                    continue;
                }

                // Sub-agent completion handoff (issue #756). The model finished
                // streaming with no tool calls — but if it has direct children
                // still running (or completions queued from children that
                // finished while we were inferring), surface their
                // `<deepseek:subagent.done>` sentinels into the transcript and
                // resume instead of ending the turn. This fulfils the contract
                // already documented in `prompts/base.md`: the parent is
                // promised it'll see the sentinel when a child finishes.
                let mut completions: Vec<crate::tools::subagent::SubAgentCompletion> = Vec::new();
                while let Ok(c) = self.rx_subagent_completion.try_recv() {
                    completions.push(c);
                }
                if completions.is_empty() {
                    let running = {
                        let mgr = self.subagent_manager.read().await;
                        mgr.running_count()
                    };
                    if should_hold_turn_for_subagents(completions.len(), running) {
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Waiting on {running} sub-agent(s) to complete..."
                            )))
                            .await;
                        tokio::select! {
                            biased;
                            () = self.cancel_token.cancelled() => {
                                let _ = self
                                    .tx_event
                                    .send(Event::status(
                                        "Request cancelled while waiting for sub-agents",
                                    ))
                                    .await;
                                return (TurnOutcomeStatus::Interrupted, None);
                            }
                            Some(c) = self.rx_subagent_completion.recv() => {
                                completions.push(c);
                                while let Ok(extra) = self.rx_subagent_completion.try_recv() {
                                    completions.push(extra);
                                }
                            }
                            Some(steer) = self.rx_steer.recv() => {
                                let trimmed = steer.trim().to_string();
                                if !trimmed.is_empty() {
                                    self.session
                                        .working_set
                                        .observe_user_message(&trimmed, &self.session.workspace);
                                    self.add_session_message(
                                        self.user_text_message_with_turn_metadata(trimmed.clone()),
                                    )
                                    .await;
                                    let _ = self
                                        .tx_event
                                        .send(Event::status(format!(
                                            "Steer input accepted: {}",
                                            summarize_text(&trimmed, 120)
                                        )))
                                        .await;
                                }
                                turn.next_step();
                                continue;
                            }
                        }
                    }
                }
                if !completions.is_empty() {
                    let count = completions.len();
                    for c in completions {
                        self.add_session_message(subagent_completion_runtime_message(&c.payload))
                            .await;
                    }
                    let _ = self
                        .tx_event
                        .send(Event::status(format!(
                            "Resuming turn with {count} sub-agent completion(s)"
                        )))
                        .await;
                    turn.next_step();
                    continue;
                }

                // Inline ```repl execution — paper-spec RLM integration.
                if has_sendable_assistant_content
                    && crate::repl::sandbox::has_repl_block(&current_text_visible)
                {
                    let repl_blocks =
                        crate::repl::sandbox::extract_repl_blocks(&current_text_visible);
                    let mut runtime = match crate::repl::runtime::PythonRuntime::new().await {
                        Ok(rt) => rt,
                        Err(e) => {
                            let _ = self
                                .tx_event
                                .send(Event::status(format!("REPL init failed: {e}")))
                                .await;
                            break;
                        }
                    };

                    let mut final_result: Option<String> = None;
                    for (i, block) in repl_blocks.iter().enumerate() {
                        let round_num = i + 1;
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "REPL round {round_num}: executing..."
                            )))
                            .await;

                        match runtime.execute(&block.code).await {
                            Ok(round) => {
                                if let Some(val) = &round.final_value {
                                    let _ = self
                                        .tx_event
                                        .send(Event::status(format!(
                                            "REPL round {round_num}: FINAL result obtained"
                                        )))
                                        .await;
                                    final_result = Some(val.clone());
                                    break;
                                }

                                // No FINAL — feed truncated stdout back as user metadata.
                                let feedback = if round.has_error {
                                    format!(
                                        "[REPL round {round_num} error]\nstdout:\n{}\nstderr:\n{}",
                                        round.stdout, round.stderr
                                    )
                                } else {
                                    format!("[REPL round {round_num} output]\n{}", round.stdout)
                                };
                                self.add_session_message(
                                    self.user_text_message_with_turn_metadata(feedback),
                                )
                                .await;
                            }
                            Err(e) => {
                                let _ = self
                                    .tx_event
                                    .send(Event::status(format!(
                                        "REPL round {round_num} failed: {e}"
                                    )))
                                    .await;
                                self.add_session_message(
                                    self.user_text_message_with_turn_metadata(format!(
                                        "[REPL round {round_num} execution failed]\n{e}"
                                    )),
                                )
                                .await;
                            }
                        }
                    }

                    if let Some(final_val) = final_result {
                        // Replace the assistant's text with the FINAL answer.
                        if let Some(last_msg) = self.session.messages.last_mut()
                            && last_msg.role == "assistant"
                        {
                            for block in &mut last_msg.content {
                                if let ContentBlock::Text { text, .. } = block {
                                    *text = final_val;
                                    break;
                                }
                            }
                        }
                        self.emit_session_updated().await;
                        break;
                    }

                    // No FINAL — let the model iterate with the feedback.
                    turn.next_step();
                    continue;
                }

                break;
            }

            // Execute tools
            let tool_exec_lock = self.tool_exec_lock.clone();
            let mcp_pool = if tool_uses
                .iter()
                .any(|tool| McpPool::is_mcp_tool(&tool.name))
            {
                match self.ensure_mcp_pool().await {
                    Ok(pool) => Some(pool),
                    Err(err) => {
                        let _ = self.tx_event.send(Event::status(err.to_string())).await;
                        None
                    }
                }
            } else {
                None
            };

            let active_tools_at_batch_start = active_tool_names.clone();
            let mut deferred_tools_hydrated_this_batch: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut plans: Vec<ToolExecutionPlan> = Vec::with_capacity(tool_uses.len());
            for (index, tool) in tool_uses.iter_mut().enumerate() {
                let tool_id = tool.id.clone();
                let mut tool_name = tool.name.clone();
                let tool_input = tool.input.clone();
                let tool_caller = tool.caller.clone();
                crate::logging::info(format!(
                    "Planning tool '{}' with input: {:?}",
                    tool_name, tool_input
                ));

                let interactive = (tool_name == "exec_shell"
                    && tool_input
                        .get("interactive")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true))
                    || tool_name == REQUEST_USER_INPUT_NAME;

                let mut approval_required = false;
                let mut approval_description = "Tool execution requires approval".to_string();
                let mut supports_parallel = false;
                let mut read_only = false;
                let mut blocked_error: Option<ToolError> = None;
                let mut guard_result: Option<ToolResult> = None;

                if mode == AppMode::Plan
                    && matches!(
                        tool_name.as_str(),
                        "exec_shell"
                            | "exec_shell_wait"
                            | "exec_shell_interact"
                            | "exec_wait"
                            | "exec_interact"
                            | CODE_EXECUTION_TOOL_NAME
                            | JS_EXECUTION_TOOL_NAME
                    )
                {
                    blocked_error = Some(ToolError::permission_denied(format!(
                        "Tool '{tool_name}' is unavailable in Plan mode"
                    )));
                }

                let requested_tool_name = tool_name.clone();
                let mut tool_def = tool_catalog.iter().find(|def| def.name == tool_name);

                // Resolve hallucinated tool names when the model emits a
                // non-canonical variant (Read_file, readFile, read-file, etc.).
                if tool_def.is_none()
                    && let Some(registry) = tool_registry
                    && let Some(canonical) = registry.resolve(&tool_name)
                {
                    crate::logging::info(format!(
                        "Resolved hallucinated tool name '{}' -> '{}'",
                        tool_name, canonical
                    ));
                    tool_def = tool_catalog.iter().find(|d| d.name == canonical);
                    if tool_def.is_some() {
                        tool_name = canonical.to_string();
                        // Update the tool_uses entry so the result is
                        // attributed to the canonical name.
                        tool.name = tool_name.clone();
                    }
                }

                if !caller_allowed_for_tool(tool_caller.as_ref(), tool_def) {
                    blocked_error = Some(ToolError::permission_denied(format!(
                        "Tool '{tool_name}' does not allow caller '{}'",
                        caller_type_for_tool_use(tool_caller.as_ref())
                    )));
                }

                if blocked_error.is_none()
                    && tool_def.is_none()
                    && !McpPool::is_mcp_tool(&tool_name)
                    && tool_name != CODE_EXECUTION_TOOL_NAME
                    && tool_name != JS_EXECUTION_TOOL_NAME
                    && !is_tool_search_tool(&tool_name)
                {
                    blocked_error = Some(ToolError::not_available(missing_tool_error_message(
                        &tool_name,
                        &tool_catalog,
                    )));
                }

                if McpPool::is_mcp_tool(&tool_name) {
                    read_only = mcp_tool_is_read_only(&tool_name);
                    supports_parallel = mcp_tool_is_parallel_safe(&tool_name);
                    approval_required = !read_only;
                    approval_description = mcp_tool_approval_description(&tool_name);
                } else if let Some(registry) = tool_registry
                    && let Some(spec) = registry.get(&tool_name)
                {
                    approval_required = spec.approval_requirement() != ApprovalRequirement::Auto;
                    approval_description = spec.description().to_string();
                    supports_parallel = spec.supports_parallel();
                    read_only = spec.is_read_only();
                } else if tool_name == CODE_EXECUTION_TOOL_NAME {
                    approval_required = true;
                    approval_description =
                        "Run model-provided Python code in local execution sandbox".to_string();
                    supports_parallel = false;
                    read_only = false;
                } else if tool_name == JS_EXECUTION_TOOL_NAME {
                    approval_required = true;
                    approval_description =
                        "Run model-provided JavaScript code in local Node.js execution sandbox"
                            .to_string();
                    supports_parallel = false;
                    read_only = false;
                } else if is_tool_search_tool(&tool_name) {
                    approval_required = false;
                    approval_description = "Search tool catalog".to_string();
                    supports_parallel = false;
                    read_only = true;
                }

                let should_emit_hydration_status =
                    !deferred_tools_hydrated_this_batch.contains(&tool_name);
                if blocked_error.is_none()
                    && let Some(result) = maybe_hydrate_requested_deferred_tool(
                        &tool_name,
                        &tool_input,
                        &tool_catalog,
                        &active_tools_at_batch_start,
                        &mut deferred_tools_hydrated_this_batch,
                    )
                {
                    if should_emit_hydration_status {
                        let status = if requested_tool_name == tool_name {
                            format!("Auto-loaded deferred tool '{tool_name}' after model request.")
                        } else {
                            format!(
                                "Auto-loaded deferred tool '{}' after resolving '{}'.",
                                tool_name, requested_tool_name
                            )
                        };
                        let _ = self.tx_event.send(Event::status(status)).await;
                    }
                    guard_result = Some(result);
                }

                if blocked_error.is_none()
                    && guard_result.is_none()
                    && let AttemptDecision::Block(message) =
                        loop_guard.record_attempt(&tool_name, &tool_input)
                {
                    crate::logging::warn(message.clone());
                    guard_result = Some(loop_guard_block_tool_result(message));
                }

                plans.push(ToolExecutionPlan {
                    index,
                    id: tool_id,
                    name: tool_name,
                    input: tool_input,
                    caller: tool_caller,
                    interactive,
                    approval_required,
                    approval_description,
                    supports_parallel,
                    read_only,
                    blocked_error,
                    guard_result,
                });
            }
            active_tool_names.extend(deferred_tools_hydrated_this_batch);

            let plan_count = plans.len();
            let batches = plan_tool_execution_batches(plans);
            let parallel_chunks = batches
                .iter()
                .filter_map(|batch| match batch {
                    ToolExecutionBatch::Parallel(plans) if plans.len() > 1 => Some(plans.len()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !parallel_chunks.is_empty() {
                let parallel_tool_count: usize = parallel_chunks.iter().sum();
                let _ = self
                    .tx_event
                    .send(Event::status(format!(
                        "Executing {parallel_tool_count} read-only tools in {} parallel chunk(s)",
                        parallel_chunks.len()
                    )))
                    .await;
            } else if plan_count > 1 {
                let _ = self
                    .tx_event
                    .send(Event::status(
                        "Executing tools sequentially (writes, approvals, or non-parallel tools detected)",
                    ))
                    .await;
            }

            let mut outcomes: Vec<Option<ToolExecOutcome>> = Vec::with_capacity(plan_count);
            outcomes.resize_with(plan_count, || None);

            for batch in batches {
                let (parallel_allowed, plans) = match batch {
                    ToolExecutionBatch::Parallel(plans) => (true, plans),
                    ToolExecutionBatch::Serial(plan) => (false, vec![*plan]),
                };

                if parallel_allowed {
                    let mut tool_tasks = FuturesUnordered::new();
                    for plan in plans {
                        if let Some(result) = plan.guard_result.clone() {
                            let result = Ok(result);
                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: plan.id.clone(),
                                    name: plan.name.clone(),
                                    result: result.clone(),
                                })
                                .await;
                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: plan.id,
                                name: plan.name,
                                input: plan.input,
                                started_at: Instant::now(),
                                result,
                            });
                            continue;
                        }
                        if let Some(err) = plan.blocked_error.clone() {
                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: plan.id,
                                name: plan.name,
                                input: plan.input,
                                started_at: Instant::now(),
                                result: Err(err),
                            });
                            continue;
                        }
                        let registry = tool_registry;
                        let lock = tool_exec_lock.clone();
                        let mcp_pool = mcp_pool.clone();
                        let tx_event = self.tx_event.clone();
                        let session_id = self.session.id.clone();
                        let started_at = Instant::now();

                        tool_tasks.push(async move {
                            let mut result = Engine::execute_tool_with_lock(
                                lock,
                                plan.supports_parallel,
                                plan.interactive,
                                tx_event.clone(),
                                plan.name.clone(),
                                plan.input.clone(),
                                registry,
                                mcp_pool,
                                None,
                            )
                            .await;

                            // #500: spill outsized output before fanout (mirror
                            // of the sequential path below). Emit a
                            // `tool.spillover` audit event so operators can
                            // correlate large-output episodes with disk usage.
                            if let Ok(tool_result) = result.as_mut()
                                && let Some(path) =
                                    crate::tools::truncate::apply_spillover_with_artifact(
                                        tool_result,
                                        &plan.id,
                                        &plan.name,
                                        &session_id,
                                    )
                            {
                                emit_tool_audit(json!({
                                    "event": "tool.spillover",
                                    "tool_id": plan.id.clone(),
                                    "tool_name": plan.name.clone(),
                                    "path": path.display().to_string(),
                                }));
                            }

                            let _ = tx_event
                                .send(Event::ToolCallComplete {
                                    id: plan.id.clone(),
                                    name: plan.name.clone(),
                                    result: result.clone(),
                                })
                                .await;

                            ToolExecOutcome {
                                index: plan.index,
                                id: plan.id,
                                name: plan.name,
                                input: plan.input,
                                started_at,
                                result,
                            }
                        });
                    }

                    while let Some(outcome) = tool_tasks.next().await {
                        let index = outcome.index;
                        outcomes[index] = Some(outcome);
                    }
                } else {
                    for plan in plans {
                        let tool_id = plan.id.clone();
                        let tool_name = plan.name.clone();
                        let tool_input = plan.input.clone();
                        let tool_caller = plan.caller.clone();

                        if let Some(result) = plan.guard_result.clone() {
                            let result = Ok(result);
                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    result: result.clone(),
                                })
                                .await;
                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: tool_id,
                                name: tool_name,
                                input: tool_input,
                                started_at: Instant::now(),
                                result,
                            });
                            continue;
                        }

                        if let Some(err) = plan.blocked_error.clone() {
                            let result = Err(err);
                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    result: result.clone(),
                                })
                                .await;
                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: tool_id,
                                name: tool_name,
                                input: tool_input,
                                started_at: Instant::now(),
                                result,
                            });
                            continue;
                        }

                        if tool_name == MULTI_TOOL_PARALLEL_NAME {
                            let started_at = Instant::now();
                            let result = self
                                .execute_parallel_tool(
                                    tool_input.clone(),
                                    tool_registry,
                                    tool_exec_lock.clone(),
                                )
                                .await;

                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    result: result.clone(),
                                })
                                .await;

                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: tool_id,
                                name: tool_name,
                                input: tool_input,
                                started_at,
                                result,
                            });
                            continue;
                        }

                        if tool_name == CODE_EXECUTION_TOOL_NAME {
                            let started_at = Instant::now();
                            let result =
                                execute_code_execution_tool(&tool_input, &self.session.workspace)
                                    .await;

                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    result: result.clone(),
                                })
                                .await;

                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: tool_id,
                                name: tool_name,
                                input: tool_input,
                                started_at,
                                result,
                            });
                            continue;
                        }

                        if tool_name == JS_EXECUTION_TOOL_NAME {
                            let started_at = Instant::now();
                            let result =
                                execute_js_execution_tool(&tool_input, &self.session.workspace)
                                    .await;

                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    result: result.clone(),
                                })
                                .await;

                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: tool_id,
                                name: tool_name,
                                input: tool_input,
                                started_at,
                                result,
                            });
                            continue;
                        }

                        if is_tool_search_tool(&tool_name) {
                            let started_at = Instant::now();
                            let result = execute_tool_search(
                                &tool_name,
                                &tool_input,
                                &tool_catalog,
                                &mut active_tool_names,
                            );

                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    result: result.clone(),
                                })
                                .await;

                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: tool_id,
                                name: tool_name,
                                input: tool_input,
                                started_at,
                                result,
                            });
                            continue;
                        }

                        if tool_name == REQUEST_USER_INPUT_NAME {
                            let started_at = Instant::now();
                            let result = match UserInputRequest::from_value(&tool_input) {
                                Ok(request) => self
                                    .await_user_input(&tool_id, request)
                                    .await
                                    .and_then(|response| {
                                        ToolResult::json(&response)
                                            .map_err(|e| ToolError::execution_failed(e.to_string()))
                                    }),
                                Err(err) => Err(err),
                            };

                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    result: result.clone(),
                                })
                                .await;

                            outcomes[plan.index] = Some(ToolExecOutcome {
                                index: plan.index,
                                id: tool_id,
                                name: tool_name,
                                input: tool_input,
                                started_at,
                                result,
                            });
                            continue;
                        }

                        // Handle approval flow: returns (result_override, context_override)
                        let (result_override, context_override): (
                            Option<Result<ToolResult, ToolError>>,
                            Option<crate::tools::ToolContext>,
                        ) = if plan.approval_required {
                            emit_tool_audit(json!({
                                "event": "tool.approval_required",
                                "tool_id": tool_id.clone(),
                                "tool_name": tool_name.clone(),
                            }));
                            let approval_key = crate::tools::approval_cache::build_approval_key(
                                &tool_name,
                                &tool_input,
                            )
                            .0;
                            let approval_grouping_key =
                                crate::tools::approval_cache::build_approval_grouping_key(
                                    &tool_name,
                                    &tool_input,
                                )
                                .0;
                            let _ = self
                                .tx_event
                                .send(Event::ApprovalRequired {
                                    id: tool_id.clone(),
                                    tool_name: tool_name.clone(),
                                    description: plan.approval_description.clone(),
                                    approval_key,
                                    approval_grouping_key,
                                })
                                .await;

                            match self.await_tool_approval(&tool_id).await {
                                Ok(ApprovalResult::Approved) => {
                                    emit_tool_audit(json!({
                                        "event": "tool.approval_decision",
                                        "tool_id": tool_id.clone(),
                                        "tool_name": tool_name.clone(),
                                        "decision": "approved",
                                        "caller": caller_type_for_tool_use(tool_caller.as_ref()),
                                    }));
                                    (None, None)
                                }
                                Ok(ApprovalResult::Denied) => {
                                    emit_tool_audit(json!({
                                        "event": "tool.approval_decision",
                                        "tool_id": tool_id.clone(),
                                        "tool_name": tool_name.clone(),
                                        "decision": "denied",
                                        "caller": caller_type_for_tool_use(tool_caller.as_ref()),
                                    }));
                                    (
                                        Some(Err(ToolError::permission_denied(format!(
                                            "Tool '{tool_name}' denied by user"
                                        )))),
                                        None,
                                    )
                                }
                                Ok(ApprovalResult::RetryWithPolicy(policy)) => {
                                    emit_tool_audit(json!({
                                        "event": "tool.approval_decision",
                                        "tool_id": tool_id.clone(),
                                        "tool_name": tool_name.clone(),
                                        "decision": "retry_with_policy",
                                        "policy": format!("{policy:?}"),
                                        "caller": caller_type_for_tool_use(tool_caller.as_ref()),
                                    }));
                                    let elevated_context = tool_registry.map(|r| {
                                        r.context().clone().with_elevated_sandbox_policy(policy)
                                    });
                                    (None, elevated_context)
                                }
                                Err(err) => (Some(Err(err)), None),
                            }
                        } else {
                            (None, None)
                        };

                        // Per-tool snapshot for surgical undo (#384): capture workspace
                        // state before file-modifying tools execute so `/undo` can
                        // revert the most recent write_file/edit_file/apply_patch.
                        if result_override.is_none()
                            && matches!(
                                tool_name.as_str(),
                                "write_file" | "edit_file" | "apply_patch"
                            )
                        {
                            let ws = self.session.workspace.clone();
                            let tid = tool_id.clone();
                            let cap = self.config.snapshots_max_workspace_bytes;
                            let _ = tokio::task::spawn_blocking(move || {
                                crate::core::turn::pre_tool_snapshot(&ws, &tid, cap)
                            })
                            .await;
                        }

                        let started_at = Instant::now();
                        let mut result = if let Some(result_override) = result_override {
                            result_override
                        } else {
                            Self::execute_tool_with_lock(
                                tool_exec_lock.clone(),
                                plan.supports_parallel,
                                plan.interactive,
                                self.tx_event.clone(),
                                tool_name.clone(),
                                tool_input.clone(),
                                tool_registry,
                                mcp_pool.clone(),
                                context_override,
                            )
                            .await
                        };

                        // #500: spill outsized tool outputs to disk before the
                        // result fans out to the model context and the UI cell.
                        // Both consumers see the same artifact reference block +
                        // metadata pointing at the session-owned full file.
                        // Emit a discrete `tool.spillover` audit event so
                        // operators can correlate large-output episodes with
                        // disk-usage growth in `~/.deepseek/tool_outputs/`.
                        if let Ok(tool_result) = result.as_mut()
                            && let Some(path) =
                                crate::tools::truncate::apply_spillover_with_artifact(
                                    tool_result,
                                    &tool_id,
                                    &tool_name,
                                    &self.session.id,
                                )
                        {
                            emit_tool_audit(json!({
                                "event": "tool.spillover",
                                "tool_id": tool_id.clone(),
                                "tool_name": tool_name.clone(),
                                "path": path.display().to_string(),
                            }));
                        }

                        let _ = self
                            .tx_event
                            .send(Event::ToolCallComplete {
                                id: tool_id.clone(),
                                name: tool_name.clone(),
                                result: result.clone(),
                            })
                            .await;

                        outcomes[plan.index] = Some(ToolExecOutcome {
                            index: plan.index,
                            id: tool_id,
                            name: tool_name,
                            input: tool_input,
                            started_at,
                            result,
                        });
                    }
                }
            }

            let mut step_error_count = 0usize;
            // Categorized tool errors collected this step. Feeds the capacity
            // controller's error-escalation checkpoint so it can distinguish
            // (e.g.) a Tool failure that should escalate from a permission
            // denial that should not.
            let mut step_error_categories: Vec<ErrorCategory> = Vec::new();
            let mut stop_after_plan_tool = false;
            let mut loop_guard_halt: Option<String> = None;

            for outcome in outcomes.into_iter().flatten() {
                let duration = outcome.started_at.elapsed();
                let tool_input = outcome.input.clone();
                let tool_name_for_ws = outcome.name.clone();
                let mut tool_call =
                    TurnToolCall::new(outcome.id.clone(), outcome.name.clone(), outcome.input);
                let should_stop_this_turn =
                    should_stop_after_plan_tool(mode, &outcome.name, &outcome.result);

                match outcome.result {
                    Ok(output) => {
                        match loop_guard.record_outcome(&outcome.name, output.success) {
                            OutcomeDecision::Continue => {}
                            OutcomeDecision::Warn(message) => {
                                crate::logging::warn(message.clone());
                                let _ = self.tx_event.send(Event::status(message)).await;
                            }
                            OutcomeDecision::Halt(message) => {
                                loop_guard_halt.get_or_insert(message);
                            }
                        }
                        emit_tool_audit(json!({
                            "event": "tool.result",
                            "tool_id": outcome.id.clone(),
                            "tool_name": outcome.name.clone(),
                            "success": output.success,
                        }));
                        let output_for_context = compact_tool_result_for_context(
                            &self.session.model,
                            &outcome.name,
                            &output,
                        );
                        let tool_was_executed = output
                            .metadata
                            .as_ref()
                            .and_then(|metadata| metadata.get("executed"))
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true);
                        let output_content = output.content;

                        tool_call.set_result(output_content.clone(), duration);
                        self.session.working_set.observe_tool_call(
                            &tool_name_for_ws,
                            &tool_input,
                            Some(&output_for_context),
                            &self.session.workspace,
                        );

                        // #136: post-edit LSP diagnostics hook. We only run
                        // this on success — failed edits leave the file
                        // untouched, so polling for diagnostics would just
                        // surface stale state.
                        if output.success && tool_was_executed {
                            self.run_post_edit_lsp_hook(&outcome.name, &tool_input)
                                .await;
                        }

                        self.add_session_message(Message {
                            role: "user".to_string(),
                            content: vec![ContentBlock::ToolResult {
                                tool_use_id: outcome.id,
                                content: output_for_context,
                                is_error: None,
                                content_blocks: None,
                            }],
                        })
                        .await;
                    }
                    Err(e) => {
                        match loop_guard.record_outcome(&outcome.name, false) {
                            OutcomeDecision::Continue => {}
                            OutcomeDecision::Warn(message) => {
                                crate::logging::warn(message.clone());
                                let _ = self.tx_event.send(Event::status(message)).await;
                            }
                            OutcomeDecision::Halt(message) => {
                                loop_guard_halt.get_or_insert(message);
                            }
                        }
                        let envelope: ErrorEnvelope = e.clone().into();
                        emit_tool_audit(json!({
                            "event": "tool.result",
                            "tool_id": outcome.id.clone(),
                            "tool_name": outcome.name.clone(),
                            "success": false,
                            "error": e.to_string(),
                            "category": envelope.category.to_string(),
                            "severity": envelope.severity.to_string(),
                        }));
                        step_error_count += 1;
                        step_error_categories.push(envelope.category);
                        let error = format_tool_error(&e, &outcome.name);
                        tool_call.set_error(error.clone(), duration);
                        self.session.working_set.observe_tool_call(
                            &tool_name_for_ws,
                            &tool_input,
                            Some(&error),
                            &self.session.workspace,
                        );
                        self.add_session_message(Message {
                            role: "user".to_string(),
                            content: vec![ContentBlock::ToolResult {
                                tool_use_id: outcome.id,
                                content: format!("Error: {error}"),
                                is_error: Some(true),
                                content_blocks: None,
                            }],
                        })
                        .await;
                    }
                }

                turn.record_tool_call(tool_call);
                stop_after_plan_tool |= should_stop_this_turn;
            }

            if stop_after_plan_tool {
                break;
            }

            if let Some(message) = loop_guard_halt {
                crate::logging::warn(message.clone());
                let _ = self.tx_event.send(Event::status(message)).await;
                break;
            }

            if self
                .run_capacity_post_tool_checkpoint(
                    turn,
                    mode,
                    tool_registry,
                    tool_exec_lock.clone(),
                    mcp_pool.clone(),
                    step_error_count,
                    consecutive_tool_error_steps,
                )
                .await
            {
                turn.next_step();
                continue;
            }

            if !pending_steers.is_empty() {
                for steer in pending_steers.drain(..) {
                    self.session
                        .working_set
                        .observe_user_message(&steer, &self.session.workspace);
                    self.add_session_message(self.user_text_message_with_turn_metadata(steer))
                        .await;
                }
            }

            if step_error_count > 0 {
                consecutive_tool_error_steps = consecutive_tool_error_steps.saturating_add(1);
            } else {
                consecutive_tool_error_steps = 0;
            }

            if self
                .run_capacity_error_escalation_checkpoint(
                    turn,
                    mode,
                    step_error_count,
                    consecutive_tool_error_steps,
                    &step_error_categories,
                )
                .await
            {
                turn.next_step();
                continue;
            }

            turn.next_step();
        }

        if self.cancel_token.is_cancelled() {
            return (TurnOutcomeStatus::Interrupted, None);
        }
        if let Some(err) = turn_error {
            return (TurnOutcomeStatus::Failed, Some(err));
        }
        (TurnOutcomeStatus::Completed, None)
    }

    pub(super) fn messages_with_turn_metadata(&self) -> Vec<Message> {
        // `<turn_meta>` is stored on user-text messages when the message is
        // appended. Do not rewrite historical messages at request time: doing
        // so makes the API prefix differ from the bytes sent in earlier turns
        // and destroys DeepSeek's KV prefix cache reuse.
        self.session.messages.clone()
    }
}

fn subagent_completion_runtime_message(payload: &str) -> Message {
    Message {
        role: "system".to_string(),
        content: vec![ContentBlock::Text {
            text: format!(
                "<deepseek:runtime_event kind=\"subagent_completion\" visibility=\"internal\">\n\
This is an internal runtime event, not user input. Use the sub-agent completion \
data below to continue coordinating the current task. Do not tell the user they \
pasted sentinels, do not explain the sentinel protocol, and do not quote the raw \
XML unless the user explicitly asks to debug sub-agent internals.\n\n\
{payload}\n\
</deepseek:runtime_event>"
            ),
            cache_control: None,
        }],
    }
}

fn should_hold_turn_for_subagents(queued_completions: usize, running_children: usize) -> bool {
    queued_completions > 0 || running_children > 0
}

/// Resolve an `"auto"` reasoning-effort tier to a concrete value.
///
/// When the configured effort is `"auto"`, inspects the last user message
/// and calls [`crate::auto_reasoning::select`] to pick the actual tier.
/// Non-`"auto"` values pass through unchanged.
fn resolve_auto_effort(reasoning_effort: Option<&str>, messages: &[Message]) -> Option<String> {
    match reasoning_effort {
        Some("auto") => {
            // Find the last user message in the conversation.
            let last_msg = messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| {
                    m.content
                        .iter()
                        .filter_map(|block| {
                            if let ContentBlock::Text { text, .. } = block {
                                if is_turn_metadata_text(text) {
                                    None
                                } else {
                                    Some(text.as_str())
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<&str>>()
                        .join(" ")
                })
                .unwrap_or_default();

            // is_subagent is false here — handle_deepseek_turn runs in the
            // main engine (not a sub-agent's inner loop). Sub-agents have
            // their own turn pass and can pass is_subagent=true when they
            // call this function directly.
            let tier = crate::auto_reasoning::select(false, &last_msg);
            let resolved = tier.as_setting().to_string();
            tracing::debug!(
                reasoning_effort = %resolved,
                is_subagent = false,
                "auto_reasoning: resolved auto tier from user message"
            );
            Some(resolved)
        }
        Some(other) => Some(other.to_string()),
        None => None,
    }
}

fn is_turn_metadata_text(text: &str) -> bool {
    text.trim_start().starts_with("<turn_meta>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_completion_handoff_is_internal_system_message() {
        let message = subagent_completion_runtime_message(
            "Build passed\n<deepseek:subagent.done>{\"agent_id\":\"agent_a\"}</deepseek:subagent.done>",
        );

        assert_eq!(message.role, "system");
        let text = match &message.content[0] {
            ContentBlock::Text { text, .. } => text,
            other => panic!("expected text block, got {other:?}"),
        };
        assert!(text.contains("internal runtime event, not user input"));
        assert!(text.contains("Do not tell the user they pasted sentinels"));
        assert!(text.contains("<deepseek:subagent.done>"));
        assert!(text.contains("Build passed"));
    }

    #[test]
    fn turn_holds_open_for_running_or_completed_subagents() {
        assert!(should_hold_turn_for_subagents(1, 0));
        assert!(should_hold_turn_for_subagents(0, 1));
        assert!(!should_hold_turn_for_subagents(0, 0));
    }

    /// Regression test for the OpenAI streaming batch tool_calls bug.
    ///
    /// Background: when an OpenAI-compatible backend (vLLM, Ollama, LM Studio,
    /// etc.) streams a response containing multiple `tool_calls` in the same
    /// assistant message, the streaming parser emits the events in this order:
    ///
    /// ```text
    /// ContentBlockStart::ToolUse { index: 0, .. }   // tool #1
    /// ContentBlockDelta { index: 0, .. }            // its arguments
    /// ContentBlockStart::ToolUse { index: 1, .. }   // tool #2
    /// ContentBlockDelta { index: 1, .. }
    /// …
    /// ContentBlockStart::ToolUse { index: N-1, .. }
    /// ContentBlockDelta { index: N-1, .. }
    /// ContentBlockStop { index: 0 }                 // ── only flushed at
    /// ContentBlockStop { index: 1 }                 //    finish_reason
    /// …                                             //    (see chat.rs
    /// ContentBlockStop { index: N-1 }               //    L2050-L2064)
    /// ```
    ///
    /// All Starts arrive before any Stop. The fix replaces the single
    /// `current_tool_index: Option<usize>` slot (overwritten by each Start)
    /// with a `HashMap<u32 block_index, usize tool_uses_idx>` that survives
    /// every Start and routes each Stop to the right `tool_uses` entry.
    ///
    /// This test confirms the invariant: feed 7 Starts then 7 Stops, expect
    /// all 7 indices to come back out in order.
    #[test]
    fn batch_tool_calls_preserve_all_tool_use_indices() {
        let mut current_tool_indices: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();

        // Simulate `ContentBlockStart::ToolUse { index: i }` for 7 tools.
        for block_index in 0..7u32 {
            current_tool_indices.insert(block_index, block_index as usize);
        }
        assert_eq!(current_tool_indices.len(), 7);

        // Now drain via `ContentBlockStop { index: i }` in the same order.
        let mut recovered: Vec<(u32, usize)> = (0..7u32)
            .map(|block_index| {
                let tool_idx = current_tool_indices
                    .remove(&block_index)
                    .expect("each block_index must route to a tool_uses entry");
                (block_index, tool_idx)
            })
            .collect();
        recovered.sort_by_key(|(block_index, _)| *block_index);
        let expected: Vec<(u32, usize)> = (0..7u32).map(|i| (i, i as usize)).collect();
        assert_eq!(
            recovered, expected,
            "every Stop must recover the tool_uses index pushed by its matching Start"
        );
        assert!(
            current_tool_indices.is_empty(),
            "all entries must drain after their Stops"
        );
    }

    #[test]
    fn loop_guard_block_tool_result_counts_as_failure() {
        let result = loop_guard_block_tool_result("Blocked: repeated call".to_string());

        assert!(
            !result.success,
            "LoopGuard blocks must count as tool failures so repeated blocked calls can trip halt handling"
        );
        assert_eq!(
            result
                .metadata
                .as_ref()
                .and_then(|m| m.get("loop_guard"))
                .and_then(|v| v.as_str()),
            Some("identical_tool_call")
        );
    }

    #[test]
    fn resolve_auto_effort_ignores_stored_turn_metadata() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "<turn_meta>\nRecent errors: src/failing.rs\n</turn_meta>".to_string(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                },
            ],
        }];

        assert_eq!(
            resolve_auto_effort(Some("auto"), &messages),
            Some("high".to_string()),
            "auto thinking should classify the user request, not stored metadata"
        );
    }
}
