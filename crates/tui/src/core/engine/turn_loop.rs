//! Main streaming turn loop for the engine.
//!
//! Extracted from `core/engine.rs` for issue #74. This module keeps the
//! existing per-turn orchestration intact: request construction, streaming
//! event handling, tool planning/execution, LSP post-edit hooks, capacity
//! checkpoints, and loop termination.

use super::stuck_guard::{
    RUNTIME_NOTICE as STUCK_RUNTIME_NOTICE, StepFingerprint, StuckGuard, StuckSignal,
};
use super::*;
use crate::core::ops::UserInputProvenance;
use crate::prompt_zones::PinnedPrefix;
use crate::runtime_handoff::{
    subagent_completion_runtime_message, waiting_for_subagents_runtime_message,
};

const MAX_APPROVAL_INTENT_SUMMARY_CHARS: usize = 2_000;
const TOOL_ERROR_DEGRADATION_THRESHOLD: u32 = 2;

fn approval_intent_summary(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut chars = trimmed.chars();
    let mut summary = chars
        .by_ref()
        .take(MAX_APPROVAL_INTENT_SUMMARY_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        summary.push_str("...");
    }
    Some(summary)
}

pub(super) fn registered_tool_approval_required(
    tool_name: &str,
    requirement: ApprovalRequirement,
    auto_approve: bool,
) -> bool {
    if requirement == ApprovalRequirement::Auto {
        return false;
    }
    if registered_tool_requires_non_bypassable_approval(tool_name) {
        return true;
    }
    !auto_approve
}

pub(super) fn tool_error_degradation_runtime_hint(
    consecutive_tool_error_steps: u32,
    step_error_tool_names: &[String],
    step_error_categories: &[ErrorCategory],
    step_error_tool_inputs: &[serde_json::Value],
) -> Option<String> {
    if consecutive_tool_error_steps < TOOL_ERROR_DEGRADATION_THRESHOLD {
        return None;
    }
    if !step_error_categories
        .iter()
        .any(|category| tool_error_category_allows_degradation(*category))
    {
        return None;
    }

    let mut tool_names = step_error_tool_names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    tool_names.sort_unstable();
    tool_names.dedup();
    let tools = if tool_names.is_empty() {
        "tools".to_string()
    } else {
        tool_names.join(", ")
    };

    let mut hint = format!(
        "Tool calls have failed for {consecutive_tool_error_steps} consecutive steps ({tools}). \
do not repeat the same call unchanged; switch to an alternate tool or source, narrow the request, \
or ask for the required input before trying again."
    );
    if let Some(direct_url_hint) =
        direct_url_pattern_fallback_hint(step_error_tool_names, step_error_tool_inputs)
    {
        hint.push(' ');
        hint.push_str(&direct_url_hint);
    }
    Some(hint)
}

fn tool_error_category_allows_degradation(category: ErrorCategory) -> bool {
    matches!(
        category,
        ErrorCategory::Network
            | ErrorCategory::RateLimit
            | ErrorCategory::Timeout
            | ErrorCategory::Tool
    )
}

fn direct_url_pattern_fallback_hint(
    step_error_tool_names: &[String],
    step_error_tool_inputs: &[serde_json::Value],
) -> Option<String> {
    let mut domains = std::collections::BTreeSet::new();
    for (tool_name, input) in step_error_tool_names
        .iter()
        .zip(step_error_tool_inputs.iter())
    {
        if matches!(tool_name.as_str(), "web_search" | "web.run") {
            collect_search_domains(input, &mut domains);
        }
    }

    let domain = domains.into_iter().next()?;
    Some(format!(
        "For blocked search, try fetch_url directly on likely URL patterns such as \
https://{domain}/announcements and https://{domain}/news."
    ))
}

fn collect_search_domains(
    input: &serde_json::Value,
    domains: &mut std::collections::BTreeSet<String>,
) {
    if let Some(values) = input.get("domains").and_then(serde_json::Value::as_array) {
        for value in values {
            if let Some(domain) = value.as_str().and_then(normalize_domain_candidate) {
                domains.insert(domain);
            }
        }
    }
    for key in ["query", "q"] {
        if let Some(query) = input.get(key).and_then(serde_json::Value::as_str) {
            collect_query_domains(query, domains);
        }
    }
    if let Some(searches) = input
        .get("search_query")
        .and_then(serde_json::Value::as_array)
    {
        for search in searches {
            collect_search_domains(search, domains);
        }
    }
}

fn collect_query_domains(query: &str, domains: &mut std::collections::BTreeSet<String>) {
    for token in query.split_whitespace() {
        let token = token.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
            )
        });
        if let Some(site) = token.strip_prefix("site:") {
            if let Some(domain) = normalize_domain_candidate(site) {
                domains.insert(domain);
            }
        } else if let Some(domain) = normalize_domain_candidate(token) {
            domains.insert(domain);
        }
    }
}

fn normalize_domain_candidate(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '<' | '>' | '.' | ',' | ';' | ':'));
    if value.is_empty() {
        return None;
    }
    let without_scheme = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    let looks_like_domain = host.contains('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        && host.rsplit('.').next().is_some_and(|suffix| {
            suffix.len() >= 2 && suffix.chars().any(|c| c.is_ascii_alphabetic())
        });
    if looks_like_domain { Some(host) } else { None }
}

fn registered_tool_requires_non_bypassable_approval(tool_name: &str) -> bool {
    matches!(tool_name, "rlm_eval" | "start_mcp_server")
}

impl Engine {
    fn drain_shell_completion_events(&self) -> Vec<crate::tools::shell::ShellCompletionEvent> {
        self.shell_manager
            .lock()
            .map(|mut manager| manager.drain_finished_jobs())
            .unwrap_or_default()
    }

    async fn drain_subagent_completion_events(&mut self, status_label: &str) -> usize {
        let mut completions: Vec<crate::tools::subagent::SubAgentCompletion> = Vec::new();
        while let Ok(completion) = self.rx_subagent_completion.try_recv() {
            if let Some(completion) = super::claim_subagent_completion(
                &mut self.delivered_subagent_completion_ids,
                completion,
            ) {
                completions.push(completion);
            }
        }

        let synthesized = {
            let manager = self.subagent_manager.read().await;
            manager.terminal_results_excluding(&self.delivered_subagent_completion_ids)
        };
        for result in synthesized {
            let completion = crate::tools::subagent::subagent_completion_from_result(&result);
            if let Some(completion) = super::claim_subagent_completion(
                &mut self.delivered_subagent_completion_ids,
                completion,
            ) {
                completions.push(completion);
            }
        }

        let count = completions.len();
        if count == 0 {
            return 0;
        }

        for completion in completions {
            self.add_session_message(subagent_completion_runtime_message(&completion.payload))
                .await;
        }
        let prefix = if status_label.is_empty() {
            String::new()
        } else {
            format!("{status_label} ")
        };
        let _ = self
            .tx_event
            .send(Event::status(format!(
                "Resuming turn with {count} {prefix}sub-agent completion(s)"
            )))
            .await;
        count
    }

    pub(super) async fn handle_deepseek_turn(
        &mut self,
        turn: &mut TurnContext,
        tool_registry: Option<&crate::tools::ToolRegistry>,
        tools: Option<Vec<Tool>>,
        mode: AppMode,
        force_update_plan_first: bool,
        dynamic_active_tools: Vec<&'static str>,
    ) -> (TurnOutcomeStatus, Option<String>) {
        // Only interactive TUI hosts own terminal chrome. Headless exec,
        // app-server, and stream-json stdout must remain byte-clean.
        if self.config.terminal_chrome_enabled {
            crate::tui::notifications::set_taskbar_progress_busy();
            crate::tui::notifications::start_title_animation("Codewhale");
        }

        let client = self
            .model_client
            .clone()
            .expect("model client should be configured");

        let mut consecutive_tool_error_steps = 0u32;
        let mut stuck_guard = StuckGuard::default();
        let mut turn_error: Option<String> = None;
        let mut context_recovery_attempts = 0u8;
        let mut tool_catalog = tools.unwrap_or_default();
        if !tool_catalog.is_empty() {
            ensure_advanced_tooling(&mut tool_catalog, mode, &self.config.tools_always_load);
        }
        if let Some(registry) = tool_registry {
            let issues = tool_catalog_consistency_issues(&tool_catalog, registry);
            if !issues.is_empty() {
                tracing::warn!(
                    target: "engine.tool_catalog",
                    ?issues,
                    "model/search tool catalog is inconsistent with the runtime registry"
                );
            }
        }
        let mut active_tool_names = initial_active_tools(&tool_catalog);
        active_tool_names.extend(
            dynamic_active_tools
                .into_iter()
                .map(std::string::ToString::to_string),
        );
        let mut goal_continuations_this_turn = 0u32;
        // Outer stream-retry counter: when the chunked-transfer connection
        // dies mid-stream and either nothing useful was streamed (#103
        // Phase 3) or the host slept mid-turn (#2990), we silently re-issue
        // the SAME request up to MAX_STREAM_RETRIES times before surfacing
        // the failure to the user.
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

            // Child agents can finish while the parent model is still taking
            // tool steps. Surface queued completions before the next provider
            // request so the parent can use them immediately instead of
            // discovering them only when it eventually emits no more tools or
            // the idle handler starts a separate follow-up turn.
            self.drain_subagent_completion_events("queued").await;

            // Ensure system prompt is up to date with latest session states
            self.refresh_system_prompt();

            if turn.at_max_steps() {
                let _ = self
                    .tx_event
                    .send(Event::status("Reached maximum steps"))
                    .await;
                break;
            }

            let compaction_pins =
                self.compaction_pins_for_active_turn(turn.active_slop_gate_message.as_ref());
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
                    client.as_ref(),
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
                            self.session.replace_messages(result.messages);
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

            if let Some(input_budget) = context_input_budget_for_route(
                self.api_provider,
                &self.session.model,
                self.active_route_limits,
                0,
            ) {
                let estimated_input = self.estimated_input_tokens();
                if estimated_input > input_budget {
                    if context_recovery_attempts >= MAX_CONTEXT_RECOVERY_ATTEMPTS {
                        let message = format!(
                            "Context remains above model limit after {MAX_CONTEXT_RECOVERY_ATTEMPTS} recovery attempts \
                             (~{estimated_input} token estimate, ~{input_budget} budget). Please run /compact or /clear."
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
                            client.as_ref(),
                            "preflight token budget",
                            turn.active_slop_gate_message.as_ref(),
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
            let force_update_plan_this_step = force_update_plan_first && !turn.has_tool_calls();
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
                self.api_provider,
                &self.api_config.deepseek_base_url(),
                &self.config.model,
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
                        let pinned_hash = pm
                            .pinned_fingerprint()
                            .map(|fp| fp.combined_sha256.clone())
                            .unwrap_or_default();
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
                                pinned_combined_hash: pinned_hash,
                            })
                            .await;
                    }
                    Ok(_) => {
                        let pinned_hash = pm
                            .pinned_fingerprint()
                            .map(|fp| fp.combined_sha256.clone())
                            .unwrap_or_default();
                        // Stable check — keep the TUI counter in sync.
                        let _ = self
                            .tx_event
                            .send(Event::PrefixCacheChange {
                                description: String::new(),
                                system_prompt_changed: false,
                                tools_changed: false,
                                stability_pct: (pm.stability_ratio() * 100.0).round() as u32,
                                changed: false,
                                pinned_combined_hash: pinned_hash,
                            })
                            .await;
                    }
                }
            }

            // Three-zone prefix contract (#2264): freeze baseline on first
            // turn, verify against it on subsequent turns. Operates alongside
            // PrefixStabilityManager as an independent diagnostic layer.
            // Phase 3: emit a one-shot 'frozen' event on first turn.
            // Drift is logged (tracing::debug!) but not re-emitted —
            // PrefixStabilityManager already reports the change above.
            let system_text =
                crate::prefix_cache::system_prompt_text(self.session.system_prompt.as_ref());
            let current_tools: &[crate::models::Tool] = active_tools.as_deref().unwrap_or_default();

            match &self.session.frozen_prefix {
                Some(frozen) => {
                    if let Err(drift) = frozen.verify(&system_text, current_tools) {
                        tracing::debug!(
                            target: "prefix_cache",
                            "three-zone drift: {drift}"
                        );
                        let pinned = PinnedPrefix::new(
                            self.session.system_prompt.as_ref(),
                            current_tools.to_vec(),
                        );
                        self.session.frozen_prefix = Some(pinned.freeze());
                    }
                }
                None => {
                    let pinned = PinnedPrefix::new(
                        self.session.system_prompt.as_ref(),
                        current_tools.to_vec(),
                    );
                    let frozen = pinned.freeze();
                    let _ = self
                        .tx_event
                        .send(Event::PrefixCacheChange {
                            description: format!("frozen: {}", frozen.short_id()),
                            system_prompt_changed: false,
                            tools_changed: false,
                            stability_pct: 100,
                            changed: false,
                            pinned_combined_hash: frozen.hash().to_string(),
                        })
                        .await;
                    self.session.frozen_prefix = Some(frozen);
                }
            }

            let request = MessageRequest {
                model: self.session.model.clone(),
                messages: self.messages_with_turn_metadata(),
                max_tokens: effective_max_output_tokens_for_route(
                    self.api_provider,
                    &self.session.model,
                    self.active_route_limits,
                ),
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
            let stream_result = tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    let _ = self.tx_event.send(Event::status("Request cancelled")).await;
                    return (TurnOutcomeStatus::Interrupted, None);
                }
                result = client.create_message_stream(stream_request.clone()) => result,
            };
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
                                client.as_ref(),
                                "provider context-length rejection",
                                turn.active_slop_gate_message.as_ref(),
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
            // #3014: Anthropic signed-thinking signature for the current
            // thinking block; must be replayed verbatim in tool loops.
            let mut current_thinking_signature: Option<String> = None;
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
            let mut tool_call_filter = ToolCallDeltaFilterState::default();
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
            // #2990 sleep-resume bookkeeping: monotonic and wall-clock stamps
            // of the last stream progress. `Instant` pauses across a host
            // suspend while `SystemTime` does not, so a large divergence on
            // the next error tells "machine slept" apart from "network died".
            let mut last_progress_mono = Instant::now();
            let mut last_progress_wall = std::time::SystemTime::now();
            let mut sleep_resume_pending = false;
            let mut stream_content_bytes: usize = 0;
            let (chunk_timeout_secs, chunk_timeout) = stream_chunk_timeout_budget(&self.config);
            let max_duration = Duration::from_secs(STREAM_MAX_DURATION_SECS);

            // Process stream events
            loop {
                let poll_outcome = tokio::select! {
                    biased;
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
                        last_progress_mono = Instant::now();
                        last_progress_wall = std::time::SystemTime::now();
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
                        // #2990: wall-clock far ahead of the monotonic clock
                        // since the last chunk means the host slept mid-stream.
                        // The partial output predates the sleep and the user
                        // was not watching — schedule a full request retry in
                        // the post-loop block instead of failing the turn.
                        let wall_elapsed = last_progress_wall
                            .elapsed()
                            .unwrap_or_else(|_| last_progress_mono.elapsed());
                        if should_resume_after_sleep(
                            sleep_gap_detected(last_progress_mono.elapsed(), wall_elapsed),
                            stream_retry_attempts,
                            self.cancel_token.is_cancelled(),
                        ) {
                            crate::logging::warn(format!(
                                "Stream error after suspected system sleep ({:?} monotonic vs {:?} wall since last chunk); scheduling request retry: {message}",
                                last_progress_mono.elapsed(),
                                wall_elapsed,
                            ));
                            sleep_resume_pending = true;
                            break;
                        }
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
                                "Transparent stream retry {transparent_stream_retries}/{MAX_TRANSPARENT_STREAM_RETRIES} (no content received yet): {message}",
                            ));
                            // Drop the failed stream before issuing the new
                            // request to release the underlying connection.
                            drop(stream);
                            let retry_stream_result = tokio::select! {
                                biased;
                                () = self.cancel_token.cancelled() => break,
                                result = client.create_message_stream(stream_request.clone()) => result,
                            };
                            match retry_stream_result {
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
                        let user_message =
                            stream_read_error_user_message(&message, any_content_received);
                        turn_error.get_or_insert(user_message.clone());
                        let _ = self
                            .tx_event
                            .send(Event::error(ErrorEnvelope::classify(user_message, true)))
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
                            tool_call_filter = ToolCallDeltaFilterState::default();
                            let filtered = filter_tool_call_delta_with_state(
                                &current_text_raw,
                                &mut tool_call_filter,
                            );
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
                                "Tool '{name}' block start. Initial input: {input:?}"
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
                                input_parse_error: None,
                            });
                        }
                        ContentBlockStart::ServerToolUse { id, name, input } => {
                            crate::logging::info(format!(
                                "Server tool '{name}' block start. Initial input: {input:?}"
                            ));
                            current_block_kind = Some(ContentBlockKind::ToolUse);
                            current_tool_indices.insert(index, tool_uses.len());
                            tool_uses.push(ToolUseState {
                                id,
                                name,
                                input,
                                caller: None,
                                input_buffer: String::new(),
                                input_parse_error: None,
                            });
                        }
                    },
                    StreamEvent::ContentBlockDelta { index, delta } => match delta {
                        Delta::TextDelta { text } => {
                            stream_content_bytes = stream_content_bytes.saturating_add(text.len());
                            current_text_raw.push_str(&text);
                            let filtered =
                                filter_tool_call_delta_with_state(&text, &mut tool_call_filter);
                            if !fake_wrapper_notice_emitted
                                && filtered.len() < text.len()
                                && contains_fake_tool_wrapper(&current_text_raw)
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
                        Delta::SignatureDelta { signature } => {
                            // #3014: capture (and concatenate, defensively)
                            // the signed-thinking signature for replay.
                            match current_thinking_signature.as_mut() {
                                Some(existing) => existing.push_str(&signature),
                                None => current_thinking_signature = Some(signature),
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
                                let flushed = flush_tool_call_delta_state(&mut tool_call_filter);
                                if !flushed.is_empty() {
                                    current_text_visible.push_str(&flushed);
                                    let _ = self
                                        .tx_event
                                        .send(Event::MessageDelta {
                                            index: index as usize,
                                            content: flushed,
                                        })
                                        .await;
                                }
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
                                    let error =
                                        malformed_tool_arguments_error(&tool_state.input_buffer);
                                    tool_state.input_parse_error = Some(error);
                                    tool_state.input =
                                        malformed_tool_arguments_input(&tool_state.input_buffer);
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
                    StreamEvent::Error { error } => {
                        // #3014: Anthropic SSE error event. The adapter
                        // surfaces fatal errors as stream Err items; this
                        // defensive arm keeps any passed-through error
                        // visible instead of silently dropped.
                        crate::logging::warn(format!("Provider stream error event: {error}"));
                        stream_errors += 1;
                    }
                }
            }

            if self.cancel_token.is_cancelled() {
                let _ = self.tx_event.send(Event::status("Request cancelled")).await;
                return (TurnOutcomeStatus::Interrupted, None);
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
            if stream_died_with_nothing || sleep_resume_pending {
                if stream_retry_attempts < MAX_STREAM_RETRIES {
                    stream_retry_attempts = stream_retry_attempts.saturating_add(1);
                    if sleep_resume_pending {
                        crate::logging::warn(format!(
                            "Resuming after system sleep (attempt {stream_retry_attempts}/{MAX_STREAM_RETRIES}); discarding partial output and retrying request"
                        ));
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "System sleep detected; connection lost — retrying request ({stream_retry_attempts}/{MAX_STREAM_RETRIES})"
                            )))
                            .await;
                        // Finalize any partially-rendered assistant cell so
                        // the retried stream renders fresh instead of
                        // appending to the pre-sleep fragment.
                        if pending_message_complete {
                            let index = last_text_index.unwrap_or(0);
                            let _ = self.tx_event.send(Event::MessageComplete { index }).await;
                        }
                    } else {
                        crate::logging::warn(format!(
                            "Stream died with no content (attempt {stream_retry_attempts}/{MAX_STREAM_RETRIES}); retrying request"
                        ));
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Connection interrupted; retrying ({stream_retry_attempts}/{MAX_STREAM_RETRIES})"
                            )))
                            .await;
                    }
                    // Don't preserve the per-stream `turn_error` — we're
                    // about to retry, and a successful retry should not
                    // surface the transient error as the turn outcome.
                    turn_error = None;
                    continue;
                }
                crate::logging::warn(format!(
                    "Stream retry budget exhausted ({stream_retry_attempts} attempts); failing turn"
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
                content_blocks.push(ContentBlock::Thinking {
                    thinking,
                    signature: current_thinking_signature.clone(),
                });
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
                        input_parse_error: None,
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

            // Issue #1727: did this turn produce ONLY a reasoning/thinking
            // block — empty content, no tool calls (e.g. gpt-oss via ollama's
            // harmony→OpenAI shim mapping to `reasoning_content`)? We do NOT
            // surface anything here: after this point the same turn can still
            // CONTINUE for pending steers (~below) or sub-agent completions,
            // and emitting now would show a spurious "turn ended" notice right
            // before the turn resumes. Capture the fact and decide later, at
            // the point the turn is certain to be finishing with no sendable
            // content (see the `tool_uses.is_empty()` tail).
            let thinking_only_no_sendable = !has_sendable_assistant_content;

            // Add assistant message to session
            if has_sendable_assistant_content {
                self.add_session_message(Message {
                    role: "assistant".to_string(),
                    content: content_blocks,
                })
                .await;
            }

            if tool_uses.is_empty() {
                match stuck_guard.observe(StepFingerprint::assistant_no_tool(&current_text_visible))
                {
                    Some(StuckSignal::Warn) => {
                        self.add_session_message(self.runtime_text_message_with_turn_metadata(
                            STUCK_RUNTIME_NOTICE.to_string(),
                            UserInputProvenance::Runtime,
                        ))
                        .await;
                        turn.next_step();
                        continue;
                    }
                    Some(StuckSignal::Stop) => {
                        let reason = "stuck loop detected after repeated no-progress messages";
                        let _ = self.tx_event.send(Event::status(reason)).await;
                        return (TurnOutcomeStatus::Failed, Some(reason.to_string()));
                    }
                    None => {}
                }
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

                let shell_completions = self.drain_shell_completion_events();
                if let Some(status) = shell_completion_status_text(&shell_completions, "") {
                    let _ = self.tx_event.send(Event::status(status)).await;
                }

                // Sub-agent completion handoff (issue #756). The model finished
                // streaming with no tool calls — but if it has direct children
                // still running (or completions queued from children that
                // finished while we were inferring), surface their
                // `<codewhale:subagent.done>` sentinels into the transcript and
                // resume instead of ending the turn. This fulfils the contract
                // already documented in `prompts/constitution.md`: the parent is
                // promised it'll see the sentinel when a child finishes.
                let subagent_completions = self.drain_subagent_completion_events("").await;
                if subagent_completions == 0 {
                    // #3216: do NOT barrier the parent on running children.
                    // Launching a sub-agent is not the same as joining it — the
                    // parent ends its turn and stays responsive. Running children
                    // are background work; their results return via the
                    // completion sentinel on a later turn. Stale children are filtered out of
                    // `running_count` by the manager's heartbeat, so they neither
                    // block nor inflate the surfaced count. (Previously the parent
                    // waited in a select! loop here until a completion or the
                    // heartbeat timeout, which read as a hard TUI freeze.)
                    // Cancellation and steering are handled at the top of the step
                    // loop; stale-agent cleanup is the manager's responsibility.
                    let running = {
                        let mgr = self.subagent_manager.read().await;
                        mgr.running_count()
                    };
                    if running > 0 {
                        let _ = self
                            .tx_event
                            .send(Event::status(format!(
                                "Turn ending with {running} sub-agent(s) still running in the background; they'll report when done."
                            )))
                            .await;
                        // Inject a waiting hint so the model does not poll
                        // with peek/status/sleep on the next turn (issue #4097).
                        self.add_session_message(waiting_for_subagents_runtime_message(running))
                            .await;
                    }
                }
                if subagent_completions > 0 {
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
                                    self.runtime_text_message_with_turn_metadata(
                                        feedback,
                                        UserInputProvenance::Runtime,
                                    ),
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
                                    self.runtime_text_message_with_turn_metadata(
                                        format!("[REPL round {round_num} execution failed]\n{e}"),
                                        UserInputProvenance::Runtime,
                                    ),
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

                // Issue #1727: the turn is now genuinely finishing with no
                // sendable content. Control only reaches here when there were
                // no pending steers (`continue`d above), no sub-agent
                // completions to resume with, and we were not holding for
                // running children (the `should_hold_turn_for_subagents`
                // branch above would have awaited / `continue`d / returned).
                // If the assistant produced ONLY a reasoning block, the prior
                // code fell straight through to this `break`, emitting nothing
                // and leaving the UI spinner hung. Surface a status now —
                // safe because the turn can no longer resume.
                // #1961: Before breaking, drain any sub-agent completions that
                // arrived between the last hold check and now. If a child finished
                // while we were running the thinking-only check, surface its
                // sentinel rather than delaying it to the next turn.
                let late_shell_completions = self.drain_shell_completion_events();
                if let Some(status) = shell_completion_status_text(&late_shell_completions, "late")
                {
                    let _ = self.tx_event.send(Event::status(status)).await;
                }

                if self.drain_subagent_completion_events("late").await > 0 {
                    turn.next_step();
                    continue;
                }

                if let Some(continuation) = self
                    .goal_continuation_message_if_needed(
                        tool_registry,
                        &mut goal_continuations_this_turn,
                    )
                    .await
                {
                    self.add_session_message(self.runtime_text_message_with_turn_metadata(
                        continuation,
                        UserInputProvenance::Runtime,
                    ))
                    .await;
                    turn.next_step();
                    continue;
                }

                if thinking_only_no_sendable {
                    let holding_for_subagents = {
                        let running = {
                            let mgr = self.subagent_manager.read().await;
                            mgr.running_count()
                        };
                        should_hold_turn_for_subagents(0, running)
                    };
                    if should_emit_thinking_only_status(
                        tool_uses.is_empty(),
                        turn_error.is_none(),
                        self.cancel_token.is_cancelled(),
                        !pending_steers.is_empty(),
                        holding_for_subagents,
                    ) {
                        let message = "Model returned reasoning but no answer or tool call; \
                                       turn ended without output. Send a follow-up to retry."
                            .to_string();
                        crate::logging::warn(&message);
                        let _ = self.tx_event.send(Event::status(message)).await;
                    }
                }

                break;
            }

            // Execute tools
            if self.shared_paused.lock().is_ok_and(|paused| *paused) {
                let _ = self
                    .tx_event
                    .send(Event::status("Request was Paused"))
                    .await;
                return (TurnOutcomeStatus::Interrupted, None);
            }

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
            // #3026: `additionalContext` strings from tool_call_before hooks,
            // keyed by tool id; appended to the tool result sent to the model.
            let mut hook_contexts: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut plans: Vec<ToolExecutionPlan> = Vec::with_capacity(tool_uses.len());
            for (index, tool) in tool_uses.iter_mut().enumerate() {
                let tool_id = tool.id.clone();
                let mut tool_name = tool.name.clone();
                let mut tool_input = tool.input.clone();
                let tool_caller = tool.caller.clone();
                crate::logging::info(format!(
                    "Planning tool '{tool_name}' with input: {tool_input:?}"
                ));

                let requested_tool_name = tool_name.clone();
                let tool_def =
                    resolve_tool_definition(&mut tool_name, &tool_catalog, tool_registry);
                if requested_tool_name != tool_name {
                    tool.name = tool_name.clone();
                }

                let interactive = (tool_name == "exec_shell"
                    && tool_input
                        .get("interactive")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true))
                    || tool_name == REQUEST_USER_INPUT_NAME;

                let mut approval_required = false;
                let mut approval_description = "Tool execution requires approval".to_string();
                let mut approval_force_prompt = false;
                let mut supports_parallel = false;
                let mut read_only = false;
                let mut detached_start = false;
                let mut resources = vec![ResourceClaim::GlobalExclusive];
                let mut blocked_error: Option<ToolError> = None;
                let guard_result: Option<ToolResult> = None;
                // #3026: set by a hook `ask` decision; applied AFTER the
                // registry-based approval computation below so it cannot be
                // clobbered by it.
                let mut hook_requires_approval = false;

                if mode_blocks_command_execution(mode, &tool_name) {
                    blocked_error = Some(ToolError::permission_denied(format!(
                        "'{tool_name}' is not available in Plan mode — switch to Act mode (`/mode act`) to run commands and code."
                    )));
                }

                if blocked_error.is_none()
                    && let Some(error) = tool.input_parse_error.clone()
                {
                    blocked_error = Some(ToolError::invalid_input(error));
                }

                // #3027: deny wins over allow — check the deny-list first so a
                // tool present in both lists is still blocked.
                if blocked_error.is_none()
                    && command_denies_tool(self.config.disallowed_tools.as_deref(), &tool_name)
                {
                    blocked_error = Some(ToolError::permission_denied(format!(
                        "Tool '{tool_name}' is in the disallowed-tools list"
                    )));
                }

                if blocked_error.is_none()
                    && !command_allows_tool(self.config.allowed_tools.as_deref(), &tool_name)
                {
                    blocked_error = Some(ToolError::permission_denied(format!(
                        "Tool '{tool_name}' is not in the allowed-tools list for the current command"
                    )));
                }

                if blocked_error.is_none()
                    && !caller_allowed_for_tool(tool_caller.as_ref(), tool_def)
                {
                    blocked_error = Some(ToolError::permission_denied(format!(
                        "Tool '{tool_name}' does not allow caller '{}'",
                        caller_type_for_tool_use(tool_caller.as_ref())
                    )));
                }

                // Fail closed: a tool with no execution path — not MCP, not
                // code/js/search, and with no registry spec — must be blocked,
                // NOT run unguarded. Previously this only checked
                // `tool_def.is_none()`, so a tool present in the model-facing
                // catalog but absent from the execution registry (or when the
                // registry itself is None) fell through every approval branch
                // with approval_required=false and executed with no gate.
                let registry_has_spec =
                    tool_registry.is_some_and(|registry| registry.get(&tool_name).is_some());
                if blocked_error.is_none()
                    && !registry_has_spec
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

                // Prepare before hooks so every input-specific authority and
                // scheduling field has one inspectable owner. Preparation is
                // side-effect free; execution remains below the full gate
                // stack exactly as before.
                let mut prepared_policy = match prepare_tool_call(
                    &tool_name,
                    tool_input.clone(),
                    tool_registry,
                    self.session.auto_approve,
                ) {
                    Ok(policy) => Some(policy),
                    Err(error) => {
                        if blocked_error.is_none() {
                            blocked_error = Some(error);
                        }
                        None
                    }
                };
                let mut reprepared_after_hook = false;

                if blocked_error.is_none()
                    && let Some(hook_executor) = self.config.hook_executor.as_ref()
                    && hook_executor.has_hooks_for_event(crate::hooks::HookEvent::ToolCallBefore)
                {
                    // Warn if any ToolCallBefore hook is configured as background
                    // — background hooks return exit_code: None immediately, so
                    // the denial check (exit_code == Some(2)) can never match.
                    if hook_executor
                        .has_background_hooks_for_event(crate::hooks::HookEvent::ToolCallBefore)
                    {
                        tracing::warn!(
                            "ToolCallBefore hook(s) configured with background=true — \
                             background hooks cannot deny tool calls because they exit \
                             immediately with no result"
                        );
                    }

                    let hook_context = crate::hooks::HookContext::new()
                        .with_tool_name(&tool_name)
                        .with_tool_args(&tool_input)
                        .with_mode(&format!("{mode:?}"))
                        .with_workspace(self.session.workspace.clone())
                        .with_model(&self.config.model)
                        .with_session_id(&self.session.id);
                    // Run hooks off the Tokio worker thread: `execute()` calls
                    // `child.wait_timeout()` which is a blocking syscall that
                    // would stall all other async tasks on this thread.
                    let executor = hook_executor.clone();
                    let hook_results = tokio::task::spawn_blocking(move || {
                        executor.execute(crate::hooks::HookEvent::ToolCallBefore, &hook_context)
                    })
                    .await
                    .unwrap_or_else(|join_err| {
                        tracing::error!("Hook executor task panicked: {join_err}");
                        Vec::new()
                    });
                    // #3026: fold all foreground hook results into one
                    // decision: deny (exit code 2 or JSON) > ask > allow;
                    // last `updatedInput` writer wins; `additionalContext`
                    // strings are concatenated.
                    let fold = fold_tool_call_before_results(&hook_results);
                    if let Some(reason) = fold.deny_reason {
                        blocked_error = Some(ToolError::permission_denied(format!(
                            "ToolCallBefore hook denied tool '{tool_name}': {reason}"
                        )));
                    } else {
                        if fold.requires_approval {
                            hook_requires_approval = true;
                        }
                        if let Some(updated) = fold.updated_input {
                            tool_input = updated;
                            reprepared_after_hook = true;
                            prepared_policy = match reprepare_tool_call_after_hook(
                                &tool_name,
                                tool_input.clone(),
                                tool_registry,
                                self.session.auto_approve,
                            ) {
                                Ok(policy) => Some(policy),
                                Err(error) => {
                                    blocked_error = Some(error);
                                    None
                                }
                            };
                        }
                        if let Some(context) = fold.additional_context {
                            hook_contexts.insert(tool_id.clone(), context);
                        }
                    }
                }

                if let Some(prepared) = prepared_policy {
                    approval_required = registered_tool_approval_required(
                        &tool_name,
                        prepared.call.approval,
                        prepared.auto_approve,
                    );
                    approval_description = prepared.call.description;
                    supports_parallel = prepared.call.supports_parallel;
                    read_only = prepared.call.read_only;
                    detached_start = prepared.call.starts_detached;
                    tool_input = prepared.call.input;
                    resources = prepared.call.resources;

                    let approval = match prepared.call.approval {
                        ApprovalRequirement::Auto => "auto",
                        ApprovalRequirement::Suggest => "suggest",
                        ApprovalRequirement::Required => "required",
                    };
                    emit_tool_audit(json!({
                        "event": "tool.prepared",
                        "tool_id": tool_id.clone(),
                        "tool_name": tool_name.clone(),
                        "read_only": read_only,
                        "supports_parallel": supports_parallel,
                        "starts_detached": detached_start,
                        "approval": approval,
                        "resources": &resources,
                        "reprepared_after_hook": reprepared_after_hook,
                    }));
                }

                if blocked_error.is_none()
                    && mode_blocks_write_capable_tool(mode, &tool_name, read_only)
                {
                    blocked_error = Some(ToolError::permission_denied(format!(
                        "'{tool_name}' is not available in Plan mode - switch to Act mode (`/mode act`) to modify files or run write-capable tools."
                    )));
                }

                // #3026: a hook `ask` decision forces the approval prompt even
                // for tools the registry would auto-run. Must stay after the
                // registry-based computation above, which assigns rather than
                // ORs `approval_required`.
                if hook_requires_approval && !self.session.auto_approve {
                    approval_required = true;
                }

                if blocked_error.is_none() {
                    let ask_rule_decision = exec_shell_ask_rule_decision(
                        &self.config,
                        &tool_name,
                        &tool_input,
                        &self.session.workspace,
                        self.session.approval_mode,
                    )
                    .or_else(|| {
                        file_tool_ask_rule_decision(
                            &self.config,
                            &tool_name,
                            &tool_input,
                            &self.session.workspace,
                            self.session.approval_mode,
                        )
                    });
                    if let Some(decision) = ask_rule_decision {
                        match decision {
                            ToolAskRuleDecision::Prompt(reason) => {
                                // #3790: the mode is the sole authority — a typed
                                // ask-rule prompts in Agent/Plan but never in YOLO
                                // (auto_approve). A typed deny rule still blocks
                                // hard, in every mode.
                                if !self.session.auto_approve {
                                    approval_required = true;
                                    approval_description = reason;
                                    approval_force_prompt = true;
                                }
                            }
                            ToolAskRuleDecision::Block(reason) => {
                                approval_required = false;
                                approval_force_prompt = false;
                                blocked_error = Some(ToolError::permission_denied(reason));
                            }
                        }
                    }
                }

                if blocked_error.is_none() {
                    let (decision, audit_event) = auto_review_plan_decision(
                        &self.config.auto_review_policy,
                        &tool_name,
                        &tool_input,
                        auto_review_run_origin_for_plan(detached_start),
                        self.session.approval_mode,
                        None,
                        crate::config::is_workspace_trusted(&self.session.workspace),
                        false,
                    );
                    emit_tool_audit(json!({
                        "event": "tool.auto_review_decision",
                        "tool_id": tool_id.clone(),
                        "auto_review": audit_event,
                    }));
                    match decision {
                        AutoReviewPlanDecision::NoChange => {}
                        AutoReviewPlanDecision::ForcePrompt(reason) => {
                            // The built-in safety floor is deliberately
                            // non-bypassable: YOLO auto-approves ordinary tool
                            // calls, but publish-like and background/headless
                            // destructive holds still require review.
                            approval_required = true;
                            approval_description = reason;
                            approval_force_prompt = true;
                        }
                        AutoReviewPlanDecision::Block(reason) => {
                            approval_required = false;
                            approval_force_prompt = false;
                            blocked_error = Some(ToolError::permission_denied(reason));
                        }
                    }
                }

                // Repo law: protected invariants with path globs compile into
                // mechanical write holds. Like the safety floor, law is not
                // bypassable by mode — it can only add holds, never remove
                // one, so this cannot weaken any gate above.
                if blocked_error.is_none()
                    && let Some(decision) = crate::repo_law::repo_law_plan_decision(
                        &self.session.workspace,
                        &tool_name,
                        &tool_input,
                    )
                {
                    emit_tool_audit(json!({
                        "event": "tool.repo_law_decision",
                        "tool_id": tool_id.clone(),
                        "decision": match &decision {
                            crate::repo_law::RepoLawPlanDecision::ForcePrompt(_) => "force_prompt",
                            crate::repo_law::RepoLawPlanDecision::Block(_) => "block",
                        },
                        "reason": match &decision {
                            crate::repo_law::RepoLawPlanDecision::ForcePrompt(reason)
                            | crate::repo_law::RepoLawPlanDecision::Block(reason) => reason.clone(),
                        },
                    }));
                    match decision {
                        crate::repo_law::RepoLawPlanDecision::ForcePrompt(reason) => {
                            approval_required = true;
                            approval_description = reason;
                            approval_force_prompt = true;
                        }
                        crate::repo_law::RepoLawPlanDecision::Block(reason) => {
                            approval_required = false;
                            approval_force_prompt = false;
                            blocked_error = Some(ToolError::permission_denied(reason));
                        }
                    }
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
                    emit_tool_audit(json!({
                        "event": "tool.schema_hydrated",
                        "tool_id": tool_id.clone(),
                        "tool_name": tool_name.clone(),
                        "auto_retry_same_turn": true,
                        "metadata": result.metadata,
                    }));
                    if should_emit_hydration_status {
                        let status = if requested_tool_name == tool_name {
                            format!(
                                "Auto-loaded deferred tool '{tool_name}' and retrying the pending call in the same turn."
                            )
                        } else {
                            format!(
                                "Auto-loaded deferred tool '{tool_name}' after resolving '{requested_tool_name}' and retrying in the same turn."
                            )
                        };
                        let _ = self.tx_event.send(Event::status(status)).await;
                    }
                    // Do not set guard_result: the tool is activated for this batch
                    // and will execute immediately with the model's original input.
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
                    approval_force_prompt,
                    supports_parallel,
                    read_only,
                    detached_start,
                    resources,
                    blocked_error,
                    guard_result,
                });
            }
            active_tool_names.extend(deferred_tools_hydrated_this_batch);

            // --- Intent summary for write tools (#2381) ---
            // When the model invokes write tools, extract its preceding text
            // as an "intent summary" so the approval view can show *why* the
            // change is being made, not just *what* will change.
            let has_write_tools = plans.iter().any(|p| {
                !p.read_only
                    && p.approval_required
                    && p.blocked_error.is_none()
                    && p.guard_result.is_none()
            });
            let intent_summary: Option<String> = if has_write_tools {
                approval_intent_summary(&current_text_visible)
            } else {
                None
            };

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
                let detached_start_count: usize = batches
                    .iter()
                    .filter_map(|batch| match batch {
                        ToolExecutionBatch::Parallel(plans) if plans.len() > 1 => {
                            Some(plans.iter().filter(|plan| plan.detached_start).count())
                        }
                        _ => None,
                    })
                    .sum();
                let tool_kind = if detached_start_count > 0 {
                    "read-only/background-start tools"
                } else {
                    "read-only tools"
                };
                let _ = self
                    .tx_event
                    .send(Event::status(format!(
                        "Executing {parallel_tool_count} {tool_kind} in {} parallel chunk(s)",
                        parallel_chunks.len(),
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

                // #3216 / #2211: once the turn is cancelled, do not start any
                // further tool batches. Cancellation arrives out-of-band (the
                // TUI cancels the shared token directly), so we can observe it
                // here even while a long serial fan-out — e.g. six `agent`
                // calls each resolving a model route under the global tool lock
                // — is mid-flight. Without this check the batch loop ran to
                // completion (~6×4s) with no way to interrupt, which read as a
                // hard TUI freeze. We record an interrupted result for every
                // remaining plan so each `tool_use` keeps a matching
                // `tool_result` (well-formed transcript), then fall through to
                // the post-loop cancellation check which ends the turn as
                // Interrupted. This branch is a no-op on the normal path.
                if self.cancel_token.is_cancelled() {
                    for plan in plans {
                        let terminal = ToolExecutionOutcome::cancelled(interrupted_tool_result());
                        let result = terminal.legacy_result();
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
                            terminal,
                        });
                    }
                    continue;
                }

                if parallel_allowed {
                    let parallel_plan_receipts: Vec<_> = plans
                        .iter()
                        .map(|plan| {
                            (
                                plan.index,
                                plan.id.clone(),
                                plan.name.clone(),
                                plan.input.clone(),
                            )
                        })
                        .collect();
                    let mut tool_tasks = FuturesUnordered::new();
                    let shell_permits =
                        Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_SHELL_EXEC));
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
                                terminal: ToolExecutionOutcome::from_legacy(result),
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
                                terminal: ToolExecutionOutcome::from_legacy(Err(err)),
                            });
                            continue;
                        }
                        let registry = tool_registry;
                        let lock = tool_exec_lock.clone();
                        let mcp_pool = mcp_pool.clone();
                        let tx_event = self.tx_event.clone();
                        let session_id = self.session.id.clone();
                        let started_at = Instant::now();
                        let shell_permits = shell_permits.clone();
                        let workspace = self.session.workspace.clone();

                        tool_tasks.push(async move {
                            let _shell_permit = if plan.name == "exec_shell" {
                                shell_permits.acquire_owned().await.ok()
                            } else {
                                None
                            };
                            let mut result = Engine::execute_tool_with_lock(
                                lock,
                                plan.supports_parallel || plan.detached_start,
                                plan.interactive,
                                tx_event.clone(),
                                plan.name.clone(),
                                plan.input.clone(),
                                workspace,
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
                                terminal: ToolExecutionOutcome::from_legacy(result),
                            }
                        });
                    }

                    let mut parallel_cancelled = false;
                    loop {
                        tokio::select! {
                            biased;
                            () = self.cancel_token.cancelled() => {
                                parallel_cancelled = true;
                                break;
                            }
                            outcome = tool_tasks.next() => {
                                let Some(outcome) = outcome else { break; };
                                let index = outcome.index;
                                outcomes[index] = Some(outcome);
                            }
                        }
                    }
                    // Dropping FuturesUnordered drops every still-active tool
                    // future (including MCP transport calls) instead of merely
                    // waiting for cooperative cancellation inside each tool.
                    drop(tool_tasks);
                    if parallel_cancelled {
                        for (index, id, name, input) in parallel_plan_receipts {
                            if outcomes[index].is_some() {
                                continue;
                            }
                            let terminal =
                                ToolExecutionOutcome::cancelled(interrupted_tool_result());
                            let result = terminal.legacy_result();
                            let _ = self
                                .tx_event
                                .send(Event::ToolCallComplete {
                                    id: id.clone(),
                                    name: name.clone(),
                                    result: result.clone(),
                                })
                                .await;
                            outcomes[index] = Some(ToolExecOutcome {
                                index,
                                id,
                                name,
                                input,
                                started_at: Instant::now(),
                                terminal,
                            });
                        }
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
                                terminal: ToolExecutionOutcome::from_legacy(result),
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
                                terminal: ToolExecutionOutcome::from_legacy(result),
                            });
                            continue;
                        }

                        if tool_name == MULTI_TOOL_PARALLEL_NAME {
                            let started_at = Instant::now();
                            let cancel_token = self.cancel_token.clone();
                            let terminal = tokio::select! {
                                biased;
                                () = cancel_token.cancelled() => {
                                    ToolExecutionOutcome::cancelled(interrupted_tool_result())
                                },
                                result = self.execute_parallel_tool(
                                    tool_input.clone(),
                                    tool_registry,
                                    tool_exec_lock.clone(),
                                ) => ToolExecutionOutcome::from_legacy(result),
                            };
                            let result = terminal.legacy_result();

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
                                terminal,
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
                                terminal: ToolExecutionOutcome::from_legacy(result),
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
                                terminal: ToolExecutionOutcome::from_legacy(result),
                            });
                            continue;
                        }

                        // Handle approval flow: returns (result_override, context_override, approval_stamp)
                        let (result_override, context_override, approval_stamp): (
                            Option<Result<ToolResult, ToolError>>,
                            Option<crate::tools::ToolContext>,
                            Option<ToolApprovalStamp>,
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
                                    input: tool_input.clone(),
                                    description: plan.approval_description.clone(),
                                    approval_key,
                                    approval_grouping_key,
                                    intent_summary: if plan.read_only {
                                        None
                                    } else {
                                        intent_summary.clone()
                                    },
                                    approval_force_prompt: plan.approval_force_prompt,
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
                                    (None, None, Some(ToolApprovalStamp::ApprovedByUser))
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
                                    (
                                        None,
                                        elevated_context,
                                        Some(ToolApprovalStamp::ApprovedWithPolicy),
                                    )
                                }
                                Err(err) => (Some(Err(err)), None, None),
                            }
                        } else {
                            (None, None, None)
                        };

                        // Per-tool snapshot for surgical undo (#384): capture workspace
                        // state before file-modifying tools execute so `/undo` can
                        // revert the most recent write_file/edit_file/apply_patch.
                        // See `should_pre_tool_snapshot` for the gating rationale (#3292).
                        if should_pre_tool_snapshot(
                            self.config.snapshots_enabled,
                            result_override.is_some(),
                            tool_name.as_str(),
                        ) {
                            let ws = self.session.workspace.clone();
                            let tid = tool_id.clone();
                            let cap = self.config.snapshots_max_workspace_bytes;
                            let _ = tokio::task::spawn_blocking(move || {
                                crate::core::turn::pre_tool_snapshot(&ws, &tid, cap)
                            })
                            .await;
                        }

                        let started_at = Instant::now();
                        let (mut result, cancelled_before_completion) =
                            if let Some(result_override) = result_override {
                                (result_override, false)
                            } else {
                                tokio::select! {
                                    biased;
                                    () = self.cancel_token.cancelled() => {
                                        (Ok(interrupted_tool_result()), true)
                                    },
                                    result = Self::execute_tool_with_lock(
                                        tool_exec_lock.clone(),
                                        plan.supports_parallel,
                                        plan.interactive,
                                        self.tx_event.clone(),
                                        tool_name.clone(),
                                        tool_input.clone(),
                                        self.session.workspace.clone(),
                                        tool_registry,
                                        mcp_pool.clone(),
                                        context_override,
                                    ) => (result, false),
                                }
                            };

                        if let Some(approval_stamp) = approval_stamp
                            && let Ok(tool_result) = result.as_mut()
                        {
                            stamp_tool_result_approval(tool_result, approval_stamp);
                        }

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

                        let terminal = if cancelled_before_completion {
                            ToolExecutionOutcome::cancelled(
                                result.expect("cancelled tool result is always model-visible"),
                            )
                        } else {
                            ToolExecutionOutcome::from_legacy(result)
                        };
                        outcomes[plan.index] = Some(ToolExecOutcome {
                            index: plan.index,
                            id: tool_id,
                            name: tool_name,
                            input: tool_input,
                            started_at,
                            terminal,
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
            let mut step_error_tool_names: Vec<String> = Vec::new();
            let mut step_error_tool_inputs: Vec<serde_json::Value> = Vec::new();
            let mut stop_after_plan_tool = false;
            // #dogfood 0.8.67: if the model mutates the goal mid-turn via
            // create_goal/update_goal, push the change to the sidebar right after
            // this tool batch instead of waiting for turn end — otherwise the
            // sidebar "Goal:" line stays stale for the whole (possibly long)
            // goal-loop turn while get_goal already reflects the new objective.
            let mut goal_tool_ran = false;
            let mut stuck_signal = None;

            for outcome in outcomes.into_iter().flatten() {
                let tool_input = outcome.input.clone();
                let tool_name_for_ws = outcome.name.clone();
                let terminal_status = outcome.terminal.status;
                let result = outcome.terminal.into_legacy_result();
                let observed_signal = match &result {
                    Ok(output) if output.success => {
                        stuck_guard.observe(StepFingerprint::tool(&outcome.name, &tool_input, None))
                    }
                    Ok(output) => stuck_guard.observe(StepFingerprint::tool(
                        &outcome.name,
                        &tool_input,
                        Some(&output.content),
                    )),
                    Err(error) => stuck_guard.observe(StepFingerprint::tool(
                        &outcome.name,
                        &tool_input,
                        Some(&error.to_string()),
                    )),
                };
                if matches!(observed_signal, Some(StuckSignal::Stop)) {
                    stuck_signal = Some(StuckSignal::Stop);
                } else if matches!(observed_signal, Some(StuckSignal::Warn))
                    && stuck_signal.is_none()
                {
                    stuck_signal = Some(StuckSignal::Warn);
                }
                if matches!(outcome.name.as_str(), "create_goal" | "update_goal") {
                    goal_tool_ran = true;
                }
                let should_stop_this_turn =
                    should_stop_after_plan_tool(mode, &outcome.name, &result);

                match result {
                    Ok(output) => {
                        emit_tool_audit(json!({
                            "event": "tool.result",
                            "tool_id": outcome.id.clone(),
                            "tool_name": outcome.name.clone(),
                            "status": terminal_status.as_str(),
                            "success": output.success,
                        }));
                        let output_for_context = compact_tool_result_for_route(
                            self.api_provider,
                            &self.session.model,
                            self.active_route_limits,
                            &outcome.name,
                            &output,
                        );
                        let tool_was_executed = output
                            .metadata
                            .as_ref()
                            .and_then(|metadata| metadata.get("executed"))
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true);
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

                        // #3026: pipe `additionalContext` from tool_call_before
                        // hooks back to the model alongside the tool result.
                        let output_for_context = match hook_contexts.get(&outcome.id) {
                            Some(context) => {
                                format!("{output_for_context}\n\n[hook context] {context}")
                            }
                            None => output_for_context,
                        };

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
                        let envelope: ErrorEnvelope = e.clone().into();
                        emit_tool_audit(json!({
                            "event": "tool.result",
                            "tool_id": outcome.id.clone(),
                            "tool_name": outcome.name.clone(),
                            "status": terminal_status.as_str(),
                            "success": false,
                            "error": e.to_string(),
                            "category": envelope.category.to_string(),
                            "severity": envelope.severity.to_string(),
                        }));
                        step_error_count += 1;
                        step_error_categories.push(envelope.category);
                        step_error_tool_names.push(outcome.name.clone());
                        step_error_tool_inputs.push(tool_input.clone());
                        let input_schema = tool_catalog
                            .iter()
                            .find(|tool| tool.name == outcome.name)
                            .map(|tool| &tool.input_schema);
                        let error = format_tool_error_with_schema(&e, &outcome.name, input_schema);
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

                turn.record_tool_call();
                stop_after_plan_tool |= should_stop_this_turn;
            }

            // Reflect a mid-turn goal change on the sidebar immediately (idempotent:
            // emit_goal_updated only sends when an objective is set, and the UI
            // applies it behind a `changed` guard).
            if goal_tool_ran {
                self.emit_goal_updated().await;
            }

            if let Some(signal) = stuck_signal {
                if matches!(signal, StuckSignal::Warn) {
                    self.add_session_message(self.runtime_text_message_with_turn_metadata(
                        STUCK_RUNTIME_NOTICE.to_string(),
                        UserInputProvenance::Runtime,
                    ))
                    .await;
                } else {
                    let reason = "stuck loop detected after repeated tool actions/results";
                    let _ = self.tx_event.send(Event::status(reason)).await;
                    return (TurnOutcomeStatus::Failed, Some(reason.to_string()));
                }
            }

            if stop_after_plan_tool {
                break;
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
                if let Some(hint) = tool_error_degradation_runtime_hint(
                    consecutive_tool_error_steps,
                    &step_error_tool_names,
                    &step_error_categories,
                    &step_error_tool_inputs,
                ) {
                    self.add_session_message(self.runtime_text_message_with_turn_metadata(
                        hint,
                        UserInputProvenance::Runtime,
                    ))
                    .await;
                }
            } else {
                consecutive_tool_error_steps = 0;
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

    async fn goal_continuation_message_if_needed(
        &self,
        tool_registry: Option<&crate::tools::ToolRegistry>,
        continuations_this_turn: &mut u32,
    ) -> Option<String> {
        let registry = tool_registry?;
        if !registry.contains("update_goal") {
            return None;
        }

        let mut snapshot = match self.config.goal_state.lock() {
            Ok(state) => state.snapshot(),
            Err(err) => {
                tracing::warn!("goal state lock poisoned during continuation check: {err}");
                return None;
            }
        };

        if !snapshot.is_active() {
            return None;
        }

        let per_turn_max = crate::tools::goal::MAX_GOAL_CONTINUATIONS_PER_TURN;
        if *continuations_this_turn >= per_turn_max {
            let _ = self
                .tx_event
                .send(Event::status(format!(
                    "Goal remains active after {per_turn_max} continuation pass(es) this turn; ending turn to avoid a runaway loop."
                )))
                .await;
            return None;
        }

        // Route the continuation decision through the goal-loop decision core.
        // There is no run-level cap — a goal runs until complete/blocked,
        // paused, or an optional token/time budget is exhausted. The per-turn
        // guard (`per_turn_max`) only bounds how many continuation passes
        // happen *within* a single turn before yielding back to the engine.
        let decision = crate::goal_loop::decide_continuation(
            crate::goal_loop::GoalRunStatus::Active,
            crate::goal_loop::GoalProgress {
                tokens_used: snapshot.tokens_used,
                time_used_seconds: snapshot.time_used_seconds,
                continuations: snapshot.continuation_count,
            },
            crate::goal_loop::GoalBudget {
                token_budget: snapshot.token_budget.map(u64::from),
                time_budget_seconds: None,
            },
        );
        if let crate::goal_loop::ContinuationDecision::Stop(reason) = decision {
            let message = match reason {
                crate::goal_loop::StopReason::TokenBudget => format!(
                    "Goal token budget reached ({} / {} tokens); ending continuation.",
                    snapshot.tokens_used,
                    snapshot.token_budget.unwrap_or_default()
                ),
                other => format!("Goal continuation stopped: {other:?}."),
            };
            let _ = self.tx_event.send(Event::status(message)).await;
            return None;
        }

        *continuations_this_turn = (*continuations_this_turn).saturating_add(1);
        match self.config.goal_state.lock() {
            Ok(mut state) => {
                state.record_continuation();
                snapshot = state.snapshot();
            }
            Err(err) => {
                tracing::warn!("goal state lock poisoned while recording continuation: {err}")
            }
        }
        let _ = self
            .tx_event
            .send(Event::status(format!(
                "Continuing active goal ({}/{per_turn_max} this turn, {} total)",
                *continuations_this_turn, snapshot.continuation_count
            )))
            .await;

        Some(crate::tools::goal::render_continuation_prompt(
            &snapshot,
            snapshot.continuation_count,
        ))
    }

    pub(super) fn messages_with_turn_metadata(&self) -> Vec<Message> {
        self.session.messages.clone().into()
    }
}

fn shell_completion_status_text(
    events: &[crate::tools::shell::ShellCompletionEvent],
    timing: &str,
) -> Option<String> {
    if events.is_empty() {
        return None;
    }

    let count = events.len();
    let failed = events
        .iter()
        .filter(|event| event.status != crate::tools::shell::ShellStatus::Completed)
        .count();
    let noun = if count == 1 { "job" } else { "jobs" };
    let prefix = if timing.trim().is_empty() {
        String::new()
    } else {
        format!("{} ", timing.trim())
    };
    let mut status = if failed == 0 {
        format!("{prefix}{count} background shell {noun} completed")
    } else {
        format!("{prefix}{count} background shell {noun} finished ({failed} failed)")
    };

    if count == 1
        && let Some(event) = events.first()
    {
        let command = truncate_runtime_status_field(&event.command, 80);
        status.push_str(&format!(": {command}"));
        if let Some(owner) = event
            .owner_agent_name
            .as_deref()
            .or(event.owner_agent_id.as_deref())
            .filter(|owner| !owner.trim().is_empty())
        {
            status.push_str(&format!(" (by {owner})"));
        }
    }

    Some(status)
}

fn truncate_runtime_status_field(text: &str, max_chars: usize) -> String {
    let normalized = text.replace(['\n', '\r'], " ");
    let mut chars = normalized.chars();
    let mut out = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

fn should_hold_turn_for_subagents(queued_completions: usize, running_children: usize) -> bool {
    // #3216: launching sub-agents must NOT barrier the parent turn. Only queued
    // completions (work already finished that must be surfaced into the
    // transcript) hold the turn open. Running children are background work — the
    // parent ends its turn and their results arrive via the completion sentinel
    // on a later turn. The
    // `running_children` argument is kept for call-site clarity and the
    // background-status message, but deliberately no longer gates the hold.
    let _ = running_children;
    queued_completions > 0
}

fn stream_chunk_timeout_budget(config: &EngineConfig) -> (u64, Duration) {
    let secs = config.stream_chunk_timeout.as_secs();
    (secs, Duration::from_secs(secs))
}

/// Whether a per-tool pre-execution snapshot should be taken before running
/// `tool_name` (#384).
///
/// Gated on `snapshots.enabled` (#3292) so that disabling snapshots suppresses
/// the per-tool `tool:<call_id>` commits, matching the pre/post-turn snapshot
/// call sites which already honor the same flag. A tool whose result is already
/// overridden (denied, hook-supplied, or otherwise short-circuited) never
/// executes a file write, so it is skipped too. Only the file-modifying tools
/// produce undoable workspace changes worth snapshotting.
fn should_pre_tool_snapshot(
    snapshots_enabled: bool,
    has_result_override: bool,
    tool_name: &str,
) -> bool {
    snapshots_enabled
        && !has_result_override
        && matches!(tool_name, "write_file" | "edit_file" | "apply_patch")
}

fn mode_blocks_command_execution(mode: AppMode, tool_name: &str) -> bool {
    mode == AppMode::Plan
        && matches!(
            tool_name,
            "exec_shell"
                | "exec_shell_wait"
                | "exec_shell_interact"
                | "exec_wait"
                | "exec_interact"
                | CODE_EXECUTION_TOOL_NAME
                | JS_EXECUTION_TOOL_NAME
        )
}

fn mode_blocks_write_capable_tool(mode: AppMode, tool_name: &str, read_only: bool) -> bool {
    mode == AppMode::Plan
        && (matches!(tool_name, "write_file" | "edit_file" | "apply_patch")
            || (McpPool::is_mcp_tool(tool_name) && !read_only))
}

/// Synthesize the tool result recorded for a tool call that never executed
/// because the turn was cancelled mid-batch (#3216 / #2211).
///
/// Esc/Ctrl+C cancels the shared cancellation token out-of-band (see
/// `EngineHandle::cancel_with_reason`), so the `for batch in batches` loop can
/// observe the cancellation between batches and stop launching further tools —
/// turning a wedged "six sub-agents, ~24s, can't cancel" turn into a prompt
/// interrupt. We still record a result for every un-run `tool_use` so each
/// keeps a matching `tool_result` and the transcript stays well-formed on
/// resume. It is an `Ok(ToolResult { success: false })` rather than an `Err`
/// so it routes through the benign outcome branch and does not inflate the
/// step's error counters or trip error-escalation.
fn interrupted_tool_result() -> ToolResult {
    ToolResult::error("Tool not executed: the request was cancelled before this tool ran.")
}

#[cfg(test)]
mod cancel_batch_tests {
    use super::*;

    #[test]
    fn interrupted_tool_result_is_a_non_error_unexecuted_marker() {
        let result = interrupted_tool_result();
        // Must not be marked successful (the tool never ran)...
        assert!(!result.success, "interrupted tool must not report success");
        // ...and must clearly explain why, for the resumed transcript.
        assert!(
            result.content.to_lowercase().contains("cancel"),
            "interrupted result should explain the cancellation: {:?}",
            result.content
        );
    }
}

#[cfg(test)]
mod pre_tool_snapshot_gate_tests {
    use super::*;

    // #3292: disabling snapshots must suppress the per-tool `tool:<call_id>`
    // commits, just like the pre/post-turn snapshot sites.
    #[test]
    fn disabled_snapshots_suppress_per_tool_snapshot() {
        for tool in ["write_file", "edit_file", "apply_patch"] {
            assert!(
                !should_pre_tool_snapshot(false, false, tool),
                "snapshots.enabled=false must skip per-tool snapshot for {tool}"
            );
        }
    }

    #[test]
    fn enabled_snapshots_snapshot_file_modifying_tools() {
        for tool in ["write_file", "edit_file", "apply_patch"] {
            assert!(
                should_pre_tool_snapshot(true, false, tool),
                "snapshots.enabled=true must snapshot {tool} before it runs"
            );
        }
    }

    #[test]
    fn overridden_result_skips_snapshot() {
        // A denied/short-circuited tool never executes a write, so no snapshot.
        assert!(!should_pre_tool_snapshot(true, true, "write_file"));
    }

    #[test]
    fn non_modifying_tools_are_never_snapshotted() {
        for tool in ["read_file", "shell", "grep", "list_dir"] {
            assert!(
                !should_pre_tool_snapshot(true, false, tool),
                "{tool} does not modify the workspace and must not be snapshotted"
            );
        }
    }

    #[test]
    fn plan_blocks_write_capable_tools_without_narrowing_operate() {
        for tool in [
            "exec_shell",
            "exec_shell_wait",
            "exec_shell_interact",
            CODE_EXECUTION_TOOL_NAME,
            JS_EXECUTION_TOOL_NAME,
        ] {
            assert!(mode_blocks_command_execution(AppMode::Plan, tool));
            assert!(
                !mode_blocks_command_execution(AppMode::Operate, tool),
                "Operate must not add a mode-only command denial for {tool}"
            );
        }

        for tool in ["write_file", "edit_file", "apply_patch"] {
            assert!(mode_blocks_write_capable_tool(AppMode::Plan, tool, false));
            assert!(
                !mode_blocks_write_capable_tool(AppMode::Operate, tool, false),
                "Operate must not add a mode-only write denial for {tool}"
            );
        }

        assert!(mode_blocks_write_capable_tool(
            AppMode::Plan,
            "mcp_filesystem_write",
            false
        ));
        assert!(!mode_blocks_write_capable_tool(
            AppMode::Operate,
            "mcp_filesystem_write",
            false
        ));
        assert!(!mode_blocks_write_capable_tool(
            AppMode::Plan,
            "mcp_filesystem_read",
            true
        ));
        assert!(!mode_blocks_write_capable_tool(
            AppMode::Plan,
            "read_file",
            true
        ));
        assert!(!mode_blocks_write_capable_tool(
            AppMode::Plan,
            "request_user_input",
            false
        ));
    }
}

#[cfg(test)]
mod stream_timeout_tests {
    use super::*;

    #[test]
    fn stream_chunk_timeout_budget_uses_engine_config() {
        let config = EngineConfig {
            stream_chunk_timeout: Duration::from_secs(42),
            ..EngineConfig::default()
        };

        assert_eq!(
            stream_chunk_timeout_budget(&config),
            (42, Duration::from_secs(42))
        );
    }
}

pub(super) fn command_allows_tool(allowed_tools: Option<&[String]>, tool_name: &str) -> bool {
    let Some(allowed_tools) = allowed_tools else {
        return true;
    };
    // Symmetric with `command_denies_tool`: support a trailing `*` wildcard
    // and lowercase both sides, so `allowed_tools = ["mcp_*"]` or `["ReadFile"]`
    // work instead of silently matching nothing (which strips the whole
    // catalog).
    let tool_name = tool_name.to_ascii_lowercase();
    allowed_tools.iter().any(|rule| {
        let rule = rule.to_ascii_lowercase();
        if let Some(prefix) = rule.strip_suffix('*') {
            tool_name.starts_with(prefix)
        } else {
            tool_name == rule
        }
    })
}

/// Folded outcome of all `tool_call_before` hook results for one tool call
/// (#3026). Precedence: deny (exit code 2 or JSON) > ask > allow;
/// `updatedInput` is last-writer-wins; `additionalContext` is concatenated.
#[derive(Debug, Default, PartialEq)]
struct ToolCallHookFold {
    /// Denial reason from an exit-code-2 hook or a JSON `deny` decision.
    deny_reason: Option<String>,
    /// At least one hook returned a JSON `ask` decision.
    requires_approval: bool,
    /// Replacement tool input from the last hook that supplied one.
    updated_input: Option<serde_json::Value>,
    /// Concatenated `additionalContext` strings from all hooks.
    additional_context: Option<String>,
}

fn fold_tool_call_before_results(results: &[crate::hooks::HookResult]) -> ToolCallHookFold {
    let mut fold = ToolCallHookFold::default();

    // Legacy hard deny: exit code 2 wins regardless of stdout (backwards
    // compatible with pre-#3026 hooks).
    if let Some(denial) = results.iter().find(|result| result.exit_code == Some(2)) {
        let reason = denial
            .stdout
            .trim()
            .lines()
            .next()
            .filter(|line| !line.is_empty())
            .or_else(|| {
                denial
                    .stderr
                    .trim()
                    .lines()
                    .next()
                    .filter(|line| !line.is_empty())
            })
            .or(denial.error.as_deref())
            .unwrap_or("ToolCallBefore hook denied tool execution");
        fold.deny_reason = Some(reason.to_string());
        return fold;
    }

    for result in results {
        // Background hooks return immediately with no process result and
        // cannot steer (the caller warns about that configuration).
        if result.exit_code.is_none() {
            continue;
        }
        let parsed = crate::hooks::parse_tool_call_before_stdout(&result.stdout);
        match parsed.decision {
            Some(crate::hooks::ToolCallDecision::Deny) => {
                fold.deny_reason =
                    Some(parsed.reason.unwrap_or_else(|| {
                        "ToolCallBefore hook denied tool execution".to_string()
                    }));
                return fold;
            }
            Some(crate::hooks::ToolCallDecision::Ask) => fold.requires_approval = true,
            Some(crate::hooks::ToolCallDecision::Allow) | None => {}
        }
        if let Some(updated) = parsed.updated_input {
            fold.updated_input = Some(updated);
        }
        if let Some(context) = parsed.additional_context {
            match &mut fold.additional_context {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(&context);
                }
                None => fold.additional_context = Some(context),
            }
        }
    }
    fold
}

/// Check whether `tool_name` is explicitly denied (#3027).
/// Deny always wins over allow.
pub(super) fn command_denies_tool(disallowed_tools: Option<&[String]>, tool_name: &str) -> bool {
    let Some(disallowed_tools) = disallowed_tools else {
        return false;
    };
    let tool_name = tool_name.to_ascii_lowercase();
    disallowed_tools.iter().any(|rule| {
        let rule = rule.to_ascii_lowercase();
        if let Some(prefix) = rule.strip_suffix('*') {
            tool_name.starts_with(prefix)
        } else {
            tool_name == rule
        }
    })
}

fn resolve_tool_definition<'a>(
    tool_name: &mut String,
    tool_catalog: &'a [Tool],
    tool_registry: Option<&crate::tools::ToolRegistry>,
) -> Option<&'a Tool> {
    let mut tool_def = tool_catalog
        .iter()
        .find(|def| def.name.as_str() == tool_name.as_str());

    // Resolve hallucinated tool names before policy gates run, so aliases like
    // ReadFile are checked against the canonical registered tool name.
    if tool_def.is_none()
        && let Some(registry) = tool_registry
        && let Some(canonical) = registry.resolve(tool_name.as_str())
    {
        crate::logging::info(format!(
            "Resolved hallucinated tool name '{tool_name}' -> '{canonical}'"
        ));
        tool_def = tool_catalog.iter().find(|d| d.name == canonical);
        if tool_def.is_some() {
            *tool_name = canonical.to_string();
        }
    }

    tool_def
}

/// Issue #1727: decide whether to surface a "thinking-only, no output" status.
///
/// Reached when the assistant turn had no sendable content (no Text, no
/// ToolUse — only a reasoning/thinking block). We notify the user *only* when
/// the turn is genuinely finishing: no tool uses to dispatch, no `turn_error`
/// already surfaced for this turn, the request wasn't cancelled, AND the turn
/// is not about to CONTINUE — there are no pending steers and we are not
/// holding the turn open for running sub-agents. The status must fire at the
/// point the turn truly ends; emitting it earlier (at the persist site) would
/// show a spurious "turn ended" notice immediately before the turn resumed
/// for a steer or a sub-agent completion.
fn should_emit_thinking_only_status(
    tool_uses_empty: bool,
    turn_error_is_none: bool,
    cancelled: bool,
    steers_pending: bool,
    holding_for_subagents: bool,
) -> bool {
    tool_uses_empty && turn_error_is_none && !cancelled && !steers_pending && !holding_for_subagents
}

/// Sentinel reasoning-effort value meaning "let the auto-reasoning system
/// decide" (#4158).
const REASONING_EFFORT_AUTO: &str = "auto";

/// Resolve an `"auto"` reasoning-effort tier to a concrete value.
///
/// When the configured effort is `"auto"`, inspects the last user message
/// and calls [`crate::auto_reasoning::select`] to pick the actual tier.
/// Non-`"auto"` values pass through unchanged.
fn resolve_auto_effort(
    reasoning_effort: Option<&str>,
    messages: &[Message],
    provider: crate::config::ApiProvider,
    base_url: &str,
    wire_model: &str,
) -> Option<String> {
    match reasoning_effort {
        Some(effort) if effort == REASONING_EFFORT_AUTO => {
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
            let resolved = tier
                .normalize_for_route(provider, base_url, wire_model)
                .as_setting()
                .to_string();
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
    fn subagent_completion_handoff_is_internal_user_message() {
        let message = subagent_completion_runtime_message(
            "Build passed\n<codewhale:subagent.done>{\"agent_id\":\"agent_a\"}</codewhale:subagent.done>",
        );

        // Must be "user", not "system": a system message appended mid-stream
        // trips strict chat templates (vLLM/Qwen3) into a 400 BadRequest
        // ("System message must be at the beginning"). The internal-event
        // framing lives in the text + visibility tag, not the role.
        assert_eq!(message.role, "user");
        let text = match &message.content[0] {
            ContentBlock::Text { text, .. } => text,
            other => panic!("expected text block, got {other:?}"),
        };
        assert!(text.contains("internal runtime event, not user input"));
        assert!(text.contains("Do not tell the user they pasted sentinels"));
        assert!(text.contains("<codewhale:subagent.done>"));
        assert!(text.contains("Build passed"));
    }

    #[test]
    fn shell_completion_status_does_not_create_runtime_handoff() {
        let status = shell_completion_status_text(
            &[crate::tools::shell::ShellCompletionEvent {
                task_id: "shell_abc".to_string(),
                command: "cargo test -p codewhale-tui".to_string(),
                status: crate::tools::shell::ShellStatus::Failed,
                exit_code: Some(101),
                duration_ms: 1234,
                stdout_tail: "running tests".to_string(),
                stderr_tail: "test failed".to_string(),
                linked_task_id: Some("task_1".to_string()),
                owner_agent_id: Some("agent_verifier".to_string()),
                owner_agent_name: Some("verifier".to_string()),
            }],
            "",
        )
        .expect("status text");

        assert!(status.contains("1 background shell job finished (1 failed)"));
        assert!(status.contains("cargo test -p codewhale-tui"));
        assert!(status.contains("by verifier"));
        assert!(!status.contains("runtime_event"));
        assert!(!status.contains("manual exec_shell_wait polling"));
        assert!(!status.contains("stderr_tail"));
    }

    #[test]
    fn turn_holds_only_for_queued_completions_not_running_children() {
        // #3216: queued completions hold the turn open so they get surfaced...
        assert!(should_hold_turn_for_subagents(1, 0));
        // ...but running children no longer barrier the parent — launching a
        // sub-agent is not the same as joining it (results arrive via the
        // completion sentinel).
        assert!(!should_hold_turn_for_subagents(0, 1));
        assert!(!should_hold_turn_for_subagents(0, 0));
        // Queued completions hold regardless of how many children are running.
        assert!(should_hold_turn_for_subagents(2, 5));
    }

    #[test]
    fn approval_intent_summary_trims_and_bounds_text() {
        assert_eq!(approval_intent_summary("   "), None);

        let long_text = format!("  {}  ", "x".repeat(MAX_APPROVAL_INTENT_SUMMARY_CHARS + 10));
        let summary = approval_intent_summary(&long_text).expect("summary");
        assert!(summary.ends_with("..."));
        assert_eq!(
            summary.chars().count(),
            MAX_APPROVAL_INTENT_SUMMARY_CHARS + 3
        );
    }

    /// Regression test for issue #1727 (P0, release-blocking).
    ///
    /// When a model (e.g. gpt-oss via ollama's harmony→OpenAI shim) returns
    /// ONLY a reasoning/thinking block — empty `content`, no `tool_calls` —
    /// `has_sendable_assistant_content` is false, so no assistant message is
    /// persisted. Previously the code also emitted NO event and fell straight
    /// through to finishing the turn: the UI spinner stayed up forever with no
    /// error, looking hung.
    ///
    /// This pins the decision: a clean turn end (no tool uses to dispatch, no
    /// `turn_error`, not cancelled, no pending steers, not holding for
    /// sub-agents) must surface a status. We must NOT spam the status when the
    /// turn is ending for another reason (error already shown, cancelled),
    /// when there are tool uses still to dispatch, or — critically (the
    /// MEDIUM review finding) — when the turn is about to CONTINUE because a
    /// steer is pending or sub-agents are still running. Emitting at the old
    /// persist site fired before those continuations were known.
    ///
    /// Limitation: this tests the extracted pure decision, not the full async
    /// `handle_deepseek_turn` loop (driving it would need a mock DeepSeek
    /// client + session + channels — far beyond a surgical fix and unlike any
    /// existing turn-loop test, which all pin pure helpers the same way). The
    /// wiring at the `tool_uses.is_empty()` tail (capture-then-decide, with the
    /// live steer/sub-agent signals) is reviewed by inspection — consistent
    /// with how the other turn-loop helpers in this module are tested.
    #[test]
    fn thinking_only_turn_emits_status_only_on_clean_end() {
        // Thinking-only response, turn genuinely ending (no tool uses, no
        // error, not cancelled, no steers pending, not holding for
        // sub-agents) → surface a status so the user isn't left staring at a
        // hung spinner.
        assert!(should_emit_thinking_only_status(
            true, true, false, false, false
        ));

        // Tool uses still pending → the normal dispatch path handles it; no
        // thinking-only status.
        assert!(!should_emit_thinking_only_status(
            false, true, false, false, false
        ));

        // A turn_error was already surfaced → don't double-report.
        assert!(!should_emit_thinking_only_status(
            true, false, false, false, false
        ));

        // Request was cancelled → cancellation status already covers it.
        assert!(!should_emit_thinking_only_status(
            true, true, true, false, false
        ));

        // A steer is pending → the turn will resume with the steer; emitting
        // "turn ended" now would be a spurious notice right before the turn
        // continues (the MEDIUM correctness finding).
        assert!(!should_emit_thinking_only_status(
            true, true, false, true, false
        ));

        // Sub-agents are still running / completions queued → the turn is
        // held open and will resume; do not claim it ended.
        assert!(!should_emit_thinking_only_status(
            true, true, false, false, true
        ));
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
            resolve_auto_effort(
                Some("auto"),
                &messages,
                crate::config::ApiProvider::Deepseek,
                crate::config::DEFAULT_DEEPSEEK_BASE_URL,
                "deepseek-v4-pro",
            ),
            Some("high".to_string()),
            "auto thinking should classify the user request, not stored metadata"
        );
    }

    #[test]
    fn allowed_tools_gate_blocks_unlisted_tool() {
        let allowed = vec!["bash".to_string(), "grep".to_string()];
        assert!(!command_allows_tool(Some(&allowed), "read"));
    }

    #[test]
    fn allowed_tools_gate_allows_listed_tool_case_insensitively() {
        let allowed = vec!["bash".to_string(), "read".to_string()];
        assert!(command_allows_tool(Some(&allowed), "Read"));
    }

    #[test]
    fn allowed_tools_gate_allows_all_tools_when_not_set() {
        assert!(command_allows_tool(None, "write"));
    }

    #[test]
    fn review_regression_allowed_tools_gate_blocks_all_tools_when_empty() {
        let allowed = Vec::new();
        assert!(!command_allows_tool(Some(&allowed), "bash"));
    }

    #[test]
    fn allowed_tools_gate_supports_wildcard_and_case() {
        // Symmetric with the deny list: `mcp_*` and mixed-case rules match.
        let allowed = vec!["mcp_*".to_string(), "ReadFile".to_string()];
        assert!(command_allows_tool(Some(&allowed), "mcp_slack_send"));
        assert!(command_allows_tool(Some(&allowed), "readfile"));
        assert!(command_allows_tool(Some(&allowed), "ReadFile"));
        assert!(!command_allows_tool(Some(&allowed), "exec_shell"));
    }

    #[test]
    fn disallowed_tools_gate_blocks_listed_tool() {
        let disallowed = vec!["exec_shell".to_string()];
        assert!(command_denies_tool(Some(&disallowed), "exec_shell"));
        assert!(!command_denies_tool(Some(&disallowed), "read_file"));
    }

    #[test]
    fn disallowed_tools_gate_blocks_case_insensitively() {
        let disallowed = vec!["exec_shell".to_string()];
        assert!(command_denies_tool(Some(&disallowed), "Exec_Shell"));
    }

    #[test]
    fn disallowed_tools_gate_blocks_prefix_wildcard() {
        let disallowed = vec!["mcp_acme_*".to_string()];
        assert!(command_denies_tool(
            Some(&disallowed),
            "mcp_acme_get_profile"
        ));
        assert!(!command_denies_tool(
            Some(&disallowed),
            "mcp_other_make_thing"
        ));
    }

    #[test]
    fn disallowed_tools_gate_is_inert_when_not_set() {
        assert!(!command_denies_tool(None, "exec_shell"));
        let empty: Vec<String> = Vec::new();
        assert!(!command_denies_tool(Some(&empty), "exec_shell"));
    }

    #[test]
    fn deny_wins_over_allow_for_same_tool() {
        // The turn-loop gate chain checks the deny-list before the allow-list,
        // so a tool present in both must still be blocked.
        let allowed = vec!["exec_shell".to_string()];
        let disallowed = vec!["exec_shell".to_string()];
        assert!(command_allows_tool(Some(&allowed), "exec_shell"));
        assert!(command_denies_tool(Some(&disallowed), "exec_shell"));
    }

    #[test]
    fn review_regression_allowed_tools_gate_checks_canonical_tool_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let context = crate::tools::spec::ToolContext::new(tmp.path().to_path_buf());
        let registry = crate::tools::ToolRegistryBuilder::new()
            .with_file_tools()
            .build(context);
        let catalog = registry.to_api_tools();
        let mut tool_name = "ReadFile".to_string();

        let tool_def = resolve_tool_definition(&mut tool_name, &catalog, Some(&registry));

        assert!(tool_def.is_some());
        assert_eq!(tool_name, "read_file");
        let allowed = vec!["read_file".to_string()];
        assert!(command_allows_tool(Some(&allowed), &tool_name));
    }

    #[test]
    fn hook_gate_denies_with_exit_code_2() {
        use crate::hooks::{Hook, HookContext, HookEvent, HookExecutor, HooksConfig};

        let deny_cmd = if cfg!(windows) { "exit /b 2" } else { "exit 2" };
        let config = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::ToolCallBefore, deny_cmd)],
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(config, std::path::PathBuf::from("."));
        let ctx = HookContext::new()
            .with_tool_name("exec_shell")
            .with_tool_args(&serde_json::json!({}));
        let results = executor.execute(HookEvent::ToolCallBefore, &ctx);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].exit_code, Some(2));
    }

    #[test]
    fn hook_gate_allows_with_exit_code_0() {
        use crate::hooks::{Hook, HookContext, HookEvent, HookExecutor, HooksConfig};

        let allow_cmd = if cfg!(windows) { "exit /b 0" } else { "exit 0" };
        let config = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::ToolCallBefore, allow_cmd)],
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(config, std::path::PathBuf::from("."));
        let ctx = HookContext::new()
            .with_tool_name("read_file")
            .with_tool_args(&serde_json::json!({}));
        let results = executor.execute(HookEvent::ToolCallBefore, &ctx);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].exit_code, Some(0));
        assert!(results[0].success);
    }

    #[test]
    fn hook_gate_failure_exit_code_1_is_not_denial() {
        use crate::hooks::{Hook, HookContext, HookEvent, HookExecutor, HooksConfig};

        let fail_cmd = if cfg!(windows) { "exit /b 1" } else { "exit 1" };
        let config = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::ToolCallBefore, fail_cmd)],
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(config, std::path::PathBuf::from("."));
        let ctx = HookContext::new()
            .with_tool_name("write_file")
            .with_tool_args(&serde_json::json!({}));
        let results = executor.execute(HookEvent::ToolCallBefore, &ctx);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].exit_code, Some(1));
        assert_ne!(results[0].exit_code, Some(2));
    }

    #[test]
    fn hook_gate_no_hooks_returns_no_results() {
        use crate::hooks::{HookContext, HookEvent, HookExecutor, HooksConfig};

        let config = HooksConfig {
            enabled: true,
            hooks: vec![],
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(config, std::path::PathBuf::from("."));
        let ctx = HookContext::new().with_tool_name("grep_files");
        let results = executor.execute(HookEvent::ToolCallBefore, &ctx);

        assert!(results.is_empty());
    }

    #[test]
    fn hook_gate_denial_reason_can_come_from_stdout() {
        use crate::hooks::{Hook, HookContext, HookEvent, HookExecutor, HooksConfig};

        let deny_cmd = if cfg!(windows) {
            "echo Tool blocked by security policy & exit /b 2"
        } else {
            "echo 'Tool blocked by security policy' && exit 2"
        };
        let config = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::ToolCallBefore, deny_cmd)],
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(config, std::path::PathBuf::from("."));
        let ctx = HookContext::new().with_tool_name("exec_shell");
        let results = executor.execute(HookEvent::ToolCallBefore, &ctx);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].exit_code, Some(2));
        assert!(results[0].stdout.contains("security"));
    }

    // ── #3026: JSON decision contract fold ─────────────────────────────────

    fn hook_result(stdout: &str, exit_code: Option<i32>) -> crate::hooks::HookResult {
        crate::hooks::HookResult {
            name: None,
            success: exit_code == Some(0),
            exit_code,
            stdout: stdout.to_string(),
            stderr: String::new(),
            duration: Duration::from_millis(1),
            error: None,
        }
    }

    #[test]
    fn hook_fold_json_deny_blocks_with_reason() {
        let fold = fold_tool_call_before_results(&[hook_result(
            r#"{"decision":"deny","reason":"nope"}"#,
            Some(0),
        )]);
        assert_eq!(fold.deny_reason.as_deref(), Some("nope"));
        assert!(!fold.requires_approval);
    }

    #[test]
    fn hook_fold_exit_code_2_denies_regardless_of_stdout() {
        let fold =
            fold_tool_call_before_results(&[hook_result(r#"{"decision":"allow"}"#, Some(2))]);
        assert!(
            fold.deny_reason.is_some(),
            "exit code 2 must hard-deny even when stdout says allow"
        );
    }

    #[test]
    fn hook_fold_deny_wins_over_ask_and_allow() {
        let fold = fold_tool_call_before_results(&[
            hook_result(r#"{"decision":"allow"}"#, Some(0)),
            hook_result(r#"{"decision":"ask"}"#, Some(0)),
            hook_result(r#"{"decision":"deny","reason":"policy"}"#, Some(0)),
        ]);
        assert_eq!(fold.deny_reason.as_deref(), Some("policy"));
    }

    #[test]
    fn hook_fold_ask_requires_approval() {
        let fold = fold_tool_call_before_results(&[
            hook_result(r#"{"decision":"allow"}"#, Some(0)),
            hook_result(r#"{"decision":"ask"}"#, Some(0)),
        ]);
        assert!(fold.deny_reason.is_none());
        assert!(fold.requires_approval);
    }

    #[test]
    fn hook_fold_updated_input_last_writer_wins() {
        let fold = fold_tool_call_before_results(&[
            hook_result(r#"{"updatedInput":{"command":"first"}}"#, Some(0)),
            hook_result(r#"{"updatedInput":{"command":"second"}}"#, Some(0)),
        ]);
        assert_eq!(
            fold.updated_input,
            Some(serde_json::json!({"command":"second"}))
        );
    }

    #[test]
    fn hook_fold_background_results_cannot_steer() {
        // Background hooks return exit_code: None immediately — their stdout
        // (if any were captured) must not deny, ask, or rewrite input.
        let fold = fold_tool_call_before_results(&[hook_result(
            r#"{"decision":"deny","reason":"too late"}"#,
            None,
        )]);
        assert_eq!(fold, ToolCallHookFold::default());
    }

    #[test]
    fn hook_fold_concatenates_additional_context() {
        let fold = fold_tool_call_before_results(&[
            hook_result(r#"{"additionalContext":"one"}"#, Some(0)),
            hook_result(r#"{"additionalContext":"two"}"#, Some(0)),
        ]);
        assert_eq!(fold.additional_context.as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn hook_fold_legacy_stdout_is_passthrough() {
        let fold = fold_tool_call_before_results(&[
            hook_result("", Some(0)),
            hook_result("not json at all", Some(0)),
            hook_result(r#"{"status":"fine"}"#, Some(1)),
        ]);
        assert_eq!(fold, ToolCallHookFold::default());
    }

    #[test]
    fn hook_gate_denies_with_json_decision_from_executor() {
        use crate::hooks::{Hook, HookContext, HookEvent, HookExecutor, HooksConfig};

        let deny_cmd = if cfg!(windows) {
            r#"echo {"decision":"deny","reason":"blocked by project policy"}"#
        } else {
            r#"echo '{"decision":"deny","reason":"blocked by project policy"}'"#
        };
        let config = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::ToolCallBefore, deny_cmd)],
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(config, std::path::PathBuf::from("."));
        let ctx = HookContext::new().with_tool_name("exec_shell");
        let results = executor.execute(HookEvent::ToolCallBefore, &ctx);

        let fold = fold_tool_call_before_results(&results);
        assert_eq!(
            fold.deny_reason.as_deref(),
            Some("blocked by project policy"),
            "JSON deny with exit code 0 must block: {results:?}"
        );
    }

    #[test]
    fn hook_gate_ask_forces_approval_from_executor() {
        use crate::hooks::{Hook, HookContext, HookEvent, HookExecutor, HooksConfig};

        let ask_cmd = if cfg!(windows) {
            r#"echo {"decision":"ask"}"#
        } else {
            r#"echo '{"decision":"ask"}'"#
        };
        let config = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::ToolCallBefore, ask_cmd)],
            ..HooksConfig::default()
        };
        let executor = HookExecutor::new(config, std::path::PathBuf::from("."));
        let ctx = HookContext::new().with_tool_name("write_file");
        let results = executor.execute(HookEvent::ToolCallBefore, &ctx);

        let fold = fold_tool_call_before_results(&results);
        assert!(fold.deny_reason.is_none());
        assert!(fold.requires_approval);
    }
}
