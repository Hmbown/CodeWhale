//! Capacity-controller checkpoints and interventions for the engine loop.
//!
//! Extracted from `core/engine.rs` for issue #74. The main turn loop still
//! decides when checkpoints run; this module owns the guardrail policy side
//! effects, replay verification, canonical-state persistence, and event
//! emission helpers.

use super::*;

use crate::models::context_window_for_model;

impl Engine {
    pub(super) async fn run_capacity_pre_request_checkpoint(
        &mut self,
        turn: &TurnContext,
        client: Option<&DeepSeekClient>,
        mode: AppMode,
    ) -> bool {
        let observation = self.capacity_observation(turn);
        let snapshot = self.capacity_controller.observe_pre_turn(observation);
        let decision = self
            .capacity_controller
            .decide(self.turn_counter, snapshot.as_ref());
        self.emit_capacity_decision(turn, snapshot.as_ref(), &decision)
            .await;

        if decision.action != GuardrailAction::TargetedContextRefresh {
            return false;
        }

        self.apply_targeted_context_refresh(turn, client, mode, snapshot.as_ref())
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_capacity_post_tool_checkpoint(
        &mut self,
        turn: &TurnContext,

        tool_registry: Option<&crate::tools::ToolRegistry>,
        tool_exec_lock: Arc<RwLock<()>>,
        mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,
        _step_error_count: usize,
        _consecutive_tool_error_steps: u32,
    ) -> bool {
        let observation = self.capacity_observation(turn);
        let snapshot = self.capacity_controller.observe_post_tool(observation);
        let decision = self
            .capacity_controller
            .decide(self.turn_counter, snapshot.as_ref());
        self.emit_capacity_decision(turn, snapshot.as_ref(), &decision)
            .await;

        match decision.action {
            GuardrailAction::VerifyWithToolReplay => {
                let _ = self
                    .apply_verify_with_tool_replay(
                        turn,
                        snapshot.as_ref(),
                        tool_registry,
                        tool_exec_lock,
                        mcp_pool,
                    )
                    .await;
                false
            }
            GuardrailAction::VerifyAndReplan => {
                self.apply_verify_and_replan(turn, snapshot.as_ref(), "high_risk_post_tool")
                    .await
            }
            GuardrailAction::NoIntervention | GuardrailAction::TargetedContextRefresh => false,
        }
    }

    pub(super) async fn run_capacity_error_escalation_checkpoint(
        &mut self,
        turn: &TurnContext,

        step_error_count: usize,
        consecutive_tool_error_steps: u32,
        error_categories: &[ErrorCategory],
    ) -> bool {
        if step_error_count == 0 && consecutive_tool_error_steps < 2 {
            return false;
        }

        // Categorize this step's failures by typed `ErrorCategory` rather than
        // substring-matching error strings. Context overflow always escalates;
        // network / rate-limit / timeout are transient and skip escalation;
        // anything else only escalates with consecutive consecutive failures.
        let has_context_overflow = error_categories.contains(&ErrorCategory::InvalidInput);
        let only_transient = !error_categories.is_empty()
            && error_categories.iter().all(|c| {
                matches!(
                    c,
                    ErrorCategory::Network | ErrorCategory::RateLimit | ErrorCategory::Timeout
                )
            });
        if only_transient && !has_context_overflow {
            return false;
        }
        if !has_context_overflow && consecutive_tool_error_steps < 2 {
            return false;
        }

        let snapshot = self
            .capacity_controller
            .last_snapshot()
            .cloned()
            .or_else(|| {
                let observation = self.capacity_observation(turn);
                self.capacity_controller.observe_pre_turn(observation)
            });
        let Some(snapshot) = snapshot else {
            return false;
        };

        let repeated_failures = step_error_count >= 2 || consecutive_tool_error_steps >= 2;
        let mut forced = snapshot.clone();
        if repeated_failures && !(snapshot.risk_band == RiskBand::High && snapshot.severe) {
            forced.risk_band = RiskBand::High;
            forced.severe = true;
        }

        let decision = self
            .capacity_controller
            .decide(self.turn_counter, Some(&forced));
        self.emit_capacity_decision(turn, Some(&forced), &decision)
            .await;

        if decision.action != GuardrailAction::VerifyAndReplan {
            return false;
        }

        let category_labels: Vec<String> = error_categories.iter().map(|c| c.to_string()).collect();
        self.apply_verify_and_replan(
            turn,
            Some(&forced),
            &format!(
                "error_escalation: step_errors={}, consecutive_steps={}, categories={}",
                step_error_count,
                consecutive_tool_error_steps,
                category_labels.join(",")
            ),
        )
        .await
    }

    pub(super) fn capacity_observation(&mut self, turn: &TurnContext) -> CapacityObservationInput {
        let message_window = self.config.capacity.profile_window.max(8) * 3;
        let action_count_this_turn = usize::try_from(turn.step)
            .unwrap_or(usize::MAX)
            .saturating_add(turn.tool_calls.len())
            .saturating_add(1);
        let tool_calls_recent_window = self.recent_tool_call_count(message_window);
        let unique_reference_ids_recent_window =
            self.recent_unique_reference_count(message_window, turn);
        let context_window = usize::try_from(
            context_window_for_model(&self.session.model)
                .unwrap_or(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS),
        )
        .unwrap_or(usize::try_from(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS).unwrap_or(128_000))
        .max(1);
        let context_used_ratio = (self.estimated_input_tokens() as f64) / (context_window as f64);

        CapacityObservationInput {
            turn_index: self.turn_counter,
            model: self.session.model.clone(),
            action_count_this_turn,
            tool_calls_recent_window,
            unique_reference_ids_recent_window,
            context_used_ratio,
        }
    }

    pub(super) fn recent_tool_call_count(&self, message_window: usize) -> usize {
        self.session
            .messages
            .iter()
            .rev()
            .take(message_window)
            .map(|msg| {
                msg.content
                    .iter()
                    .filter(|block| {
                        matches!(
                            block,
                            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                        )
                    })
                    .count()
            })
            .sum()
    }

    pub(super) fn recent_unique_reference_count(
        &self,
        message_window: usize,
        turn: &TurnContext,
    ) -> usize {
        let mut refs = std::collections::HashSet::new();
        for msg in self.session.messages.iter().rev().take(message_window) {
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        refs.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        refs.insert(tool_use_id.clone());
                    }
                    ContentBlock::Text { text, .. } => {
                        for token in text.split_whitespace() {
                            if token.contains('/') || token.contains('.') {
                                refs.insert(
                                    token
                                        .trim_matches(|c: char| ",.;:()[]{}".contains(c))
                                        .to_string(),
                                );
                            }
                        }
                    }
                    ContentBlock::Thinking { .. }
                    | ContentBlock::ServerToolUse { .. }
                    | ContentBlock::ToolSearchToolResult { .. }
                    | ContentBlock::CodeExecutionToolResult { .. }
                    | ContentBlock::ImageUrl { .. } => {}
                }
            }
        }
        for tool_call in turn.tool_calls.iter().rev().take(8) {
            refs.insert(tool_call.id.clone());
        }
        for path in self.session.working_set.top_paths(8) {
            refs.insert(path);
        }
        refs.retain(|item| !item.is_empty());
        refs.len()
    }

    pub(super) async fn emit_coherence_signal(
        &mut self,
        signal: CoherenceSignal,
        reason: impl Into<String>,
    ) {
        let next = next_coherence_state(self.coherence_state, signal);
        self.coherence_state = next;
        let _ = self
            .tx_event
            .send(Event::CoherenceState {
                state: next,
                label: next.label().to_string(),
                description: next.description().to_string(),
                reason: reason.into(),
            })
            .await;
    }

    pub(super) async fn emit_compaction_started(
        &mut self,
        id: String,
        auto: bool,
        message: String,
    ) {
        let _ = self
            .tx_event
            .send(Event::CompactionStarted {
                id,
                auto,
                message: message.clone(),
            })
            .await;
        self.emit_coherence_signal(CoherenceSignal::CompactionStarted, message)
            .await;
    }

    pub(super) async fn emit_compaction_completed(
        &mut self,
        id: String,
        auto: bool,
        message: String,
        messages_before: Option<usize>,
        messages_after: Option<usize>,
    ) {
        let _ = self
            .tx_event
            .send(Event::CompactionCompleted {
                id,
                auto,
                message: message.clone(),
                messages_before,
                messages_after,
            })
            .await;
        self.emit_coherence_signal(CoherenceSignal::CompactionCompleted, message)
            .await;
    }

    pub(super) async fn emit_compaction_failed(&mut self, id: String, auto: bool, message: String) {
        let _ = self
            .tx_event
            .send(Event::CompactionFailed {
                id,
                auto,
                message: message.clone(),
            })
            .await;
        self.emit_coherence_signal(CoherenceSignal::CompactionFailed, message)
            .await;
    }

    pub(super) async fn emit_capacity_decision(
        &mut self,
        turn: &TurnContext,
        snapshot: Option<&CapacitySnapshot>,
        decision: &CapacityDecision,
    ) {
        let Some(snapshot) = snapshot else {
            return;
        };
        let _ = self
            .tx_event
            .send(Event::CapacityDecision {
                session_id: self.session.id.clone(),
                turn_id: turn.id.clone(),
                h_hat: snapshot.h_hat,
                c_hat: snapshot.c_hat,
                slack: snapshot.slack,
                min_slack: snapshot.profile.min_slack,
                violation_ratio: snapshot.profile.violation_ratio,
                p_fail: snapshot.p_fail,
                risk_band: snapshot.risk_band.as_str().to_string(),
                action: decision.action.as_str().to_string(),
                cooldown_blocked: decision.cooldown_blocked,
                reason: decision.reason.clone(),
            })
            .await;
        self.emit_coherence_signal(
            CoherenceSignal::CapacityDecision {
                risk_band: snapshot.risk_band,
                action: decision.action,
                cooldown_blocked: decision.cooldown_blocked,
            },
            format!(
                "capacity_decision: risk={} action={} reason={}",
                snapshot.risk_band.as_str(),
                decision.action.as_str(),
                decision.reason
            ),
        )
        .await;
    }

    pub(super) async fn emit_capacity_intervention(
        &mut self,
        turn: &TurnContext,
        action: GuardrailAction,
        before_prompt_tokens: usize,
        after_prompt_tokens: usize,
        replay_outcome: Option<String>,
        replan_performed: bool,
    ) {
        let _ = self
            .tx_event
            .send(Event::CapacityIntervention {
                session_id: self.session.id.clone(),
                turn_id: turn.id.clone(),
                action: action.as_str().to_string(),
                before_prompt_tokens,
                after_prompt_tokens,
                compaction_size_reduction: before_prompt_tokens.saturating_sub(after_prompt_tokens),
                replay_outcome,
                replan_performed,
            })
            .await;
        self.emit_coherence_signal(
            CoherenceSignal::CapacityIntervention { action },
            format!("capacity_intervention: action={}", action.as_str()),
        )
        .await;
    }

    pub(super) async fn apply_targeted_context_refresh(
        &mut self,
        turn: &TurnContext,
        client: Option<&DeepSeekClient>,
        _mode: AppMode,
        snapshot: Option<&CapacitySnapshot>,
    ) -> bool {
        let before_tokens = self.estimated_input_tokens();
        let compaction_pins = self
            .session
            .working_set
            .pinned_message_indices(&self.session.messages, &self.session.workspace);
        let compaction_paths = self.session.working_set.top_paths(24);

        let mut refreshed = false;
        let should_run_summary_compaction = self.config.compaction.enabled
            && should_compact(
                &self.session.messages,
                &self.config.compaction,
                Some(&self.session.workspace),
                Some(&compaction_pins),
                Some(&compaction_paths),
            );
        if should_run_summary_compaction && let Some(client) = client {
            match compact_messages_safe(
                client,
                &self.session.messages,
                &self.config.compaction,
                Some(&self.session.workspace),
                Some(&compaction_pins),
                Some(&compaction_paths),
            )
            .await
            {
                Ok(result) => {
                    if !result.messages.is_empty() || self.session.messages.is_empty() {
                        self.session.messages = result.messages.into();
                        self.merge_compaction_summary(result.summary_prompt);
                        refreshed = true;
                    }
                }
                Err(err) => {
                    let _ = self
                        .tx_event
                        .send(Event::status(format!(
                            "Capacity refresh compaction failed: {err}. Falling back to local trim."
                        )))
                        .await;
                }
            }
        }

        if !refreshed {
            let target_budget = context_input_budget(&self.session.model)
                .unwrap_or(self.config.compaction.token_threshold.max(1));
            if self.estimated_input_tokens() > target_budget {
                let trimmed = self.trim_oldest_messages_to_budget(target_budget);
                refreshed = trimmed > 0;
            }
        }

        if !refreshed {
            return false;
        }

        let canonical = self.build_canonical_state(turn, None);
        let source_message_ids = self.capacity_source_message_ids(turn);
        let record = self.build_capacity_record(
            turn,
            GuardrailAction::TargetedContextRefresh,
            snapshot,
            canonical.clone(),
            source_message_ids,
            None,
        );
        let pointer = self
            .persist_capacity_record(turn, GuardrailAction::TargetedContextRefresh, &record)
            .await;
        self.merge_compaction_summary(Some(self.canonical_prompt(
            &canonical,
            &pointer,
            GuardrailAction::TargetedContextRefresh,
            None,
        )));
        self.refresh_system_prompt();
        self.emit_session_updated().await;

        let after_tokens = self.estimated_input_tokens();
        self.emit_capacity_intervention(
            turn,
            GuardrailAction::TargetedContextRefresh,
            before_tokens,
            after_tokens,
            None,
            false,
        )
        .await;
        self.capacity_controller
            .mark_intervention_applied(self.turn_counter, GuardrailAction::TargetedContextRefresh);
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn apply_verify_with_tool_replay(
        &mut self,
        turn: &TurnContext,
        snapshot: Option<&CapacitySnapshot>,
        tool_registry: Option<&crate::tools::ToolRegistry>,
        tool_exec_lock: Arc<RwLock<()>>,
        mut mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,
    ) -> bool {
        let before_tokens = self.estimated_input_tokens();
        let Some(candidate) = self.select_replay_candidate(turn, tool_registry) else {
            return false;
        };

        if McpPool::is_mcp_tool(&candidate.name) && mcp_pool.is_none() {
            mcp_pool = self.ensure_mcp_pool().await.ok();
        }

        let supports_parallel = if McpPool::is_mcp_tool(&candidate.name) {
            mcp_tool_is_parallel_safe(&candidate.name)
        } else {
            tool_registry
                .and_then(|registry| registry.get(&candidate.name))
                .is_some_and(|spec| spec.supports_parallel())
        };
        let interactive = (candidate.name == "exec_shell"
            && candidate
                .input
                .get("interactive")
                .and_then(serde_json::Value::as_bool)
                == Some(true))
            || candidate.name == REQUEST_USER_INPUT_NAME;

        let replay_result = Self::execute_tool_with_lock(
            tool_exec_lock,
            supports_parallel,
            interactive,
            self.tx_event.clone(),
            candidate.name.clone(),
            candidate.input.clone(),
            tool_registry,
            mcp_pool.clone(),
            None,
        )
        .await;

        let (pass, replay_outcome, diff_summary) = match replay_result {
            Ok(output) => {
                let original = candidate.result.as_deref().unwrap_or_default();
                let replay = output.content.as_str();
                let equal = original.trim() == replay.trim();
                let diff = if equal {
                    "output_match".to_string()
                } else {
                    format!(
                        "output_mismatch: original='{}' replay='{}'",
                        summarize_text(original, 140),
                        summarize_text(replay, 140)
                    )
                };
                (
                    equal,
                    if equal {
                        "pass".to_string()
                    } else {
                        "conflict".to_string()
                    },
                    diff,
                )
            }
            Err(err) => {
                self.capacity_controller
                    .mark_replay_failed(self.turn_counter);
                (
                    false,
                    "error".to_string(),
                    format!("replay_error: {}", summarize_text(&err.to_string(), 180)),
                )
            }
        };

        let verification_note = format!(
            "[verification replay] tool={} pass={} details={}",
            candidate.name, pass, diff_summary
        );
        self.add_session_message(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: candidate.id.clone(),
                content: verification_note.clone(),
                is_error: None,
                content_blocks: None,
            }],
        })
        .await;

        if !pass {
            self.capacity_controller
                .mark_replay_failed(self.turn_counter);
        }

        let canonical = self.build_canonical_state(
            turn,
            Some(if pass {
                "replay verification passed"
            } else {
                "replay verification failed or conflicted"
            }),
        );
        let replay_info = Some(ReplayInfo {
            tool_id: candidate.id.clone(),
            tool_name: candidate.name.clone(),
            pass,
            diff_summary: diff_summary.clone(),
        });
        let source_message_ids = self.capacity_source_message_ids(turn);
        let record = self.build_capacity_record(
            turn,
            GuardrailAction::VerifyWithToolReplay,
            snapshot,
            canonical.clone(),
            source_message_ids,
            replay_info,
        );
        let pointer = self
            .persist_capacity_record(turn, GuardrailAction::VerifyWithToolReplay, &record)
            .await;
        self.merge_compaction_summary(Some(self.canonical_prompt(
            &canonical,
            &pointer,
            GuardrailAction::VerifyWithToolReplay,
            Some(&verification_note),
        )));
        self.refresh_system_prompt();
        self.emit_session_updated().await;

        let after_tokens = self.estimated_input_tokens();
        self.emit_capacity_intervention(
            turn,
            GuardrailAction::VerifyWithToolReplay,
            before_tokens,
            after_tokens,
            Some(replay_outcome),
            false,
        )
        .await;
        self.capacity_controller
            .mark_intervention_applied(self.turn_counter, GuardrailAction::VerifyWithToolReplay);
        true
    }

    pub(super) async fn apply_verify_and_replan(
        &mut self,
        turn: &TurnContext,
        snapshot: Option<&CapacitySnapshot>,
        reason: &str,
    ) -> bool {
        let before_tokens = self.estimated_input_tokens();
        let canonical = self.build_canonical_state(turn, Some(reason));
        let source_message_ids = self.capacity_source_message_ids(turn);
        let record = self.build_capacity_record(
            turn,
            GuardrailAction::VerifyAndReplan,
            snapshot,
            canonical.clone(),
            source_message_ids,
            None,
        );
        let pointer = self
            .persist_capacity_record(turn, GuardrailAction::VerifyAndReplan, &record)
            .await;

        // The replan path needs the *full* messages, not summaries.
        // `scan_canonical_inputs` already located the indices in a single
        // reverse pass; clone from the live `messages` slice once. We
        // pass `true` because the replan path consumes
        // `latest_verified_user_idx` below.
        let scan = scan_canonical_inputs(&self.session.messages, true);
        let latest_user = scan
            .latest_user_text_idx
            .and_then(|idx| self.session.messages.get(idx).cloned());
        let latest_verified = scan
            .latest_verified_user_idx
            .and_then(|idx| self.session.messages.get(idx).cloned());

        self.session.messages.clear();
        if let Some(msg) = latest_user {
            self.session.messages.push(msg);
        }
        if let Some(msg) = latest_verified {
            self.session.messages.push(msg);
        }
        self.session.bump_messages_revision();

        self.merge_compaction_summary(Some(self.canonical_prompt(
            &canonical,
            &pointer,
            GuardrailAction::VerifyAndReplan,
            Some("Replan now from canonical state. Keep steps minimal and verifiable."),
        )));
        self.refresh_system_prompt();
        self.emit_session_updated().await;

        let _ = self
            .tx_event
            .send(Event::status(
                "Capacity guardrail: context reset to canonical state; replanning step."
                    .to_string(),
            ))
            .await;

        let after_tokens = self.estimated_input_tokens();
        self.emit_capacity_intervention(
            turn,
            GuardrailAction::VerifyAndReplan,
            before_tokens,
            after_tokens,
            None,
            true,
        )
        .await;
        self.capacity_controller
            .mark_intervention_applied(self.turn_counter, GuardrailAction::VerifyAndReplan);
        true
    }

    pub(super) fn select_replay_candidate(
        &self,
        turn: &TurnContext,
        tool_registry: Option<&crate::tools::ToolRegistry>,
    ) -> Option<TurnToolCall> {
        turn.tool_calls
            .iter()
            .rev()
            .find(|call| {
                call.error.is_none()
                    && call.result.is_some()
                    && self.tool_is_replayable_read_only(&call.name, tool_registry)
            })
            .cloned()
    }

    pub(super) fn tool_is_replayable_read_only(
        &self,
        tool_name: &str,
        tool_registry: Option<&crate::tools::ToolRegistry>,
    ) -> bool {
        if tool_name == MULTI_TOOL_PARALLEL_NAME || tool_name == REQUEST_USER_INPUT_NAME {
            return false;
        }
        if McpPool::is_mcp_tool(tool_name) {
            return mcp_tool_is_read_only(tool_name);
        }
        tool_registry
            .and_then(|registry| registry.get(tool_name))
            .is_some_and(|spec| spec.is_read_only())
    }

    pub(super) fn build_canonical_state(
        &self,
        turn: &TurnContext,
        note: Option<&str>,
    ) -> CanonicalState {
        // Single reverse scan of session.messages collects the goal,
        // confirmed facts (capped at 4), and the latest verified-user
        // message index. Previously this function did two reverse
        // `.iter().rev().find_map()` walks and a third for facts; the
        // dedicated scan below replaces all three with one pass that
        // also early-exits once every collector is satisfied. We pass
        // `false` here because build_canonical_state does not consume
        // `latest_verified_user_idx`, so we don't need the scan to keep
        // looking for it.
        let scan = scan_canonical_inputs(&self.session.messages, false);
        let goal = scan
            .goal
            .unwrap_or_else(|| "Continue current task from compact state".to_string());

        let mut constraints = vec![
            format!("model={}", self.session.model),
            format!("workspace={}", self.session.workspace.display()),
        ];
        if let Some(note) = note {
            constraints.push(summarize_text(note, 180));
        }

        let open_loops: Vec<String> = turn
            .tool_calls
            .iter()
            .rev()
            .filter_map(|call| {
                call.error
                    .as_ref()
                    .map(|error| format!("{}: {}", call.name, summarize_text(error, 180)))
            })
            .take(4)
            .collect();

        let pending_actions: Vec<String> = if open_loops.is_empty() {
            vec!["Continue with next smallest verifiable step".to_string()]
        } else {
            vec![
                "Re-evaluate failed tool steps with narrower scope".to_string(),
                "Re-derive plan from canonical facts before further edits".to_string(),
            ]
        };

        let mut critical_refs = self.session.working_set.top_paths(8);
        for tool_call in turn.tool_calls.iter().rev().take(4) {
            critical_refs.push(format!("tool:{}", tool_call.id));
        }
        critical_refs.dedup();

        CanonicalState {
            goal,
            constraints,
            confirmed_facts: scan.confirmed_facts,
            open_loops,
            pending_actions,
            critical_refs,
        }
    }

    pub(super) fn canonical_prompt(
        &self,
        canonical: &CanonicalState,
        pointer: &str,
        action: GuardrailAction,
        extra: Option<&str>,
    ) -> SystemPrompt {
        let mut lines = vec![
            COMPACTION_SUMMARY_MARKER.to_string(),
            format!("Capacity Canonical State [{}]", action.as_str()),
            format!("Goal: {}", canonical.goal),
            "Constraints:".to_string(),
        ];
        for item in &canonical.constraints {
            lines.push(format!("- {}", summarize_text(item, 200)));
        }
        lines.push("Confirmed Facts:".to_string());
        for item in &canonical.confirmed_facts {
            lines.push(format!("- {}", summarize_text(item, 200)));
        }
        lines.push("Open Loops:".to_string());
        if canonical.open_loops.is_empty() {
            lines.push("- none".to_string());
        } else {
            for item in &canonical.open_loops {
                lines.push(format!("- {}", summarize_text(item, 200)));
            }
        }
        lines.push("Pending Actions:".to_string());
        for item in &canonical.pending_actions {
            lines.push(format!("- {}", summarize_text(item, 200)));
        }
        lines.push("Critical Refs:".to_string());
        for item in &canonical.critical_refs {
            lines.push(format!("- {}", summarize_text(item, 200)));
        }
        if let Some(extra) = extra {
            lines.push(format!("Instruction: {}", summarize_text(extra, 240)));
        }
        lines.push(format!("Memory Pointer: {pointer}"));

        SystemPrompt::Blocks(vec![crate::models::SystemBlock {
            block_type: "text".to_string(),
            text: lines.join("\n"),
            cache_control: None,
        }])
    }

    pub(super) fn capacity_source_message_ids(&self, turn: &TurnContext) -> Vec<String> {
        let mut ids: Vec<String> = turn
            .tool_calls
            .iter()
            .rev()
            .take(8)
            .map(|call| call.id.clone())
            .collect();
        ids.reverse();
        ids
    }

    pub(super) fn build_capacity_record(
        &self,
        turn: &TurnContext,
        action: GuardrailAction,
        snapshot: Option<&CapacitySnapshot>,
        canonical: CanonicalState,
        source_message_ids: Vec<String>,
        replay_info: Option<ReplayInfo>,
    ) -> CapacityMemoryRecord {
        let (h_hat, c_hat, slack, risk_band) = snapshot
            .map(|s| (s.h_hat, s.c_hat, s.slack, s.risk_band.as_str().to_string()))
            .unwrap_or_else(|| (0.0, 0.0, 0.0, "unknown".to_string()));

        CapacityMemoryRecord {
            id: new_record_id(),
            ts: now_rfc3339(),
            turn_index: self.turn_counter,
            action_trigger: action.as_str().to_string(),
            h_hat,
            c_hat,
            slack,
            risk_band,
            canonical_state: canonical,
            source_message_ids: if source_message_ids.is_empty() {
                vec![turn.id.clone()]
            } else {
                source_message_ids
            },
            replay_info,
        }
    }

    pub(super) async fn persist_capacity_record(
        &mut self,
        turn: &TurnContext,
        action: GuardrailAction,
        record: &CapacityMemoryRecord,
    ) -> String {
        let pointer = format!("memory://{}/{}", self.session.id, record.id);
        if let Err(err) = append_capacity_record(&self.session.id, record) {
            let _ = self
                .tx_event
                .send(Event::CapacityMemoryPersistFailed {
                    session_id: self.session.id.clone(),
                    turn_id: turn.id.clone(),
                    action: action.as_str().to_string(),
                    error: summarize_text(&err.to_string(), 280),
                })
                .await;
            return format!("{pointer}?persist=failed");
        }
        pointer
    }

    pub(super) fn rehydrate_latest_canonical_state(&mut self) {
        let Ok(records) = load_last_k_capacity_records(&self.session.id, 1) else {
            return;
        };
        let Some(last) = records.last() else {
            return;
        };
        let pointer = format!("memory://{}/{}", self.session.id, last.id);
        let prompt = self.canonical_prompt(
            &last.canonical_state,
            &pointer,
            GuardrailAction::NoIntervention,
            Some("Rehydrated canonical state from memory."),
        );
        self.merge_compaction_summary(Some(prompt));
    }
}

/// Maximum number of confirmed-fact snippets retained by the canonical-state
/// scan. Matches the prior `build_canonical_state` behavior — only the
/// four most recent non-error tool results are surfaced.
const CANONICAL_SCAN_MAX_FACTS: usize = 4;

/// Output of [`scan_canonical_inputs`]: everything `build_canonical_state`
/// and `apply_verify_and_replan` need to know about the session's recent
/// history, collected in a single reverse pass over `session.messages`.
///
/// Index fields (`latest_user_text_idx`, `latest_verified_user_idx`) point
/// into the original `messages` slice so the caller can clone the full
/// `Message` value when the re-plan path needs to keep it across a
/// `messages.clear()`.
#[derive(Debug, Default)]
struct CanonicalStateScan {
    /// Most recent user-text block, summarized to ≤220 chars. `None` when
    /// no user message with a Text block exists.
    goal: Option<String>,
    /// Index of the most recent user message containing at least one
    /// `Text` content block. Used by the re-plan path to keep the
    /// latest user request across a `messages.clear()`.
    latest_user_text_idx: Option<usize>,
    /// Index of the most recent user message whose content includes a
    /// `[verification replay]` tool result. Used by the re-plan path.
    latest_verified_user_idx: Option<usize>,
    /// Up to [`CANONICAL_SCAN_MAX_FACTS`] most recent non-error
    /// `ToolResult` snippets, newest first.
    confirmed_facts: Vec<String>,
    /// Running count of facts collected so far; lets the early-exit
    /// condition avoid an extra `Vec::len()` call per message.
    facts_collected: usize,
}

impl CanonicalStateScan {
    /// `true` once every collector the caller actually needs is satisfied.
    ///
    /// `find_verified` controls whether `latest_verified_user_idx` is part
    /// of the early-exit gate. The build_canonical_state path does not
    /// consume that field, so passing `false` lets the scan stop as soon
    /// as the goal and `CANONICAL_SCAN_MAX_FACTS` facts are found — a
    /// huge win on long histories with no verification replay.
    fn is_complete(&self, find_verified: bool) -> bool {
        self.goal.is_some()
            && (!find_verified || self.latest_verified_user_idx.is_some())
            && self.facts_collected >= CANONICAL_SCAN_MAX_FACTS
    }
}

/// Walk `messages` once (in reverse) and collect everything the canonical
/// state and re-plan paths need. Replaces the previous pattern of three
/// independent reverse scans: one for the goal, one for confirmed facts,
/// and one for the latest verified user message.
///
/// `find_verified` controls whether the scan bothers locating the
/// latest verified user message. Callers that don't need it (e.g.
/// `build_canonical_state`) should pass `false` so the early-exit
/// condition can fire as soon as the goal + facts are gathered.
fn scan_canonical_inputs(messages: &[Message], find_verified: bool) -> CanonicalStateScan {
    let mut scan = CanonicalStateScan::default();
    for (idx, msg) in messages.iter().enumerate().rev() {
        if msg.role == "user" {
            if scan.goal.is_none()
                && let Some(text) = msg.content.iter().find_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
            {
                scan.goal = Some(summarize_text(text, 220));
                scan.latest_user_text_idx = Some(idx);
            }
            if find_verified && scan.latest_verified_user_idx.is_none() {
                let verified = msg.content.iter().any(|b| match b {
                    ContentBlock::ToolResult { content, .. } => {
                        content.contains("[verification replay]")
                    }
                    _ => false,
                });
                if verified {
                    scan.latest_verified_user_idx = Some(idx);
                }
            }
        }
        if scan.facts_collected < CANONICAL_SCAN_MAX_FACTS {
            for block in &msg.content {
                if let ContentBlock::ToolResult { content, .. } = block
                    && !content.starts_with("Error:")
                {
                    scan.confirmed_facts.push(summarize_text(content, 180));
                    scan.facts_collected = scan.facts_collected.saturating_add(1);
                    if scan.facts_collected >= CANONICAL_SCAN_MAX_FACTS {
                        break;
                    }
                }
            }
        }
        if scan.is_complete(find_verified) {
            break;
        }
    }
    scan
}

#[cfg(test)]
mod canonical_scan_tests {
    use super::*;
    use crate::models::ContentBlock;

    fn user_text_msg(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn user_with_verified_replay(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: text.to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "x".to_string(),
                    content: "[verification replay] pass=true".to_string(),
                    is_error: None,
                    content_blocks: None,
                },
            ],
        }
    }

    fn tool_result_msg(content: &str) -> Message {
        Message {
            role: "tool".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "x".to_string(),
                content: content.to_string(),
                is_error: None,
                content_blocks: None,
            }],
        }
    }

    #[test]
    fn scan_returns_goal_for_latest_user_text() {
        let messages = vec![
            user_text_msg("first"),
            tool_result_msg("a"),
            user_text_msg("second"),
            tool_result_msg("b"),
            user_text_msg("third"),
        ];
        let scan = scan_canonical_inputs(&messages, false);
        // Goal should be the most recent user text.
        let goal = scan.goal.expect("goal");
        assert!(
            goal.contains("third"),
            "expected the most recent, got {goal}"
        );
        assert_eq!(scan.latest_user_text_idx, Some(4));
    }

    #[test]
    fn scan_collects_up_to_four_facts_newest_first() {
        let messages = vec![
            tool_result_msg("fact-A"),
            tool_result_msg("fact-B"),
            tool_result_msg("fact-C"),
            tool_result_msg("fact-D"),
            tool_result_msg("fact-E"),
        ];
        let scan = scan_canonical_inputs(&messages, false);
        assert_eq!(scan.confirmed_facts.len(), 4);
        // The four most recent (newest first) are E, D, C, B.
        assert!(scan.confirmed_facts[0].contains("fact-E"));
        assert!(scan.confirmed_facts[1].contains("fact-D"));
        assert!(scan.confirmed_facts[2].contains("fact-C"));
        assert!(scan.confirmed_facts[3].contains("fact-B"));
    }

    #[test]
    fn scan_skips_error_results() {
        let messages = vec![
            tool_result_msg("good-A"),
            tool_result_msg("Error: bad"),
            tool_result_msg("good-B"),
        ];
        let scan = scan_canonical_inputs(&messages, false);
        assert_eq!(scan.confirmed_facts.len(), 2);
        assert!(scan.confirmed_facts[0].contains("good-B"));
        assert!(scan.confirmed_facts[1].contains("good-A"));
    }

    #[test]
    fn scan_finds_latest_verified_user_message() {
        let messages = vec![
            user_text_msg("first"),
            user_with_verified_replay("verified"),
            user_text_msg("third"),
        ];
        let scan = scan_canonical_inputs(&messages, true);
        // The verified marker is on the *middle* message, not the most
        // recent. The scan should report its actual position.
        assert_eq!(scan.latest_verified_user_idx, Some(1));
        // The goal still points at the most recent user text.
        assert!(scan.goal.as_deref().unwrap_or("").contains("third"));
    }

    #[test]
    fn scan_handles_empty_input() {
        let scan = scan_canonical_inputs(&[], false);
        assert!(scan.goal.is_none());
        assert!(scan.latest_verified_user_idx.is_none());
        assert!(scan.latest_user_text_idx.is_none());
        assert!(scan.confirmed_facts.is_empty());
    }

    #[test]
    fn scan_early_exits_when_complete() {
        // 1000 tool results — the scan should stop walking once the
        // first 4 facts and a goal are found. We can't directly assert
        // "didn't visit every element" without instrumentation, but the
        // call must return promptly with the right slice. We pass
        // `find_verified=false` so the scan does not have to keep
        // walking looking for a verified user message that isn't there.
        let mut messages: Vec<Message> = (0..1000)
            .map(|i| tool_result_msg(&format!("fact-{i}")))
            .collect();
        // Most recent user message comes last.
        messages.push(user_text_msg("goal"));
        let scan = scan_canonical_inputs(&messages, false);
        assert!(scan.goal.as_deref().unwrap_or("").contains("goal"));
        assert_eq!(scan.confirmed_facts.len(), 4);
    }
}
