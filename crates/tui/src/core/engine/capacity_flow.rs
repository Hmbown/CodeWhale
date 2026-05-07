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
        let snapshot = self
            .capacity_controller
            .observe_pre_turn(self.capacity_observation(turn));
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
        mode: AppMode,
        tool_registry: Option<&crate::tools::ToolRegistry>,
        tool_exec_lock: Arc<RwLock<()>>,
        mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,
        _step_error_count: usize,
        _consecutive_tool_error_steps: u32,
    ) -> bool {
        let snapshot = self
            .capacity_controller
            .observe_post_tool(self.capacity_observation(turn));
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
                        mode,
                        snapshot.as_ref(),
                        tool_registry,
                        tool_exec_lock,
                        mcp_pool,
                    )
                    .await;
                false
            }
            GuardrailAction::VerifyAndReplan => {
                self.apply_verify_and_replan(turn, mode, snapshot.as_ref(), "high_risk_post_tool")
                    .await
            }
            GuardrailAction::NoIntervention | GuardrailAction::TargetedContextRefresh => false,
        }
    }

    pub(super) async fn run_capacity_error_escalation_checkpoint(
        &mut self,
        turn: &TurnContext,
        mode: AppMode,
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
                self.capacity_controller
                    .observe_pre_turn(self.capacity_observation(turn))
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
            mode,
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

    pub(super) fn capacity_observation(&self, turn: &TurnContext) -> CapacityObservationInput {
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
                    | ContentBlock::CodeExecutionToolResult { .. } => {}
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
        mode: AppMode,
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
                        self.session.messages = result.messages;
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
            let target_budget = context_input_budget(&self.session.model, TURN_MAX_OUTPUT_TOKENS)
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
        self.refresh_system_prompt(mode);
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
        mode: AppMode,
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
        self.refresh_system_prompt(mode);
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
        mode: AppMode,
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

        let latest_user = self
            .session
            .messages
            .iter()
            .rev()
            .find(|msg| {
                msg.role == "user"
                    && msg
                        .content
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Text { .. }))
            })
            .cloned();
        let latest_verified = self
            .session
            .messages
            .iter()
            .rev()
            .find(|msg| {
                msg.role == "user"
                    && msg.content.iter().any(|block| match block {
                        ContentBlock::ToolResult { content, .. } => {
                            content.contains("[verification replay]")
                        }
                        _ => false,
                    })
            })
            .cloned();

        self.session.messages.clear();
        if let Some(msg) = latest_user {
            self.session.messages.push(msg);
        }
        if let Some(msg) = latest_verified {
            self.session.messages.push(msg);
        }

        self.merge_compaction_summary(Some(self.canonical_prompt(
            &canonical,
            &pointer,
            GuardrailAction::VerifyAndReplan,
            Some("Replan now from canonical state. Keep steps minimal and verifiable."),
        )));
        self.refresh_system_prompt(mode);
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
        let goal = self
            .session
            .messages
            .iter()
            .rev()
            .find_map(|msg| {
                if msg.role != "user" {
                    return None;
                }
                msg.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(summarize_text(text, 220)),
                    _ => None,
                })
            })
            .unwrap_or_else(|| "Continue current task from compact state".to_string());

        let mut constraints = vec![
            format!("model={}", self.session.model),
            format!("workspace={}", self.session.workspace.display()),
        ];
        if let Some(note) = note {
            constraints.push(summarize_text(note, 180));
        }

        let mut confirmed_facts = Vec::new();
        for msg in self.session.messages.iter().rev() {
            for block in &msg.content {
                if let ContentBlock::ToolResult { content, .. } = block {
                    if content.starts_with("Error:") {
                        continue;
                    }
                    confirmed_facts.push(summarize_text(content, 180));
                    if confirmed_facts.len() >= 4 {
                        break;
                    }
                }
            }
            if confirmed_facts.len() >= 4 {
                break;
            }
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
            confirmed_facts,
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
        // First try: load from current session
        if let Ok(records) = load_last_k_capacity_records(&self.session.id, 1) {
            if let Some(last) = records.last() {
                let pointer = format!("memory://{}/{}", self.session.id, last.id);
                let prompt = self.canonical_prompt(
                    &last.canonical_state,
                    &pointer,
                    GuardrailAction::NoIntervention,
                    Some("Rehydrated canonical state from memory."),
                );
                self.merge_compaction_summary(Some(prompt));
                return;
            }
        }

        // Fallback: scan across all sessions for the newest record
        if let Some((session_id, last)) = find_latest_cross_session() {
            let pointer = format!("memory://{}/{}", session_id, last.id);
            let prompt = self.canonical_prompt(
                &last.canonical_state,
                &pointer,
                GuardrailAction::NoIntervention,
                Some("Rehydrated canonical state from cross-session memory."),
            );
            self.merge_compaction_summary(Some(prompt));
        }
    }
}
