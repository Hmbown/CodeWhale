//! Engine-side authority for `/preview-request` (#1004, #3928).
//!
//! The preview lives here — not in the command layer — because only the
//! engine can rebuild the *exact* next-turn state: the tool catalog under the
//! live mode, gates, permission posture and connected MCP tools; the system
//! prompt for the route the next turn would use; the hypothetical next user
//! message in its production form; and the request the turn loop would hand
//! to `create_message_stream`.
//!
//! Four rules this module exists to enforce:
//!
//! - **Never `session.last_tool_catalog`.** That value is one turn stale and
//!   stores the pre-activation catalog, so it cannot describe what the *next*
//!   request would send. The catalog is rebuilt through
//!   [`Engine::build_turn_tool_registry_and_catalog`], which returns the same
//!   typed policy a real turn consumes.
//! - **Never invent a route.** For fixed routes, the host resolves the next
//!   turn through the same shared planner production dispatch uses. Auto would
//!   require a model-classifier call, so the human preview stops before the
//!   planner and emits a typed unavailable state. No route, endpoint, wire
//!   model, billing, tool budget, or body hash is recycled from the installed
//!   route.
//! - **Never resolve by side effect.** The catalog build runs with
//!   [`SubAgentWiring::Inert`] and [`McpAccess::PassiveSnapshot`]: no fork
//!   snapshot, no spawned drainer, no MCP pool creation, no `connect_all`, no
//!   status events. When the connected MCP state is not already exactly what
//!   a turn would use, the tool section is reported unavailable rather than
//!   made exact by connecting — **and so is the body**, because a body built
//!   from a tool surface missing its MCP contribution is a body no turn would
//!   send.
//! - **Never install anything, not even briefly.** The planned route is
//!   projected into a throw-away client; `self.api_provider`,
//!   `self.session.model`, `self.session.system_prompt`, and the MCP pool are
//!   all left untouched. Everything a turn would *install before* building its
//!   request — the command-scoped tool gate, the effective mode and approval
//!   posture, the policy-narrowing event, the observed working set — is passed
//!   as a value or snapshotted onto a clone. There is no write-then-restore
//!   anywhere in this module: a restore is not atomic across an `.await`, and
//!   it does not survive a cancellation or a panic.
//! - **Never claim exactness the runtime would break.** Mutable
//!   `message_submit` hooks, background-shell completions, running or
//!   terminal-undelivered sub-agent completions,
//!   pending LSP diagnostics, auto-compaction, and context-overflow recovery
//!   all rewrite the request between submit and the wire. An inspection may
//!   neither run them nor consume them, so when any of them apply the affected
//!   sections are typed unavailable instead of published.
//!
//! Scope: this describes the primary agent turn (`create_message_stream`).
//! Auxiliary provider calls are out of scope; see `docs/PREVIEW_REQUEST.md`.
//!
//! The `dryrun` concept — preview the next request from the real
//! request-building seam rather than a hand-rolled summary — is harvested
//! from PR #1099 by TaoMu (GTC2080).

use super::*;

use crate::client::PreparedOutboundRequest;
use crate::request_manifest::{
    Availability, BasePromptProvenance, BillingFacts, ManifestDraft, PreparedBodyInputs,
    PromptProvenance, ReasoningResolution, RequestManifest, RouteFacts, SessionFacts,
    SystemPromptAssembly, ToolSurfaceFacts, UnavailableReason,
};
use crate::route_runtime::ResolvedRuntimeRoute;
use crate::safe_label::SafeLabel;

/// Everything the host must supply for the engine to describe the next
/// request. These are the same posture fields a `SendMessage` would carry, so
/// the preview describes the turn the user is actually about to run.
#[derive(Debug)]
pub struct PreviewRequestInputs {
    pub mode: AppMode,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub auto_approve: bool,
    pub approval_mode: crate::tui::approval::ApprovalMode,
    pub allowed_tools: Option<Vec<String>>,
    pub dynamic_tools: Vec<DynamicToolSpec>,
    pub provenance: UserInputProvenance,
    /// The model selector the user chose: `auto` when auto model routing is
    /// on. Never the concrete model an unresolved auto route might pick.
    pub requested_model: String,
    /// Reasoning tier the user has selected (`auto`, `high`, `off`, …).
    pub requested_reasoning: String,
    pub auto_model: bool,
    /// Whether the *caller* supplied a hypothetical next prompt.
    ///
    /// Deliberately independent of [`Self::next_turn`]: when planning that
    /// prompt fails, the manifest must still say a prompt was supplied.
    /// Deriving the flag from `next_turn.is_some()` told the user to "pass
    /// `--prompt`" when they just had.
    pub hypothetical_prompt_supplied: bool,
    /// The exact next turn, resolved by the host's shared route planner.
    /// `None` means no exact next turn exists to describe.
    pub next_turn: Option<Box<PreviewNextTurn>>,
    /// Why `next_turn` is absent. Ignored when `next_turn` is present.
    pub unresolved: PreviewUnresolved,
}

/// One hypothetical next turn, planned by the production route planner.
#[derive(Debug)]
pub struct PreviewNextTurn {
    /// The model-facing text of the hypothetical user message, already
    /// through the host's file-mention/skill resolution — the same string a
    /// real `SendMessage` would carry. Never stored in the session and never
    /// sent to a provider.
    pub content: String,
    /// The route the planner resolved for this turn.
    pub route: Box<ResolvedRuntimeRoute>,
    /// Immutable prompt facts captured from the same host state as the
    /// matching production submit.
    pub prompt_context: NextTurnPromptContext,
    /// Normalized reasoning-effort api value from the planner, exactly as it
    /// would be sent.
    pub reasoning_effort: Option<String>,
    /// True when the user selected auto reasoning and the planner picked that
    /// tier.
    pub reasoning_effort_auto: bool,
    /// How the auto router chose this route, when auto routing ran.
    pub auto_route_source: Option<String>,
    /// Typed selection provenance captured by the shared production planner.
    pub routing_source: crate::turn_route_plan::TurnRoutingSource,
    /// The compaction policy the planner resolved for this route. A real turn
    /// installs it before the turn loop decides whether to auto-compact, so
    /// the preview evaluates that decision against the same policy.
    pub compaction: crate::compaction::CompactionConfig,
}

/// Why no exact next turn was planned.
#[derive(Debug, Clone)]
pub enum PreviewUnresolved {
    /// Auto model routing is on and no hypothetical prompt was supplied.
    AutoRouteNeedsPrompt,
    /// Auto model routing needs a classifier provider call. A preview is
    /// strictly offline, so it stops before invoking the shared route planner.
    AutoRouteClassificationNotExecuted,
    /// No hypothetical prompt was supplied, so there is no next-turn body.
    NoPrompt,
    /// The shared planner ran and failed. Carries raw host text; it crosses
    /// the safe-label boundary before it reaches any surface.
    PlanFailed(String),
    /// Mutable `message_submit` hooks are configured. A real submit runs them
    /// before file mentions, skill wrapping, route planning, and the tool
    /// policy see the text, and they may replace or block it outright. An
    /// inspection must not execute a hook, so nothing downstream of the text
    /// — route, tools, or body — can be claimed exact.
    MessageSubmitHooksConfigured,
    /// Resolving the prompt into model-facing content failed exactly as a real
    /// submit would have failed. Carries raw host text.
    PromptResolutionFailed(String),
}

impl PreviewUnresolved {
    fn as_availability<T>(&self) -> Availability<T> {
        match self {
            Self::AutoRouteNeedsPrompt => {
                Availability::unavailable(UnavailableReason::AutoRouteUnresolvedUntilNextPrompt)
            }
            Self::AutoRouteClassificationNotExecuted => {
                Availability::unavailable(UnavailableReason::AutoRouteClassificationNotExecuted)
            }
            Self::NoPrompt => {
                Availability::unavailable(UnavailableReason::NoHypotheticalPromptSupplied)
            }
            Self::PlanFailed(error) => {
                Availability::unavailable_with(UnavailableReason::RoutePlanFailed, error.clone())
            }
            Self::MessageSubmitHooksConfigured => {
                Availability::unavailable(UnavailableReason::MessageSubmitHooksNotExecuted)
            }
            Self::PromptResolutionFailed(error) => Availability::unavailable_with(
                UnavailableReason::PromptResolutionFailed,
                error.clone(),
            ),
        }
    }
}

impl Engine {
    /// Describe the request the next turn would send, without sending it.
    pub(super) async fn build_request_manifest(
        &mut self,
        inputs: PreviewRequestInputs,
    ) -> RequestManifest {
        let session = self.preview_session_facts(&inputs);

        // An automatic goal continuation has a terminal gate before route
        // dispatch. Mirror it before doing any request construction: an
        // exhausted active goal has no eligible next outbound request. Usage
        // is deliberately retained when a goal is resumed or its budget is
        // changed, so lowering a budget below already-consumed usage also
        // closes this gate.
        let goal_budget_exhausted = match self.config.goal_state.lock() {
            Ok(state) => {
                let snapshot = state.snapshot();
                Ok(snapshot.is_active()
                    && crate::goal_loop::token_budget_exhausted(
                        crate::goal_loop::GoalProgress {
                            tokens_used: snapshot.tokens_used,
                            time_used_seconds: snapshot.time_used_seconds,
                            continuations: snapshot.continuation_count,
                        },
                        crate::goal_loop::GoalBudget {
                            token_budget: snapshot.token_budget.map(u64::from),
                            time_budget_seconds: None,
                            max_continuations: self.config.goal_max_continuations,
                        },
                    ))
            }
            Err(err) => {
                tracing::warn!("goal state lock poisoned while previewing request: {err}");
                Err(())
            }
        };
        let unavailable_reason = match goal_budget_exhausted {
            Ok(true) => Some(UnavailableReason::GoalTokenBudgetExhausted),
            Ok(false) => None,
            Err(()) => Some(UnavailableReason::GoalStateNotSnapshottable),
        };
        if let Some(reason) = unavailable_reason {
            return RequestManifest::build(ManifestDraft {
                session,
                route: Availability::unavailable(reason),
                tools: Availability::unavailable(reason),
                body: Availability::unavailable(reason),
            });
        }

        let Some(next_turn) = inputs.next_turn else {
            let unresolved = inputs.unresolved;
            return RequestManifest::build(ManifestDraft {
                session,
                route: unresolved.as_availability(),
                tools: unresolved.as_availability(),
                body: unresolved.as_availability(),
            });
        };

        let PreviewNextTurn {
            content: hypothetical_content,
            route: planned_route,
            prompt_context: planned_prompt_context,
            reasoning_effort,
            reasoning_effort_auto,
            auto_route_source,
            routing_source,
            compaction: planned_compaction,
        } = *next_turn;

        // Project the planned route into a throw-away client. `validate`
        // reuses the host's preflighted client when there is one and never
        // touches engine state — unlike `install_resolved_runtime_route`,
        // which is what a real turn calls.
        let route = match (*planned_route).validate() {
            Ok(route) => route,
            Err(error) => {
                let unavailable = PreviewUnresolved::PlanFailed(error);
                return RequestManifest::build(ManifestDraft {
                    session,
                    route: unavailable.as_availability(),
                    tools: unavailable.as_availability(),
                    body: unavailable.as_availability(),
                });
            }
        };

        let provider = route.identity.provider;
        let model = route.model.clone();
        let limits = crate::route_budget::known_route_limits(route.candidate.limits());
        let base_url = route.candidate.endpoint().base_url.clone();
        let route_context = TurnRouteContext {
            provider,
            model: model.clone(),
            capabilities: route.candidate.capabilities(),
            limits,
            client: Some(route.client.clone()),
            api_config: route.config.clone(),
            locale_tag: self.config.locale_tag.clone(),
            role_models: self.subagent_role_models(),
            fleet_roster: self.config.fleet_roster.clone(),
            auto_model: inputs.auto_model,
            reasoning_effort: reasoning_effort.clone(),
            reasoning_effort_auto,
        };

        // Same policy derivation as `handle_send_message`, so the catalog is
        // filtered under the posture the next turn would actually use.
        let input_policy = effective_input_policy(
            inputs.provenance,
            inputs.mode,
            &hypothetical_content,
            inputs.allow_shell,
            inputs.trust_mode,
            inputs.mode == AppMode::Yolo || inputs.auto_approve,
            inputs.approval_mode,
        );
        let prompt_context = NextTurnPromptContext {
            mode: input_policy.mode,
            ..planned_prompt_context
        };

        // The command-scoped allow gate is *passed*, never installed. The
        // earlier shape wrote `self.config.allowed_tools`, awaited the whole
        // catalog build, and wrote it back: for the duration of that await the
        // engine carried a gate belonging to a turn that was never going to
        // run, and a cancellation or panic in between would have left it
        // installed for good.
        let build = self
            .build_turn_tool_registry_and_catalog(
                &input_policy,
                &inputs.dynamic_tools,
                inputs.allowed_tools.clone(),
                SubAgentWiring::Inert,
                McpAccess::PassiveSnapshot,
                route_context.clone(),
                "",
            )
            .await;

        // The build owns the exact same initial subset dispatch consumes.
        let surface = &build.surface;
        let active_tools = surface.active.clone().unwrap_or_default();
        let active_catalog_sha256 = active_tool_catalog_sha256(&active_tools);

        let tool_choice = surface.active.as_ref().map(|_| {
            if surface.strict_tool_mode {
                json!("required")
            } else {
                json!({ "type": "auto" })
            }
        });

        // The tool surface is only publishable when the MCP contribution is
        // exactly known. Anything else would be "the tools of some other
        // turn", which is the failure mode this command exists to avoid.
        let tools = match build.mcp.server_count() {
            Some(mcp_server_count) => Availability::Exact(ToolSurfaceFacts {
                catalog_tool_count: surface.catalog.len(),
                deferred_tool_count: surface
                    .catalog
                    .iter()
                    .filter(|tool| tool.defer_loading.unwrap_or(false))
                    .count(),
                active_tool_count: active_tools.len(),
                active_tool_catalog_sha256: active_catalog_sha256,
                tool_surface_budget: format!(
                    "{:?}",
                    route_context.capability_profile().tool_surface_budget
                ),
                standard_and_full_surfaces_collapsed: standard_and_full_collapse(
                    &surface.catalog,
                    &self.config.tools_always_load,
                ),
                mcp_server_count,
                mcp_tool_count: active_tools
                    .iter()
                    .filter(|tool| build.mcp_tool_names.contains(&tool.name))
                    .count(),
            }),
            None => match &build.mcp {
                McpToolState::Unavailable { reason } => Availability::unavailable_with(
                    UnavailableReason::McpStateNotSnapshottable,
                    reason.label(),
                ),
                McpToolState::Disabled | McpToolState::Live { .. } => {
                    Availability::unavailable(UnavailableReason::McpStateNotSnapshottable)
                }
            },
        };

        // The system prompt a turn would send is composed for *its* route, so
        // an auto-routed preview must not reuse the installed model's prompt.
        // A session-level override wins here exactly as it does in
        // `refresh_system_prompt`.
        let system_prompt = if self.session.system_prompt_override {
            self.session.system_prompt.clone()
        } else {
            self.compose_stable_system_prompt(&prompt_context)
        };

        // The hypothetical user message goes through the same constructor
        // production uses — turn metadata, route stamp, and provenance — so
        // the body being hashed is the body a real turn would build. It is
        // appended to a *clone* of the history and discarded: the session
        // never sees it.
        //
        // A real submit calls `working_set.observe_user_message` before it
        // writes `<turn_meta>`, so the block reflects files the new message
        // mentions. The preview observes the message on a **clone** of the
        // working set and builds the block from that snapshot: same bytes, no
        // session write. Nothing here restores state, because nothing here
        // changes any.
        let mut previewed_working_set = self.session.working_set.clone();
        previewed_working_set.observe_user_message(&hypothetical_content, &self.session.workspace);
        let hypothetical_user_message = self.user_text_message_from_snapshot(
            hypothetical_content.clone(),
            &model,
            inputs.auto_model,
            reasoning_effort.as_deref(),
            reasoning_effort_auto,
            inputs.provenance,
            TurnMetadataSnapshot {
                prompt_context: &prompt_context,
                system_prompt: system_prompt.as_ref(),
                approval_mode: input_policy.approval_mode_for_session(),
                working_set: &previewed_working_set,
                policy_narrowing: input_policy.narrowing.as_ref(),
            },
        );
        // Classification input for the provenance section: the prompt this
        // request actually carries, not the session's current one.
        let system_prompt_text = crate::prefix_cache::system_prompt_text(system_prompt.as_ref());

        let mut messages = self.messages_with_turn_metadata();
        messages.push(hypothetical_user_message);

        // Transforms the turn loop would apply to this conversation between
        // dispatch and the wire. Detected read-only; nothing pending is
        // consumed, drained, or flushed by looking.
        let mut runtime_transforms = self
            .preview_runtime_transforms(&messages, &previewed_working_set, &planned_compaction)
            .await;

        // Resolve the same authoritative transient Work/To-do tail that the
        // production loop snapshots once per request. It is appended to the
        // outbound copy only: like production, auto-compaction and automatic
        // reasoning inspect stored history plus the submitted user message,
        // while preflight estimation and the provider body both receive this
        // exact same tail value. A graph read failure cannot fall back to a
        // stale legacy list in an exact preview.
        let work_state_tail = match self.work_state_source().exact_tail_message().await {
            Ok(tail) => tail,
            Err(error) => {
                return RequestManifest::build(ManifestDraft {
                    session,
                    route: Availability::unavailable_with(
                        UnavailableReason::WorkStateNotSnapshottable,
                        error.clone(),
                    ),
                    tools: Availability::unavailable_with(
                        UnavailableReason::WorkStateNotSnapshottable,
                        error.clone(),
                    ),
                    body: Availability::unavailable_with(
                        UnavailableReason::WorkStateNotSnapshottable,
                        error,
                    ),
                });
            }
        };

        // The turn loop resolves an `auto` sentinel tier against the messages
        // it is about to send, *after* the planner normalized it. Skipping
        // that step described a request carrying a literal `auto`, which no
        // route receives.
        let effective_reasoning_effort = super::turn_loop::resolve_auto_effort(
            reasoning_effort.as_deref(),
            &messages,
            provider,
            &base_url,
            &model,
        );

        let mut outbound_messages = messages.clone();
        if let Some(work_state_tail) = work_state_tail.as_ref() {
            outbound_messages.push(work_state_tail.clone());
        }

        // The production overflow gate estimates the logical messages and
        // system prompt, not serialized provider-body bytes. Use that same
        // contract here; the manifest keeps its wire estimate separately as
        // an observability metric.
        let base_input_estimate_tokens = crate::compaction::estimate_input_tokens_conservative(
            &messages,
            system_prompt.as_ref(),
        );
        let production_input_estimate_tokens =
            super::turn_loop::production_input_estimate_with_work_tail(
                base_input_estimate_tokens,
                work_state_tail.as_ref(),
            );

        let request = MessageRequest {
            model: model.clone(),
            messages: outbound_messages,
            max_tokens: effective_max_output_tokens_for_route(provider, &model, limits),
            system: system_prompt,
            tools: surface.active.clone(),
            tool_choice: tool_choice.clone(),
            metadata: None,
            thinking: None,
            reasoning_effort: effective_reasoning_effort,
            stream: Some(true),
            temperature: None,
            top_p: None,
        };

        let prepared = match route.client.prepare_outbound_request(request, true) {
            Ok(prepared) => prepared.with_route_id(route.identity.exact_id.clone()),
            Err(error) => {
                // Route identity is read *off the prepared request*, so a
                // preparation failure leaves the endpoint, wire model, and
                // dialect unknown too. The tool surface survives: it was built
                // before the body and does not depend on it.
                return RequestManifest::build(ManifestDraft {
                    session,
                    route: Availability::unavailable_with(
                        UnavailableReason::RequestPreparationFailed,
                        format!("{error:#}"),
                    ),
                    tools,
                    body: Availability::unavailable_with(
                        UnavailableReason::RequestPreparationFailed,
                        format!("{error:#}"),
                    ),
                });
            }
        };

        // `include` on a Responses body discloses reasoning output; it does not
        // ask the route to think. Treating any control key as a reasoning
        // request made every Codex turn read as an explicit user selection.
        let reasoning_resolution = if !prepared.reasoning.controls_reasoning() {
            ReasoningResolution::NotApplicable
        } else if reasoning_effort_auto {
            ReasoningResolution::ResolvedFromHypotheticalPrompt
        } else if prepared.reasoning.requested_effort.is_none() {
            ReasoningResolution::RouteDefault
        } else {
            ReasoningResolution::Explicit
        };

        // Headroom and overflow both follow production's message/system
        // estimator. When an earlier runtime transform cannot be observed
        // without mutation, `runtime_transforms` makes this body unavailable
        // rather than publishing a guess.
        let input_budget_ceiling_tokens =
            context_input_budget_for_route(provider, &model, limits, 0);
        if crate::request_manifest::production_input_budget_exceeded(
            input_budget_ceiling_tokens,
            production_input_estimate_tokens,
        ) {
            runtime_transforms
                .push("context-overflow recovery would trim or compact the conversation");
        }

        let route_facts = RouteFacts {
            provider_id: SafeLabel::identifier(&prepared.endpoint.provider_id),
            provider_display: SafeLabel::phrase(&prepared.endpoint.provider_display),
            route_id: prepared
                .endpoint
                .route_id
                .as_deref()
                .map(SafeLabel::identifier),
            dialect: prepared.dialect.as_str().to_string(),
            route_shape: prepared.endpoint.shape.as_str().to_string(),
            endpoint_host_class: prepared.safe_endpoint_host_class(),
            endpoint_fingerprint: prepared.endpoint_fingerprint(),
            wire_model: SafeLabel::catalog_model(&prepared.wire_model),
            caller_entrypoint: prepared.entrypoint.as_str().to_string(),
            body_stream_field: prepared.wire_stream_field(),
            context_limit_tokens: route.context_window.tokens,
            context_limit_source: route.context_window.source,
            route_input_limit_tokens: limits.and_then(|limits| limits.input_tokens),
            route_output_limit_tokens: limits.and_then(|limits| limits.output_tokens),
            billing: preview_billing_facts(&route.config, provider, &base_url),
            routing_source: routing_source.label().to_string(),
            auto_route_source: auto_route_source.as_deref().map(SafeLabel::phrase),
        };

        let prompt = self.preview_prompt_provenance(&prepared, system_prompt_text.as_str(), &model);

        // The body is a *dependent* fact. A tool surface whose MCP
        // contribution is unknown does not yield "the same body with no MCP
        // tools" — a real turn would connect and may send a different tool
        // list, a different tool region, and therefore a different body,
        // local component fingerprint, and hash. Publishing an exact body there was the reviewed
        // defect: it fabricated an empty MCP contribution and hashed it.
        // Likewise, a request the turn loop would rewrite before sending is
        // not the request that would be sent.
        let body = if let Some(inherited) = tools.propagate() {
            inherited
        } else if runtime_transforms.is_empty() {
            Availability::Exact(PreparedBodyInputs {
                prepared: &prepared,
                reasoning_resolution,
                prompt,
                input_budget_ceiling_tokens,
                production_input_estimate_tokens,
                tool_surface_is_exact: true,
            })
        } else {
            Availability::unavailable_with(
                UnavailableReason::RuntimeTransformsBeforeSend,
                runtime_transforms.join("; "),
            )
        };

        RequestManifest::build(ManifestDraft {
            session,
            route: Availability::Exact(route_facts),
            tools,
            body,
        })
    }

    /// Transforms the turn loop would apply to this conversation between
    /// dispatch and the first provider request.
    ///
    /// Every check is **read-only**. Nothing here drains the steer channel,
    /// receives a queued sub-agent completion, flushes an LSP block, or runs
    /// compaction: an inspection that consumed pending state would change the
    /// very turn it claims to describe. Where a queue can only be *counted*
    /// rather than inspected, counting is what happens.
    ///
    /// Returned strings are compile-time constants. They are joined into a
    /// typed unavailable detail, which still crosses the safe-label boundary.
    async fn preview_runtime_transforms(
        &self,
        messages: &[Message],
        working_set: &crate::working_set::WorkingSet,
        compaction: &crate::compaction::CompactionConfig,
    ) -> Vec<&'static str> {
        let mut reasons = Vec::new();

        if !self.pending_lsp_blocks.is_empty() {
            reasons.push("pending LSP diagnostics would be injected as a synthetic message");
        }

        let shell_completion_may_be_injected = self
            .shell_manager
            .lock()
            .map_or(true, |manager| manager.may_have_undelivered_completion());
        if shell_completion_may_be_injected {
            reasons.push("a background shell completion may be injected before the request");
        }

        let queued_completions = !self.rx_subagent_completion.is_empty() || {
            let manager = self.subagent_manager.read().await;
            manager.may_transform_next_parent_request(&self.delivered_subagent_completion_ids)
        };
        if queued_completions {
            reasons.push("a running or undelivered sub-agent completion may be injected");
        }

        if compaction.enabled {
            let pins = self.compaction_pins_for_messages(messages, working_set);
            let paths = working_set.top_paths(24);
            if should_compact(
                messages,
                compaction,
                Some(&self.session.workspace),
                Some(&pins),
                Some(&paths),
            ) {
                reasons.push("auto-compaction would rewrite the conversation first");
            }
        }

        reasons
    }

    /// Posture that depends on neither the route nor the next message.
    fn preview_session_facts(&self, inputs: &PreviewRequestInputs) -> SessionFacts {
        let base = crate::prompts::effective_base_prompt_text();
        let input_policy = effective_input_policy(
            inputs.provenance,
            inputs.mode,
            "",
            inputs.allow_shell,
            inputs.trust_mode,
            inputs.mode == AppMode::Yolo || inputs.auto_approve,
            inputs.approval_mode,
        );
        SessionFacts {
            agent_role: "primary".to_string(),
            lane_kind: "interactive-primary".to_string(),
            fleet_assignment: "not-applicable-primary-agent".to_string(),
            requested_model: SafeLabel::catalog_model(&inputs.requested_model),
            auto_model_routing: inputs.auto_model,
            requested_reasoning: SafeLabel::identifier(&inputs.requested_reasoning),
            // What the caller supplied, not what planning managed to do with
            // it: a plan failure must not read as "you forgot `--prompt`".
            hypothetical_prompt_supplied: inputs.hypothetical_prompt_supplied,
            mode: input_policy.mode.label().to_string(),
            approval_mode: format!("{:?}", input_policy.approval_mode_for_session()),
            allowed_tool_gate_count: inputs.allowed_tools.as_ref().map(Vec::len),
            disallowed_tool_gate_count: self.config.disallowed_tools.as_ref().map(Vec::len),
            base_prompt: BasePromptProvenance {
                origin: crate::prompts::base_prompt_origin().label().to_string(),
                bytes: base.len(),
                sha256: crate::hashing::sha256_hex(base.as_bytes()),
            },
        }
    }

    /// System-prompt provenance, as labels and hashes only.
    ///
    /// `effective` is the prompt of the request being described, so an
    /// auto-routed preview classifies the prompt it would actually send
    /// rather than the session's currently installed one.
    fn preview_prompt_provenance(
        &self,
        prepared: &PreparedOutboundRequest,
        effective: &str,
        model: &str,
    ) -> PromptProvenance {
        let base = crate::prompts::effective_base_prompt_text();
        let configured =
            crate::prompts::compose_default_static_layers(crate::prompts::Personality::Calm, model);

        let assembly = if effective.trim().is_empty() {
            SystemPromptAssembly::None
        } else if effective.trim() == base.trim() {
            SystemPromptAssembly::BaseOnly
        } else if effective.trim() == configured.trim() {
            SystemPromptAssembly::BaseWithConfiguredLayers
        } else {
            SystemPromptAssembly::BaseWithRuntimeAdditions
        };

        let view = prepared.wire_view();
        PromptProvenance {
            assembly,
            // The hash of the prompt the *prepared request* carries, in its
            // final wire form — not of an independently recomposed string.
            effective_system_canonical_json_bytes: view.system_bytes,
            effective_system_sha256: view.system_sha256.clone(),
        }
    }
}

/// Typed billing facts for the planned route, from the same helper the footer
/// and sidebar read. Every label is a compile-time constant.
fn preview_billing_facts(
    config: &crate::config::Config,
    provider: crate::config::ApiProvider,
    base_url: &str,
) -> BillingFacts {
    if let Some(surface) = crate::pricing::billing_surface_for_route(provider, Some(base_url)) {
        return BillingFacts::Surface { surface };
    }
    match crate::route_billing::for_route(config, provider) {
        crate::route_billing::BillingPresentation::Metered => BillingFacts::Metered,
        crate::route_billing::BillingPresentation::Subscription(plan) => {
            BillingFacts::Subscription { plan }
        }
        crate::route_billing::BillingPresentation::Local => BillingFacts::Local,
        crate::route_billing::BillingPresentation::Unknown => BillingFacts::Unknown,
    }
}

/// Stable hash over the exact active tool catalog: name, description, and
/// schema, in catalog order. Changes when a tool is added, removed,
/// reordered, or has its schema transformed.
///
/// This is the *single* definition of the active-tool-catalog digest. The
/// request manifest fills `ToolSurfaceFacts::active_tool_catalog_sha256` from
/// it, and `crate::tool_inspection` reports the same value for the same
/// prepared request. Neither surface keeps a digest of its own, so a human
/// reading `/tools` and a human reading `/request` are looking at the same
/// accounting object rather than two hashes that can silently diverge.
pub(crate) fn active_tool_catalog_sha256(tools: &[Tool]) -> String {
    let mut canonical = String::new();
    for tool in tools {
        canonical.push_str(&tool.name);
        canonical.push('\u{1}');
        canonical.push_str(&tool.description);
        canonical.push('\u{1}');
        canonical.push_str(&crate::client::canonical_json(&tool.input_schema));
        canonical.push('\n');
    }
    crate::hashing::sha256_hex(canonical.as_bytes())
}

/// Whether the Standard and Full tool surfaces currently produce the same
/// catalog.
///
/// Derived, not asserted: the surface shaper is run over this exact catalog
/// under both budgets and the results compared. If Standard and Full ever
/// genuinely diverge, this reports `false` without anyone editing copy.
fn standard_and_full_collapse(
    catalog: &[Tool],
    always_load: &std::collections::HashSet<String>,
) -> bool {
    super::tool_catalog::surface_budgets_produce_same_catalog(
        catalog,
        always_load,
        crate::model_profile::ToolSurfaceBudget::Standard,
        crate::model_profile::ToolSurfaceBudget::Full,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_unavailable_reason<T>(availability: &Availability<T>, expected: UnavailableReason) {
        let Availability::Unavailable(unavailable) = availability else {
            panic!("expected typed unavailable state");
        };
        assert_eq!(unavailable.reason, expected);
    }

    fn tool(name: &str, deferred: bool) -> Tool {
        Tool {
            tool_type: None,
            name: name.to_string(),
            description: format!("{name} description"),
            input_schema: json!({"type": "object", "properties": {}}),
            allowed_callers: None,
            defer_loading: Some(deferred),
            input_examples: None,
            strict: None,
            cache_control: None,
        }
    }

    #[test]
    fn active_catalog_hash_tracks_membership_order_and_schema() {
        let base = vec![tool("Bash", false), tool("File", false)];
        let baseline = active_tool_catalog_sha256(&base);

        assert_eq!(baseline, active_tool_catalog_sha256(&base.clone()));

        let mut reordered = base.clone();
        reordered.swap(0, 1);
        assert_ne!(baseline, active_tool_catalog_sha256(&reordered));

        let mut fewer = base.clone();
        fewer.pop();
        assert_ne!(baseline, active_tool_catalog_sha256(&fewer));

        let mut retyped = base.clone();
        retyped[0].input_schema = json!({"type": "object", "required": ["cmd"]});
        assert_ne!(baseline, active_tool_catalog_sha256(&retyped));
    }

    #[test]
    fn work_tail_context_ceiling_uses_production_headroom_and_no_send_decision() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "near the context ceiling".to_string(),
                cache_control: None,
            }],
        }];
        let work_tail =
            crate::work_grounding::work_state_message(&crate::tools::todo::TodoListSnapshot {
                items: vec![crate::tools::todo::TodoItem {
                    id: 1,
                    content: "retain the separately framed Work tail".to_string(),
                    status: crate::tools::todo::TodoStatus::InProgress,
                }],
                completion_pct: 0,
                in_progress_id: Some(1),
            })
            .expect("nonempty Work tail");
        let system = SystemPrompt::Text("stable system".to_string());
        let base = crate::compaction::estimate_input_tokens_conservative(&messages, Some(&system));
        let production =
            super::turn_loop::production_input_estimate_with_work_tail(base, Some(&work_tail));

        let mut combined_messages = messages;
        combined_messages.push(work_tail);
        let combined_once = crate::compaction::estimate_input_tokens_conservative(
            &combined_messages,
            Some(&system),
        );
        assert!(
            production > combined_once,
            "the production decomposition charges a second fixed framing overhead"
        );

        // This is the reviewed edge: the old combined-once estimate would
        // allow a send, while production is one token over its ceiling. The
        // same signed-headroom helper now drives both the manifest number and
        // preview's overflow/no-exact-outbound decision.
        let ceiling = production - 1;
        assert_eq!(
            crate::request_manifest::production_input_headroom(Some(ceiling), production),
            Some(-1)
        );
        assert!(crate::request_manifest::production_input_budget_exceeded(
            Some(ceiling),
            production
        ));
        assert!(!crate::request_manifest::production_input_budget_exceeded(
            Some(ceiling),
            combined_once
        ));
    }

    #[test]
    fn standard_and_full_are_reported_collapsed_from_the_real_shaper() {
        let catalog = vec![tool("Bash", false), tool("agent", false), tool("Web", true)];
        let always_load = std::collections::HashSet::new();
        assert!(
            standard_and_full_collapse(&catalog, &always_load),
            "Standard and Full apply no narrowing today, so they must report collapsed"
        );
    }

    #[tokio::test]
    async fn pending_shell_completion_makes_the_body_unavailable_without_draining_it() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);

        {
            let mut manager = engine.shell_manager.lock().expect("shell manager");
            let command = if cfg!(windows) {
                "Start-Sleep -Seconds 30"
            } else {
                "sleep 30"
            };
            manager
                .execute(command, None, 30_000, true)
                .expect("background shell starts");
            assert!(manager.may_have_undelivered_completion());
        }

        let planned = plan(&config, &identity, false, "inspect the next request").await;
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), "inspect the next request"))
            .await;
        let unavailable = match &manifest.body {
            Availability::Unavailable(unavailable) => unavailable,
            Availability::Exact(_) => panic!("pending shell completion must fail closed"),
        };
        assert_eq!(
            unavailable.reason,
            UnavailableReason::RuntimeTransformsBeforeSend
        );
        assert!(
            unavailable
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("background shell completion")),
            "{unavailable:?}"
        );

        let mut manager = engine.shell_manager.lock().expect("shell manager");
        assert!(
            manager.may_have_undelivered_completion(),
            "preview must not drain or report the completion"
        );
        let _ = manager.kill_running();
        let _ = manager.drain_finished_jobs_with_evidence();
    }

    #[tokio::test]
    async fn running_direct_child_fails_closed_without_consuming_or_mutating_state() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        let mut before = {
            let mut manager = engine.subagent_manager.write().await;
            manager.insert_test_running_direct_child("preview_pending", tmp.path());
            serde_json::to_value(manager.list()).expect("manager snapshot")
        };
        if let Some(rows) = before.as_array_mut() {
            for row in rows {
                row.as_object_mut()
                    .expect("agent object")
                    .remove("duration_ms");
            }
        }
        let delivered_before = engine.delivered_subagent_completion_ids.clone();

        let planned = plan(&config, &identity, false, "inspect while child runs").await;
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), "inspect while child runs"))
            .await;
        let unavailable = match &manifest.body {
            Availability::Unavailable(unavailable) => unavailable,
            Availability::Exact(_) => panic!("running child must fail closed"),
        };
        assert_eq!(
            unavailable.reason,
            UnavailableReason::RuntimeTransformsBeforeSend
        );
        assert!(
            unavailable
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("running or undelivered sub-agent"))
        );

        let mut after = {
            let manager = engine.subagent_manager.read().await;
            assert!(
                manager
                    .may_transform_next_parent_request(&engine.delivered_subagent_completion_ids)
            );
            serde_json::to_value(manager.list()).expect("manager snapshot")
        };
        if let Some(rows) = after.as_array_mut() {
            for row in rows {
                row.as_object_mut()
                    .expect("agent object")
                    .remove("duration_ms");
            }
        }
        assert_eq!(before, after, "preview must not mutate child state");
        assert_eq!(
            delivered_before, engine.delivered_subagent_completion_ids,
            "preview must not claim child delivery"
        );
    }

    #[tokio::test]
    async fn terminal_undelivered_child_fails_closed_without_claiming_delivery() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        let agent_id = {
            let mut manager = engine.subagent_manager.write().await;
            manager.insert_test_terminal_direct_child("preview_terminal", tmp.path())
        };

        let planned = plan(&config, &identity, false, "inspect settled child").await;
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), "inspect settled child"))
            .await;
        assert!(matches!(manifest.body, Availability::Unavailable(_)));
        assert!(
            !engine.delivered_subagent_completion_ids.contains(&agent_id),
            "preview must not claim terminal delivery"
        );
        let manager = engine.subagent_manager.read().await;
        assert!(
            manager.may_transform_next_parent_request(&engine.delivered_subagent_completion_ids)
        );
        assert!(matches!(
            manager
                .get_result(&agent_id)
                .expect("terminal child")
                .status,
            crate::tools::subagent::SubAgentStatus::Completed
        ));
    }

    #[test]
    fn turn_metadata_uses_planned_cross_route_limits_not_installed_limits() {
        let config = deepseek_config();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.api_provider = ApiProvider::Deepseek;
        let installed_limits = codewhale_config::route::RouteLimits {
            context_tokens: Some(4_096),
            input_tokens: None,
            output_tokens: Some(512),
        };
        engine.active_route_limits = Some(installed_limits);
        // Large enough to be critical for the installed 4K route, but safely
        // below the warning threshold for the planned 123K route.
        engine.session.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "x".repeat(20_000),
                cache_control: None,
            }],
        });
        let prompt_context = NextTurnPromptContext::for_planned_turn(
            ApiProvider::Openrouter,
            "qwen/qwen3.6-flash".to_string(),
            Some(codewhale_config::route::RouteLimits {
                context_tokens: Some(123_456),
                input_tokens: None,
                output_tokens: Some(4_096),
            }),
            AppMode::Agent,
            None,
            GoalStatus::Active,
            None,
            false,
            None,
        );
        let system_prompt = engine.compose_stable_system_prompt(&prompt_context);
        assert_eq!(
            engine.context_pressure_line(
                "cross-route budget",
                &prompt_context,
                system_prompt.as_ref()
            ),
            None,
            "the planned 123K route must not inherit the installed route's pressure"
        );
        let installed_context = NextTurnPromptContext::for_planned_turn(
            ApiProvider::Deepseek,
            "deepseek-v4-flash".to_string(),
            Some(installed_limits),
            AppMode::Agent,
            None,
            GoalStatus::Active,
            None,
            false,
            None,
        );
        assert_eq!(
            engine
                .context_pressure_line("cross-route budget", &installed_context, None)
                .as_deref(),
            Some(
                "Context pressure: critical — CRITICAL: stop expanding scope; run /compact immediately or finish the current task"
            ),
            "control fixture must be critical under the installed 4K limits"
        );
        let message = engine.user_text_message_from_snapshot(
            "cross-route budget".to_string(),
            &prompt_context.model,
            true,
            None,
            false,
            UserInputProvenance::ExternalUser,
            TurnMetadataSnapshot {
                prompt_context: &prompt_context,
                system_prompt: system_prompt.as_ref(),
                approval_mode: crate::tui::approval::ApprovalMode::Suggest,
                working_set: &engine.session.working_set,
                policy_narrowing: None,
            },
        );
        let metadata = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .next_back()
            .expect("turn metadata text");
        assert!(
            !metadata.contains("Context pressure:"),
            "planned route metadata must remain below warning: {metadata}"
        );
        assert!(!metadata.contains("123456 tokens"), "{metadata}");
        assert!(!metadata.contains("4096 tokens"), "{metadata}");
    }

    #[tokio::test]
    async fn planned_route_builds_subagent_catalog_without_installed_client() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        let _ = engine.config.features.enable(Feature::Subagents);
        engine.config.subagents_enabled = true;
        engine.deepseek_client = None;
        let planned = plan(&config, &identity, false, "planned child route").await;
        let route = planned.route.validate().expect("planned route validates");
        let planned_model = route.model.clone();
        let policy = TurnAuthority::from_effective_fields(
            AppMode::Agent,
            false,
            false,
            false,
            crate::tui::approval::ApprovalMode::Suggest,
        );
        let build = engine
            .build_turn_tool_registry_and_catalog(
                &policy,
                &[],
                None,
                SubAgentWiring::Inert,
                McpAccess::PassiveSnapshot,
                TurnRouteContext {
                    provider: route.identity.provider,
                    model: route.model.clone(),
                    capabilities: route.candidate.capabilities(),
                    limits: crate::route_budget::known_route_limits(route.candidate.limits()),
                    client: Some(route.client),
                    api_config: route.config,
                    locale_tag: engine.config.locale_tag.clone(),
                    role_models: engine.subagent_role_models(),
                    fleet_roster: engine.config.fleet_roster.clone(),
                    auto_model: false,
                    reasoning_effort: planned.effective_reasoning_effort,
                    reasoning_effort_auto: planned.auto_controls_reasoning,
                },
                "",
            )
            .await;
        assert!(
            build
                .surface
                .catalog
                .iter()
                .any(|tool| tool.name == "agent"),
            "the planned route client must make sub-agent tools available"
        );
        assert_eq!(
            build.subagent_runtime_model.as_deref(),
            Some(planned_model.as_str()),
            "the child runtime must carry the planned route model"
        );
    }

    /// Auto routing with no hypothetical prompt: every route-derived fact is
    /// structurally absent, and the flag is never cleared just because the
    /// session happens to have an installed route.
    #[tokio::test]
    async fn auto_route_without_a_prompt_omits_every_final_fact() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut engine, _handle) = Engine::new(
            EngineConfig {
                workspace: tmp.path().to_path_buf(),
                ..Default::default()
            },
            &crate::config::Config::default(),
        );

        let manifest = engine
            .build_request_manifest(PreviewRequestInputs {
                mode: AppMode::Agent,
                allow_shell: false,
                trust_mode: false,
                auto_approve: false,
                approval_mode: crate::tui::approval::ApprovalMode::Suggest,
                allowed_tools: None,
                dynamic_tools: Vec::new(),
                provenance: UserInputProvenance::ExternalUser,
                requested_model: "auto".to_string(),
                requested_reasoning: "auto".to_string(),
                auto_model: true,
                hypothetical_prompt_supplied: false,
                next_turn: None,
                unresolved: PreviewUnresolved::AutoRouteNeedsPrompt,
            })
            .await;

        assert!(manifest.route.exact().is_none());
        assert!(manifest.tools.exact().is_none());
        assert!(manifest.body.exact().is_none());
        assert_eq!(manifest.session.requested_model.as_str(), "auto");
        assert!(manifest.session.auto_model_routing);
        assert!(!manifest.session.hypothetical_prompt_supplied);

        let json = manifest.to_json();
        for forbidden in [
            "provider_id",
            "wire_model",
            "endpoint_fingerprint",
            "body_sha256",
            "tool_surface_budget",
            "billing",
        ] {
            assert!(!json.contains(forbidden), "{forbidden} leaked:\n{json}");
        }
    }

    fn deepseek_config() -> crate::config::Config {
        let providers = crate::config::ProvidersConfig {
            deepseek: crate::config::ProviderConfig {
                api_key: Some("sk-test-deepseek".to_string()),
                model: Some("deepseek-chat".to_string()),
                ..crate::config::ProviderConfig::default()
            },
            ..crate::config::ProvidersConfig::default()
        };
        crate::config::Config {
            provider: Some("deepseek".to_string()),
            providers: Some(providers),
            ..crate::config::Config::default()
        }
    }

    fn deepseek_identity() -> crate::config::ProviderIdentity {
        crate::config::ProviderIdentity {
            provider: ApiProvider::Deepseek,
            key: "deepseek".to_string(),
            exact_id: None,
        }
    }

    /// Run the *production* route planner, provider-free: with `auto_model`
    /// the classifier short-circuits to the inventory heuristic under `cfg!(test)`.
    async fn plan(
        config: &crate::config::Config,
        identity: &crate::config::ProviderIdentity,
        auto_model: bool,
        prompt: &str,
    ) -> crate::turn_route_plan::PlannedTurnRoute {
        plan_for(
            config,
            identity,
            ApiProvider::Deepseek,
            "deepseek-chat",
            auto_model,
            prompt,
        )
        .await
    }

    async fn plan_for(
        config: &crate::config::Config,
        identity: &crate::config::ProviderIdentity,
        provider: ApiProvider,
        model: &str,
        auto_model: bool,
        prompt: &str,
    ) -> crate::turn_route_plan::PlannedTurnRoute {
        plan_with_reasoning(
            config,
            identity,
            provider,
            model,
            auto_model,
            if auto_model {
                crate::tui::app::ReasoningEffort::Auto
            } else {
                crate::tui::app::ReasoningEffort::High
            },
            prompt,
        )
        .await
    }

    /// `plan_for` with the requested reasoning tier under test control. The
    /// exact-route matrix needs `off` to observe a route that normalizes it
    /// (direct Moonshot K3 sends `low`).
    async fn plan_with_reasoning(
        config: &crate::config::Config,
        identity: &crate::config::ProviderIdentity,
        provider: ApiProvider,
        model: &str,
        auto_model: bool,
        reasoning_effort: crate::tui::app::ReasoningEffort,
        prompt: &str,
    ) -> crate::turn_route_plan::PlannedTurnRoute {
        crate::turn_route_plan::plan_turn_route(crate::turn_route_plan::TurnRoutePlanRequest {
            route_config: config,
            app_route_identity: identity,
            api_provider: provider,
            app_model: model,
            auto_model,
            reasoning_effort,
            mode: AppMode::Agent,
            content: prompt,
            display_text: prompt,
            auto_router_context: "",
            should_auto_resolve: auto_model,
            allow_auto_router_response_cache: false,
            preflight_required: false,
            auto_compact_user_configured: false,
            auto_compact: true,
            auto_compact_threshold_percent: 80.0,
        })
        .await
        .expect("the shared planner resolves a configured route")
    }

    fn preview_engine(config: &crate::config::Config) -> (Engine, EngineHandle, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (engine, handle) = Engine::new(
            EngineConfig {
                workspace: tmp.path().to_path_buf(),
                ..Default::default()
            },
            config,
        );
        (engine, handle, tmp)
    }

    fn wire_preview_engine(config: &crate::config::Config) -> (Engine, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut engine, _handle) = Engine::new(
            EngineConfig {
                workspace: tmp.path().to_path_buf(),
                max_steps: 1,
                snapshots_enabled: false,
                terminal_chrome_enabled: false,
                ..Default::default()
            },
            config,
        );
        engine.config.features.disable(Feature::Mcp);
        engine.config.subagents_enabled = false;
        (engine, tmp)
    }

    fn inputs(
        auto_model: bool,
        planned: Option<crate::turn_route_plan::PlannedTurnRoute>,
        prompt: &str,
    ) -> PreviewRequestInputs {
        PreviewRequestInputs {
            mode: AppMode::Agent,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            provenance: UserInputProvenance::ExternalUser,
            requested_model: if auto_model {
                "auto".to_string()
            } else {
                "deepseek-chat".to_string()
            },
            requested_reasoning: if auto_model { "auto" } else { "high" }.to_string(),
            auto_model,
            hypothetical_prompt_supplied: true,
            next_turn: planned.map(|planned| {
                let prompt_context = NextTurnPromptContext::for_planned_turn(
                    planned.route.identity.provider,
                    planned.route.model.clone(),
                    crate::route_budget::known_route_limits(planned.route.candidate.limits()),
                    AppMode::Agent,
                    None,
                    GoalStatus::Active,
                    None,
                    false,
                    None,
                );
                Box::new(PreviewNextTurn {
                    content: prompt.to_string(),
                    route: Box::new(planned.route),
                    prompt_context,
                    reasoning_effort: planned.effective_reasoning_effort,
                    reasoning_effort_auto: planned.auto_controls_reasoning,
                    auto_route_source: planned
                        .auto_selection
                        .as_ref()
                        .map(|selection| selection.source.label().to_string()),
                    routing_source: planned.routing_source,
                    compaction: planned.compaction,
                })
            }),
            unresolved: PreviewUnresolved::NoPrompt,
        }
    }

    /// Typed controls for the preview/wire parity fixture. Defaults mirror the
    /// ordinary active DeepSeek turn; individual tests override only the
    /// production context they are proving.
    struct PreviewWireFixture {
        goal_objective: Option<String>,
        goal_status: GoalStatus,
        translation_enabled: bool,
        verbosity: Option<String>,
        requested_model: Option<String>,
        requested_reasoning: Option<String>,
    }

    impl Default for PreviewWireFixture {
        fn default() -> Self {
            Self {
                goal_objective: None,
                goal_status: GoalStatus::Active,
                translation_enabled: false,
                verbosity: None,
                requested_model: None,
                requested_reasoning: None,
            }
        }
    }

    async fn assert_preview_matches_first_wire_body(
        engine: &mut Engine,
        server: &wiremock::MockServer,
        planned: crate::turn_route_plan::PlannedTurnRoute,
        prompt: &str,
        fixture: PreviewWireFixture,
    ) -> (RequestManifest, serde_json::Value) {
        let PreviewWireFixture {
            goal_objective,
            goal_status,
            translation_enabled,
            verbosity,
            requested_model,
            requested_reasoning,
        } = fixture;
        let production_route = planned.route.clone();
        let compaction = planned.compaction.clone();
        let reasoning_effort = planned.effective_reasoning_effort.clone();
        let reasoning_effort_auto = planned.auto_controls_reasoning;
        let mut preview_inputs = inputs(false, Some(planned), prompt);
        if let Some(requested_model) = requested_model {
            preview_inputs.requested_model = requested_model;
        }
        if let Some(requested_reasoning) = requested_reasoning {
            preview_inputs.requested_reasoning = requested_reasoning;
        }
        let next = preview_inputs.next_turn.as_mut().expect("planned preview");
        next.prompt_context = NextTurnPromptContext::for_planned_turn(
            production_route.identity.provider,
            production_route.model.clone(),
            crate::route_budget::known_route_limits(production_route.candidate.limits()),
            AppMode::Agent,
            goal_objective.clone(),
            goal_status,
            None,
            translation_enabled,
            verbosity.clone(),
        );
        let manifest = engine.build_request_manifest(preview_inputs).await;
        let preview_hash = manifest
            .body
            .exact()
            .expect("preview body is exact")
            .body_sha256
            .clone();

        let _ = engine
            .handle_send_message(
                prompt.to_string(),
                AppMode::Agent,
                production_route,
                compaction,
                goal_objective,
                None,
                goal_status,
                reasoning_effort,
                reasoning_effort_auto,
                false,
                false,
                false,
                false,
                crate::tui::approval::ApprovalMode::Suggest,
                translation_enabled,
                None,
                Vec::new(),
                None,
                verbosity,
                UserInputProvenance::ExternalUser,
            )
            .await;

        let requests = server
            .received_requests()
            .await
            .expect("wire mock records requests");
        assert_eq!(requests.len(), 1, "the fixture must make one provider call");
        let first_wire_body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("first HTTP body is JSON");
        let first_wire_hash =
            crate::hashing::sha256_hex(crate::client::canonical_json(&first_wire_body).as_bytes());
        assert_eq!(
            preview_hash, first_wire_hash,
            "preview hash must match the body captured at the HTTP boundary"
        );
        (manifest, first_wire_body)
    }

    #[tokio::test]
    async fn graph_backed_work_tail_matches_the_first_http_body() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .mount(&server)
            .await;
        let mut config = deepseek_config();
        config
            .providers
            .as_mut()
            .expect("providers")
            .deepseek
            .base_url = Some(server.uri());
        let identity = deepseek_identity();
        let (mut engine, _tmp) = wire_preview_engine(&config);
        let graph_todos = crate::tools::todo::TodoListSnapshot {
            items: vec![crate::tools::todo::TodoItem {
                id: 1,
                content: "preserve this graph-authoritative Work item".to_string(),
                status: crate::tools::todo::TodoStatus::InProgress,
            }],
            completion_pct: 0,
            in_progress_id: Some(1),
        };
        let work = crate::work_graph::new_shared_work_runtime(
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        );
        work.restore(
            "preview-graph-work",
            None,
            &graph_todos,
            &crate::tools::plan::PlanSnapshot::default(),
        )
        .expect("restore graph-backed Work state");
        *engine.config.todos.lock().await = crate::tools::todo::TodoList::new();
        assert!(
            engine.config.todos.lock().await.snapshot().is_empty(),
            "legacy projection is intentionally stale for this authority test"
        );
        engine.config.runtime_services.work = Some(work);

        let prompt = "inspect the request with live Work state";
        let planned = plan(&config, &identity, false, prompt).await;
        let (_, first_wire_body) = assert_preview_matches_first_wire_body(
            &mut engine,
            &server,
            planned,
            prompt,
            PreviewWireFixture::default(),
        )
        .await;
        let body_text = first_wire_body.to_string();
        assert!(body_text.contains("<codewhale:work_state>"), "{body_text}");
        assert!(
            body_text.contains("preserve this graph-authoritative Work item"),
            "{body_text}"
        );
    }

    #[tokio::test]
    async fn exhausted_active_goal_has_no_exact_outbound_request() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        sync_goal_state_from_host(
            &engine.config.goal_state,
            Some("finish the release"),
            Some(100),
            GoalStatus::Active,
        );
        engine
            .config
            .goal_state
            .lock()
            .expect("goal state")
            .record_usage(100, 0);

        let prompt = "continue the release";
        let planned = plan(&config, &identity, false, prompt).await;
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), prompt))
            .await;
        assert_unavailable_reason(&manifest.route, UnavailableReason::GoalTokenBudgetExhausted);
        assert_unavailable_reason(&manifest.tools, UnavailableReason::GoalTokenBudgetExhausted);
        assert_unavailable_reason(&manifest.body, UnavailableReason::GoalTokenBudgetExhausted);
    }

    #[tokio::test]
    async fn resumed_goal_with_raised_budget_becomes_previewable_again() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        sync_goal_state_from_host(
            &engine.config.goal_state,
            Some("finish the release"),
            Some(100),
            GoalStatus::Active,
        );
        {
            let mut state = engine.config.goal_state.lock().expect("goal state");
            state.record_usage(100, 0);
            state
                .mark_paused(GoalPauseReason::BudgetLimit)
                .expect("pause goal");
        }
        sync_goal_state_from_host(
            &engine.config.goal_state,
            Some("finish the release"),
            Some(200),
            GoalStatus::Active,
        );

        let prompt = "continue under the raised budget";
        let planned = plan(&config, &identity, false, prompt).await;
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), prompt))
            .await;
        assert!(manifest.body.exact().is_some());
    }

    #[tokio::test]
    async fn lowering_active_goal_budget_below_used_tokens_closes_preview_gate() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        sync_goal_state_from_host(
            &engine.config.goal_state,
            Some("finish the release"),
            Some(200),
            GoalStatus::Active,
        );
        engine
            .config
            .goal_state
            .lock()
            .expect("goal state")
            .record_usage(100, 0);
        sync_goal_state_from_host(
            &engine.config.goal_state,
            Some("finish the release"),
            Some(50),
            GoalStatus::Active,
        );

        let prompt = "continue after lowering the budget";
        let planned = plan(&config, &identity, false, prompt).await;
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), prompt))
            .await;
        let Availability::Unavailable(unavailable) = manifest.body else {
            panic!("lowering the budget below usage must close the outbound gate");
        };
        assert_eq!(
            unavailable.reason,
            UnavailableReason::GoalTokenBudgetExhausted
        );
    }

    #[tokio::test]
    async fn translation_prompt_context_matches_captured_first_production_body() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .mount(&server)
            .await;
        let mut config = deepseek_config();
        config
            .providers
            .as_mut()
            .expect("providers")
            .deepseek
            .base_url = Some(server.uri());
        let identity = deepseek_identity();
        let (mut engine, _tmp) = wire_preview_engine(&config);
        engine.config.translation_enabled = false;
        let planned = plan(&config, &identity, false, "/translate explain this").await;
        let _ = assert_preview_matches_first_wire_body(
            &mut engine,
            &server,
            planned,
            "/translate explain this",
            PreviewWireFixture {
                translation_enabled: true,
                verbosity: Some("concise".to_string()),
                ..Default::default()
            },
        )
        .await;
    }

    #[tokio::test]
    async fn paused_detach_goal_context_matches_captured_first_production_body() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .mount(&server)
            .await;
        let mut config = deepseek_config();
        config
            .providers
            .as_mut()
            .expect("providers")
            .deepseek
            .base_url = Some(server.uri());
        let identity = deepseek_identity();
        let (mut engine, _tmp) = wire_preview_engine(&config);
        engine.config.goal_objective = Some("stale paused objective".to_string());
        sync_goal_state_from_host(
            &engine.config.goal_state,
            Some("stale paused objective"),
            None,
            GoalStatus::Active,
        );
        let prompt = "answer only this new question\n\nCodewhale paused custom slash command context:\nThe user is not resuming that paused command.";
        let planned = plan(&config, &identity, false, prompt).await;
        let (_, first_wire_body) = assert_preview_matches_first_wire_body(
            &mut engine,
            &server,
            planned,
            prompt,
            PreviewWireFixture::default(),
        )
        .await;
        assert!(
            !first_wire_body
                .to_string()
                .contains("stale paused objective"),
            "detached paused goal leaked onto the first wire body"
        );
    }

    #[tokio::test]
    async fn anthropic_preview_matches_the_first_native_messages_wire_body() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: {\"type\":\"message_stop\"}\n\n"),
            )
            .mount(&server)
            .await;
        let model = "claude-sonnet-4-6";
        let config = crate::config::Config {
            provider: Some("anthropic".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                anthropic: crate::config::ProviderConfig {
                    api_key: Some("test-anthropic-key".to_string()),
                    base_url: Some(server.uri()),
                    model: Some(model.to_string()),
                    ..crate::config::ProviderConfig::default()
                },
                ..crate::config::ProvidersConfig::default()
            }),
            ..crate::config::Config::default()
        };
        let identity = crate::config::ProviderIdentity {
            provider: ApiProvider::Anthropic,
            key: "anthropic".to_string(),
            exact_id: None,
        };
        let (mut engine, _tmp) = wire_preview_engine(&config);
        let prompt = "inspect the native Messages payload";
        let planned = plan_for(
            &config,
            &identity,
            ApiProvider::Anthropic,
            model,
            false,
            prompt,
        )
        .await;
        let (_, first_wire_body) = assert_preview_matches_first_wire_body(
            &mut engine,
            &server,
            planned,
            prompt,
            PreviewWireFixture::default(),
        )
        .await;

        assert!(first_wire_body.get("system").is_some());
        assert!(first_wire_body.get("messages").is_some());
        assert!(first_wire_body.get("input").is_none());
    }

    // ---------------------------------------------------------------------
    // #4707 — provider-free exact-route request/receipt matrix.
    //
    // The four route wire-truths are already pinned at the client boundary
    // (`client.rs`, `client/chat.rs`). What was missing is the *join*: that the
    // manifest a user reads from `/preview-request` describes the very bytes
    // those routes put on the wire. Each case below runs the production
    // planner, previews, then sends one turn through a local capture server
    // with the semantic endpoint left exact, and asserts the manifest against
    // the captured body — hash, sizes, route facts, and the requested→effective
    // reasoning triple.
    // ---------------------------------------------------------------------

    /// One exact provider route in the matrix.
    struct MatrixRoute {
        /// Test-facing name; also the failure-message prefix.
        name: &'static str,
        provider: ApiProvider,
        provider_key: &'static str,
        base_url: &'static str,
        model: &'static str,
        requested_reasoning: crate::tui::app::ReasoningEffort,
        requested_reasoning_label: &'static str,
        /// Reasoning-control keys the manifest must report, in receipt order.
        expect_control_keys: &'static [&'static str],
        /// Effort actually on the wire — `None` when the route publishes a
        /// thinking toggle with no granularity. Never a fabricated tier.
        expect_wire_effort: Option<&'static str>,
        expect_wire_effort_source: Option<&'static str>,
        /// The output-cap key this route writes, and the one it must not.
        expect_output_cap_key: &'static str,
        expect_absent_output_cap_key: &'static str,
    }

    fn glm_5_2_zai_coding() -> MatrixRoute {
        MatrixRoute {
            name: "GLM-5.2 @ Z.ai coding",
            provider: ApiProvider::Zai,
            provider_key: "zai",
            base_url: crate::config::DEFAULT_ZAI_BASE_URL,
            model: crate::config::ZAI_GLM_5_2_MODEL,
            requested_reasoning: crate::tui::app::ReasoningEffort::High,
            requested_reasoning_label: "high",
            expect_control_keys: &["reasoning_effort", "thinking"],
            expect_wire_effort: Some("high"),
            expect_wire_effort_source: Some("reasoning_effort"),
            expect_output_cap_key: "max_tokens",
            expect_absent_output_cap_key: "max_completion_tokens",
        }
    }

    fn glm_5_turbo_zai() -> MatrixRoute {
        MatrixRoute {
            name: "GLM-5-Turbo @ Z.ai",
            provider: ApiProvider::Zai,
            provider_key: "zai",
            base_url: crate::config::DEFAULT_ZAI_BASE_URL,
            model: crate::config::ZAI_GLM_5_TURBO_MODEL,
            requested_reasoning: crate::tui::app::ReasoningEffort::High,
            requested_reasoning_label: "high",
            // No invented granularity: the toggle ships, the tier does not.
            expect_control_keys: &["thinking"],
            expect_wire_effort: None,
            expect_wire_effort_source: None,
            expect_output_cap_key: "max_tokens",
            expect_absent_output_cap_key: "max_completion_tokens",
        }
    }

    fn kimi_k3_moonshot_direct() -> MatrixRoute {
        MatrixRoute {
            name: "kimi-k3 @ api.moonshot.ai",
            provider: ApiProvider::Moonshot,
            provider_key: "moonshot",
            base_url: crate::config::DEFAULT_MOONSHOT_BASE_URL,
            model: crate::config::MOONSHOT_KIMI_K3_MODEL,
            // The visible normalization: `off` is not a tier this route has.
            requested_reasoning: crate::tui::app::ReasoningEffort::Off,
            requested_reasoning_label: "off",
            expect_control_keys: &["reasoning_effort"],
            expect_wire_effort: Some("low"),
            expect_wire_effort_source: Some("reasoning_effort"),
            expect_output_cap_key: "max_completion_tokens",
            expect_absent_output_cap_key: "max_tokens",
        }
    }

    fn k3_kimi_code() -> MatrixRoute {
        MatrixRoute {
            name: "k3 @ api.kimi.com/coding/v1",
            provider: ApiProvider::Moonshot,
            provider_key: "moonshot",
            base_url: crate::config::DEFAULT_KIMI_CODE_BASE_URL,
            model: crate::config::KIMI_CODE_K3_MODEL,
            requested_reasoning: crate::tui::app::ReasoningEffort::Off,
            requested_reasoning_label: "off",
            expect_control_keys: &["thinking"],
            expect_wire_effort: Some("low"),
            expect_wire_effort_source: Some("thinking.effort"),
            expect_output_cap_key: "max_tokens",
            expect_absent_output_cap_key: "max_completion_tokens",
        }
    }

    fn minimax_m3() -> MatrixRoute {
        MatrixRoute {
            name: "MiniMax-M3 @ api.minimax.io",
            provider: ApiProvider::Minimax,
            provider_key: "minimax",
            base_url: crate::config::DEFAULT_MINIMAX_BASE_URL,
            model: crate::config::DEFAULT_MINIMAX_MODEL,
            requested_reasoning: crate::tui::app::ReasoningEffort::High,
            requested_reasoning_label: "high",
            expect_control_keys: &["thinking", "reasoning_split"],
            expect_wire_effort: None,
            expect_wire_effort_source: None,
            expect_output_cap_key: "max_completion_tokens",
            expect_absent_output_cap_key: "max_tokens",
        }
    }

    fn matrix_routes() -> Vec<MatrixRoute> {
        vec![
            glm_5_2_zai_coding(),
            glm_5_turbo_zai(),
            kimi_k3_moonshot_direct(),
            k3_kimi_code(),
            minimax_m3(),
        ]
    }

    fn matrix_config(route: &MatrixRoute) -> crate::config::Config {
        let entry = crate::config::ProviderConfig {
            api_key: Some(format!("sk-test-{}-matrix-key", route.provider_key)),
            base_url: Some(route.base_url.to_string()),
            model: Some(route.model.to_string()),
            ..crate::config::ProviderConfig::default()
        };
        let mut providers = crate::config::ProvidersConfig::default();
        match route.provider {
            ApiProvider::Zai => providers.zai = entry,
            ApiProvider::Moonshot => providers.moonshot = entry,
            ApiProvider::Minimax => providers.minimax = entry,
            other => panic!("{}: unhandled matrix provider {other:?}", route.name),
        }
        crate::config::Config {
            provider: Some(route.provider_key.to_string()),
            providers: Some(providers),
            ..crate::config::Config::default()
        }
    }

    fn matrix_identity(route: &MatrixRoute) -> crate::config::ProviderIdentity {
        crate::config::ProviderIdentity {
            provider: route.provider,
            key: route.provider_key.to_string(),
            exact_id: None,
        }
    }

    /// Plan the exact route through production, then redirect only the
    /// *transport* at the local capture server. The endpoint identity the
    /// route shaper reads is untouched, so the captured body is the body
    /// `api.z.ai` / `api.moonshot.ai` / `api.kimi.com` / `api.minimax.io`
    /// would have received.
    async fn matrix_planned_route(
        route: &MatrixRoute,
        config: &crate::config::Config,
        transport_base_url: Option<&str>,
        prompt: &str,
    ) -> crate::turn_route_plan::PlannedTurnRoute {
        let identity = matrix_identity(route);
        let mut planned = plan_with_reasoning(
            config,
            &identity,
            route.provider,
            route.model,
            false,
            route.requested_reasoning,
            prompt,
        )
        .await;
        if let Some(transport_base_url) = transport_base_url {
            let validated = planned
                .route
                .clone()
                .validate()
                .expect("the matrix route validates into a concrete client");
            let mut client = validated.client.clone();
            client.set_test_chat_transport_base_url(transport_base_url.to_string());
            planned.route = crate::route_runtime::ValidatedRuntimeRoute {
                client,
                ..validated
            }
            .into_resolved();
        }
        planned
    }

    /// Non-system messages on a Chat Completions body — the manifest counts
    /// the system region separately, so `message_count` must exclude it.
    fn non_system_message_count(body: &serde_json::Value) -> usize {
        body.get("messages")
            .and_then(serde_json::Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .filter(|message| {
                        message.get("role").and_then(serde_json::Value::as_str) != Some("system")
                    })
                    .count()
            })
            .unwrap_or_default()
    }

    async fn assert_matrix_route(route: &MatrixRoute) {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let name = route.name;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .mount(&server)
            .await;

        let config = matrix_config(route);
        let (mut engine, _tmp) = wire_preview_engine(&config);
        let prompt = "inspect the exact next request for this route";
        let uri = server.uri();
        let planned = matrix_planned_route(route, &config, Some(uri.as_str()), prompt).await;
        let planned_provider = planned.route.identity.provider;
        let planned_base_url = planned.route.candidate.endpoint().base_url.clone();
        let planned_model = planned.route.model.clone();
        let expected_wire_model = crate::config::wire_model_for_provider_route(
            planned_provider,
            &planned_base_url,
            &planned_model,
        );
        assert_eq!(
            planned_base_url.trim_end_matches('/'),
            route.base_url.trim_end_matches('/'),
            "{name}: the planner must keep the exact configured endpoint"
        );

        let (manifest, body) = assert_preview_matches_first_wire_body(
            &mut engine,
            &server,
            planned,
            prompt,
            PreviewWireFixture {
                requested_model: Some(route.model.to_string()),
                requested_reasoning: Some(route.requested_reasoning_label.to_string()),
                ..Default::default()
            },
        )
        .await;

        // --- Route facts -------------------------------------------------
        let facts = manifest
            .route
            .exact()
            .expect("a configured fixed route is exact");
        assert_eq!(facts.provider_id.as_str(), route.provider_key, "{name}");
        assert_eq!(facts.wire_model.as_str(), expected_wire_model, "{name}");
        assert_eq!(facts.dialect, "chat-completions", "{name}");
        assert_eq!(facts.routing_source, "active-fixed-route", "{name}");
        assert_eq!(
            body.get("model").and_then(serde_json::Value::as_str),
            Some(expected_wire_model.as_str()),
            "{name}: the manifest's wire model must be the model on the wire: {body}"
        );

        // --- Body identity, byte accounting, and size estimates ----------
        let facts_body = manifest.body.exact().expect("a prompted body is exact");
        let canonical = crate::client::canonical_json(&body);
        assert_eq!(
            facts_body.body_sha256,
            crate::hashing::sha256_hex(canonical.as_bytes()),
            "{name}: manifest body hash must equal the captured wire body hash"
        );
        assert_eq!(
            facts_body.body_canonical_json_bytes,
            canonical.len(),
            "{name}: canonical body size must describe the captured body"
        );
        assert_eq!(
            facts_body.system_canonical_json_bytes
                + facts_body.tool_schema_canonical_json_bytes
                + facts_body.message_canonical_json_bytes
                + facts_body.framing_canonical_json_bytes,
            facts_body.body_canonical_json_bytes,
            "{name}: the four accounting classes must sum to the body"
        );
        assert!(
            facts_body.system_canonical_json_bytes > 0,
            "{name}: this route sends a system region"
        );
        assert!(
            facts_body.tool_schema_canonical_json_bytes > 0,
            "{name}: this route sends tool schemas"
        );
        assert!(
            facts_body.message_canonical_json_bytes > 0,
            "{name}: this route sends messages"
        );
        assert_eq!(
            facts_body.message_count,
            non_system_message_count(&body),
            "{name}: message_count counts the non-system messages on the wire"
        );
        assert!(
            facts_body.tool_result_canonical_json_bytes <= facts_body.message_canonical_json_bytes,
            "{name}: tool results are a subset of messages"
        );
        assert!(
            facts_body.attachment_canonical_json_bytes <= facts_body.message_canonical_json_bytes,
            "{name}: attachments are a subset of messages"
        );
        assert_eq!(
            facts_body.tool_schema_wire_sha256,
            body.get("tools").map(|tools| {
                crate::hashing::sha256_hex(crate::client::canonical_json(tools).as_bytes())
            }),
            "{name}: the tool-schema digest must be over the schemas on the wire"
        );
        assert!(
            facts_body.estimates.system > 0 && facts_body.estimates.tool_schemas > 0,
            "{name}: per-class estimates are derived from the same wire regions"
        );
        assert!(
            facts_body.estimates.total_conservative > 0,
            "{name}: a whole-body estimate is available"
        );

        // --- Output cap: exactly the key this route writes ----------------
        let wire_cap = body
            .get(route.expect_output_cap_key)
            .and_then(serde_json::Value::as_u64);
        assert!(
            wire_cap.is_some(),
            "{name}: expected `{}` on the wire: {body}",
            route.expect_output_cap_key
        );
        assert!(
            body.get(route.expect_absent_output_cap_key).is_none(),
            "{name}: `{}` must not be on the wire: {body}",
            route.expect_absent_output_cap_key
        );
        assert_eq!(
            facts_body.wire_output_cap_tokens, wire_cap,
            "{name}: the reported output cap is the one literally on the wire"
        );

        // --- requested → effective reasoning ------------------------------
        assert_eq!(
            manifest.session.requested_model.as_str(),
            route.model,
            "{name}"
        );
        assert_eq!(
            manifest.session.requested_reasoning.as_str(),
            route.requested_reasoning_label,
            "{name}"
        );
        assert_eq!(
            facts_body.reasoning_resolution,
            ReasoningResolution::Explicit,
            "{name}: a fixed route with an explicitly requested tier"
        );
        assert_eq!(
            facts_body.reasoning_wire_control_keys, route.expect_control_keys,
            "{name}: reasoning-control keys, against the captured body {body}"
        );
        assert_eq!(
            facts_body
                .reasoning_wire_effort
                .as_ref()
                .map(|effort| effort.as_str()),
            route.expect_wire_effort,
            "{name}: wire effort, against the captured body {body}"
        );
        assert_eq!(
            facts_body.reasoning_wire_effort_source.as_deref(),
            route.expect_wire_effort_source,
            "{name}"
        );
        // Every reported control key is genuinely present on the wire, and the
        // reported effort is genuinely readable at the reported key path.
        for key in &facts_body.reasoning_wire_control_keys {
            assert!(
                body.get(key).is_some(),
                "{name}: reported control key `{key}` is not on the wire: {body}"
            );
        }
        match (
            facts_body.reasoning_wire_effort_source.as_deref(),
            route.expect_wire_effort,
        ) {
            (Some("reasoning_effort"), Some(effort)) => assert_eq!(
                body.get("reasoning_effort")
                    .and_then(serde_json::Value::as_str),
                Some(effort),
                "{name}: {body}"
            ),
            (Some(path), Some(effort)) => {
                let pointer = format!("/{}", path.replace('.', "/"));
                assert_eq!(
                    body.pointer(&pointer).and_then(serde_json::Value::as_str),
                    Some(effort),
                    "{name}: {body}"
                );
            }
            (None, None) => assert!(
                body.get("reasoning_effort").is_none(),
                "{name}: no effort was reported, so none may be on the wire: {body}"
            ),
            (source, effort) => panic!("{name}: inconsistent effort receipt {source:?}/{effort:?}"),
        }

        // --- Provider-authoritative usage ---------------------------------
        // A preview describes a request that has not been sent. Unknown stays
        // unknown; it never becomes a zero.
        assert!(
            matches!(
                &facts_body.provider_reported_usage,
                Availability::Unavailable(unavailable)
                    if unavailable.reason == UnavailableReason::ProviderRequestNotExecuted
            ),
            "{name}: preview must not claim provider usage"
        );
        let json = manifest.to_json();
        assert!(
            !json.contains("\"input_tokens\""),
            "{name}: no fabricated usage counters reach the surface:\n{json}"
        );
    }

    #[tokio::test]
    async fn matrix_glm_5_2_zai_coding_preview_matches_the_first_wire_body() {
        assert_matrix_route(&glm_5_2_zai_coding()).await;
    }

    #[tokio::test]
    async fn matrix_glm_5_turbo_zai_preview_matches_the_first_wire_body() {
        assert_matrix_route(&glm_5_turbo_zai()).await;
    }

    #[tokio::test]
    async fn matrix_kimi_k3_moonshot_direct_preview_matches_the_first_wire_body() {
        assert_matrix_route(&kimi_k3_moonshot_direct()).await;
    }

    #[tokio::test]
    async fn matrix_k3_kimi_code_preview_matches_the_first_wire_body() {
        assert_matrix_route(&k3_kimi_code()).await;
    }

    #[tokio::test]
    async fn matrix_minimax_m3_preview_matches_the_first_wire_body() {
        assert_matrix_route(&minimax_m3()).await;
    }

    /// The active tool-catalog hash is a *catalog identity*, not a wire fact:
    /// the same catalog under the same posture must hash the same on every
    /// route, however differently each dialect then shapes those schemas.
    /// (Unit-level membership/order/schema sensitivity is pinned by
    /// `active_catalog_hash_tracks_membership_order_and_schema`.)
    #[tokio::test]
    async fn matrix_routes_share_one_active_tool_catalog_hash() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let prompt = "describe the shared tool catalog";
        let mut observed: Vec<(&'static str, usize, String, String)> = Vec::new();

        for route in matrix_routes() {
            let config = matrix_config(&route);
            let (mut engine, _handle) = Engine::new(
                EngineConfig {
                    workspace: workspace.path().to_path_buf(),
                    max_steps: 1,
                    snapshots_enabled: false,
                    terminal_chrome_enabled: false,
                    ..Default::default()
                },
                &config,
            );
            engine.config.features.disable(Feature::Mcp);
            engine.config.subagents_enabled = false;

            let planned = matrix_planned_route(&route, &config, None, prompt).await;
            let manifest = engine
                .build_request_manifest(inputs(false, Some(planned), prompt))
                .await;
            let tools = manifest
                .tools
                .exact()
                .expect("MCP is off, so the tool surface is exact");
            assert!(
                tools.standard_and_full_surfaces_collapsed,
                "{}: this fixture's catalog fits both budgets, so the surface \
                 budget label cannot change catalog membership",
                route.name
            );
            observed.push((
                route.name,
                tools.active_tool_count,
                tools.active_tool_catalog_sha256.clone(),
                tools.tool_surface_budget.clone(),
            ));
        }

        assert_eq!(observed.len(), 5, "every matrix route is represented");
        let (first_name, first_count, first_hash, _) = observed[0].clone();
        for (name, count, hash, _) in &observed {
            assert_eq!(
                *count, first_count,
                "{name} vs {first_name}: the matrix fixture holds the tool surface constant"
            );
            assert_eq!(
                hash, &first_hash,
                "{name} vs {first_name}: one catalog must hash to one identity across routes"
            );
        }

        // …and the routes really are distinct in capability posture: the shared
        // hash is a genuine cross-route agreement, not five copies of one
        // route. GLM-5.2 publishes a `Full` tool surface budget while
        // GLM-5-Turbo publishes `Standard`, and the catalog identity is
        // unchanged by that difference.
        let budgets: std::collections::BTreeSet<&str> = observed
            .iter()
            .map(|(_, _, _, budget)| budget.as_str())
            .collect();
        assert!(
            budgets.len() > 1,
            "the matrix spans routes with different surface budgets: {budgets:?}"
        );
    }

    /// Provider-authoritative usage is never a preview fact, and it is never a
    /// zero standing in for "not measured". It becomes knowable only when a
    /// response reports it, through the same `parse_usage` seam the turn loop
    /// uses.
    #[tokio::test]
    async fn provider_reported_usage_is_unavailable_until_a_response_reports_it() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let usage = json!({"prompt_tokens": 137, "completion_tokens": 24, "total_tokens": 161});
        let stream = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "choices": [{"index": 0, "delta": {"content": "ok"}}],
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": usage,
            })
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stream),
            )
            .mount(&server)
            .await;

        let mut config = deepseek_config();
        config
            .providers
            .as_mut()
            .expect("providers")
            .deepseek
            .base_url = Some(server.uri());
        let identity = deepseek_identity();
        let (mut engine, _tmp) = wire_preview_engine(&config);

        let prompt = "count the tokens this turn will report";
        let planned = plan(&config, &identity, false, prompt).await;
        let production_route = planned.route.clone();
        let compaction = planned.compaction.clone();
        let reasoning_effort = planned.effective_reasoning_effort.clone();
        let reasoning_effort_auto = planned.auto_controls_reasoning;

        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), prompt))
            .await;
        let body = manifest.body.exact().expect("a prompted body is exact");
        assert!(
            matches!(
                &body.provider_reported_usage,
                Availability::Unavailable(unavailable)
                    if unavailable.reason == UnavailableReason::ProviderRequestNotExecuted
            ),
            "no request occurred, so there is nothing the provider reported"
        );
        assert_eq!(
            engine.session.total_usage.input_tokens, 0,
            "and nothing has been recorded yet"
        );
        assert_eq!(engine.session.total_usage.output_tokens, 0);

        let _ = engine
            .handle_send_message(
                prompt.to_string(),
                AppMode::Agent,
                production_route,
                compaction,
                None,
                None,
                GoalStatus::Active,
                reasoning_effort,
                reasoning_effort_auto,
                false,
                false,
                false,
                false,
                crate::tui::approval::ApprovalMode::Suggest,
                false,
                None,
                Vec::new(),
                None,
                None,
                UserInputProvenance::ExternalUser,
            )
            .await;

        // The completed turn's counts are exactly what `parse_usage` reads off
        // the reported usage object — no rounding, no substituted estimate.
        let parsed = crate::client::parse_usage(Some(&usage));
        assert_eq!(u64::from(parsed.input_tokens), 137);
        assert_eq!(u64::from(parsed.output_tokens), 24);
        assert_eq!(
            (
                engine.session.total_usage.input_tokens,
                engine.session.total_usage.output_tokens
            ),
            (
                u64::from(parsed.input_tokens),
                u64::from(parsed.output_tokens)
            ),
            "the turn records the provider-authoritative counts, not an estimate"
        );
        let reported = crate::request_manifest::ProviderReportedUsage {
            input_tokens: engine.session.total_usage.input_tokens,
            output_tokens: engine.session.total_usage.output_tokens,
        };
        assert_eq!(reported.input_tokens, 137);
        assert_eq!(reported.output_tokens, 24);
    }

    /// A fixed route with a hypothetical prompt describes the next turn
    /// exactly: route, tools, and body are all published, and the prompt is
    /// part of the hashed body.
    #[tokio::test]
    async fn fixed_route_with_a_prompt_describes_the_exact_next_turn() {
        let mut config = deepseek_config();
        config
            .providers
            .as_mut()
            .expect("providers")
            .deepseek
            .context_window = Some(123_456);
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        engine.active_route_limits = Some(codewhale_config::route::RouteLimits {
            context_tokens: Some(4_096),
            input_tokens: Some(3_000),
            output_tokens: Some(512),
        });

        let planned = plan(&config, &identity, false, "refactor the parser").await;
        let planned_limits =
            crate::route_budget::known_route_limits(planned.route.candidate.limits());
        let expected_input_budget = context_input_budget_for_route(
            planned.route.identity.provider,
            &planned.route.model,
            planned_limits,
            0,
        );
        let expected_wire_output = crate::route_budget::effective_max_output_tokens_for_route(
            planned.route.identity.provider,
            &planned.route.model,
            planned_limits,
        );
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), "refactor the parser"))
            .await;

        let route = manifest.route.exact().expect("a fixed route is exact");
        assert_eq!(route.provider_id.as_str(), "deepseek");
        assert_eq!(route.routing_source, "active-fixed-route");
        assert_eq!(route.dialect, "chat-completions");
        assert_eq!(route.caller_entrypoint, "streaming");
        assert_eq!(route.body_stream_field, Some(true));
        assert_eq!(route.context_limit_tokens, 123_456);
        assert_eq!(
            route.context_limit_source,
            crate::route_runtime::ContextWindowSource::Configured
        );
        assert_eq!(
            route.route_input_limit_tokens,
            planned_limits.and_then(|limits| limits.input_tokens)
        );
        assert_eq!(
            route.route_output_limit_tokens,
            planned_limits.and_then(|limits| limits.output_tokens)
        );
        assert!(!route.wire_model.is_redacted());
        assert!(
            manifest.tools.exact().is_some(),
            "MCP is off in this engine"
        );

        let body = manifest.body.exact().expect("a prompted body is exact");
        assert_eq!(body.input_budget_ceiling_tokens, expected_input_budget);
        assert_eq!(
            body.wire_output_cap_tokens,
            Some(u64::from(expected_wire_output))
        );
        assert_eq!(body.body_sha256.len(), 64);
        assert!(
            body.message_count >= 1,
            "the hypothetical prompt is a message"
        );
        assert!(body.local_system_tools_component_sha256.is_some());
        assert!(manifest.session.hypothetical_prompt_supplied);

        // The prompt is genuinely part of the request being described.
        let other_planned = plan(&config, &identity, false, "write the release notes").await;
        let other = engine
            .build_request_manifest(inputs(
                false,
                Some(other_planned),
                "write the release notes",
            ))
            .await;
        assert_ne!(
            body.body_sha256,
            other.body.exact().expect("exact").body_sha256,
            "a different next prompt must produce a different body hash"
        );
    }

    /// The engine can describe an Auto route receipt supplied by a trusted
    /// host without consulting installed state. The human preview command
    /// deliberately never obtains such a receipt, because doing so would call
    /// the provider-backed classifier.
    #[tokio::test]
    async fn host_supplied_auto_route_receipt_matches_the_production_planner() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);

        let planned = plan(&config, &identity, true, "explain this stack trace").await;
        let planned_provider = planned.effective_provider;
        let planned_identity = planned.effective_provider_identity.clone();
        let planned_model = planned.route.model.clone();
        let planned_base_url = planned.route.candidate.endpoint().base_url.clone();
        assert!(
            planned.auto_controls_reasoning,
            "the helper requests auto reasoning for its auto-model fixture"
        );

        let manifest = engine
            .build_request_manifest(inputs(true, Some(planned), "explain this stack trace"))
            .await;

        let route = manifest
            .route
            .exact()
            .expect("auto + prompt resolves a route");
        assert_eq!(route.provider_id.as_str(), planned_provider.as_str());
        assert_eq!(route.routing_source, "auto-provider-classifier");
        assert_eq!(planned_identity, "deepseek");
        assert_eq!(
            route.wire_model.as_str(),
            crate::config::wire_model_for_provider_route(
                planned_provider,
                &planned_base_url,
                &planned_model,
            ),
            "the wire model is the planner's model after route remapping — not \
             the model the session happens to have installed"
        );
        assert_eq!(
            manifest.session.requested_model.as_str(),
            "auto",
            "the manifest never reports the resolved model as the user's selection"
        );

        let body = match &manifest.body {
            Availability::Exact(body) => body,
            Availability::Unavailable(unavailable) => {
                panic!("auto + prompt should have an exact body: {unavailable:?}")
            }
        };
        assert_ne!(
            body.reasoning_resolution,
            ReasoningResolution::Explicit,
            "an auto-routed turn never claims an explicit user tier"
        );

        // The hypothetical prompt is part of the hashed body on the auto path
        // too, not only on the fixed one.
        let other = plan(&config, &identity, true, "rename one local variable").await;
        let other = engine
            .build_request_manifest(inputs(true, Some(other), "rename one local variable"))
            .await;
        assert_ne!(
            body.body_sha256,
            other.body.exact().expect("exact").body_sha256
        );
    }

    /// The passive path must not create an MCP pool, connect a server, or
    /// emit a UI event — it reports the tool surface unavailable instead.
    #[tokio::test]
    async fn preview_tool_snapshot_has_no_mcp_or_event_side_effects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = crate::config::Config {
            provider: Some("deepseek".to_string()),
            ..crate::config::Config::default()
        };
        let (mut engine, handle) = Engine::new(
            EngineConfig {
                workspace: tmp.path().to_path_buf(),
                ..Default::default()
            },
            &config,
        );
        let _ = engine.config.features.enable(Feature::Mcp);

        let policy = TurnAuthority::from_effective_fields(
            AppMode::Agent,
            false,
            false,
            false,
            crate::tui::approval::ApprovalMode::Suggest,
        );
        let build = engine
            .build_turn_tool_registry_and_catalog(
                &policy,
                &[],
                None,
                SubAgentWiring::Inert,
                McpAccess::PassiveSnapshot,
                TurnRouteContext {
                    provider: engine.api_provider,
                    model: engine.session.model.clone(),
                    capabilities: engine.active_route_capabilities,
                    limits: engine.active_route_limits,
                    client: engine.deepseek_client.clone(),
                    api_config: Box::new(engine.api_config.clone()),
                    locale_tag: engine.config.locale_tag.clone(),
                    role_models: engine.subagent_role_models(),
                    fleet_roster: engine.config.fleet_roster.clone(),
                    auto_model: false,
                    reasoning_effort: None,
                    reasoning_effort_auto: false,
                },
                "",
            )
            .await;

        assert!(
            engine.mcp_pool.is_none(),
            "a passive snapshot must not create the MCP pool"
        );
        assert!(
            matches!(build.mcp, McpToolState::Unavailable { .. }),
            "with no connected pool the MCP tool state is unavailable, not empty"
        );
        assert!(build.mcp.server_count().is_none());
        drop(handle);
    }

    /// The reviewed blocker: with MCP enabled but nothing connected, the
    /// preview built a catalog with zero MCP tools, prepared a body from it,
    /// and published that body as `Exact` — a hash of a request no turn would
    /// ever send. The body must inherit the tool surface's typed reason.
    #[tokio::test]
    async fn unavailable_mcp_state_makes_the_body_unavailable_too() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        // MCP on, pool never started: a real turn would connect and could
        // discover tools this catalog does not contain.
        let _ = engine.config.features.enable(Feature::Mcp);

        let planned = plan(&config, &identity, false, "refactor the parser").await;
        let manifest = engine
            .build_request_manifest(inputs(false, Some(planned), "refactor the parser"))
            .await;

        assert!(
            manifest.tools.exact().is_none(),
            "an unconnected MCP pool is not a snapshottable tool surface"
        );
        assert!(
            manifest.body.exact().is_none(),
            "a body built from a tool surface missing its MCP contribution \
             must not be published as exact"
        );
        assert!(
            manifest.route.exact().is_some(),
            "the route does not depend on the MCP contribution and stays exact"
        );

        // No body fact — hash, byte count, or local component fingerprint —
        // reaches either surface.
        let json = manifest.to_json();
        for forbidden in [
            "body_sha256",
            "local_system_tools_component_sha256",
            "tool_schema_wire_sha256",
            "body_canonical_json_bytes",
            "estimated_input_headroom_tokens",
        ] {
            assert!(!json.contains(forbidden), "{forbidden} leaked:\n{json}");
        }
        assert!(json.contains("mcp-state-not-snapshottable"), "{json}");
        assert!(engine.mcp_pool.is_none(), "no pool was created by looking");
    }

    /// A preview is an inspection. Every piece of engine state a turn would
    /// have written must be byte-identical afterwards — including the ones the
    /// earlier implementation wrote and restored around an `.await`.
    #[tokio::test]
    async fn building_a_manifest_writes_no_engine_state() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        engine.config.allowed_tools = Some(vec!["Bash".to_string()]);
        engine.session.add_message(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "an earlier turn".to_string(),
                cache_control: None,
            }],
        });

        let allowed_before = engine.config.allowed_tools.clone();
        let disallowed_before = engine.config.disallowed_tools.clone();
        let messages_before = engine.messages_with_turn_metadata();
        let model_before = engine.session.model.clone();
        let system_prompt_before = system_prompt_hash(engine.session.system_prompt.as_ref());
        let system_hash_before = engine.session.last_system_prompt_hash;
        let working_set_before = engine
            .session
            .working_set
            .summary_block(&engine.config.workspace);
        let provider_before = engine.api_provider;
        let mode_before = engine.current_mode;
        let narrowing_before = format!("{:?}", engine.last_policy_narrowing);
        let turn_counter_before = engine.turn_counter;

        // A *different* command-scoped gate than the installed one, and a
        // prompt that mentions a path so the working set would move if the
        // preview observed it on the session rather than on a clone.
        let mut preview_inputs = inputs(
            false,
            Some(plan(&config, &identity, false, "inspect src/lib.rs").await),
            "inspect src/lib.rs",
        );
        preview_inputs.allowed_tools = Some(vec!["Read".to_string()]);
        let manifest = engine.build_request_manifest(preview_inputs).await;
        assert!(manifest.body.exact().is_some(), "fixture should be exact");

        assert_eq!(engine.config.allowed_tools, allowed_before, "tool gate");
        assert_eq!(engine.config.disallowed_tools, disallowed_before);
        assert_eq!(
            engine.messages_with_turn_metadata(),
            messages_before,
            "history"
        );
        assert_eq!(engine.session.model, model_before);
        assert_eq!(
            system_prompt_hash(engine.session.system_prompt.as_ref()),
            system_prompt_before
        );
        assert_eq!(engine.session.last_system_prompt_hash, system_hash_before);
        assert_eq!(
            engine
                .session
                .working_set
                .summary_block(&engine.config.workspace),
            working_set_before,
            "the hypothetical message is observed on a clone, never on the session"
        );
        assert_eq!(engine.api_provider, provider_before);
        assert_eq!(engine.current_mode, mode_before);
        assert_eq!(
            format!("{:?}", engine.last_policy_narrowing),
            narrowing_before
        );
        assert_eq!(engine.turn_counter, turn_counter_before);
        assert!(engine.mcp_pool.is_none());
    }

    /// The gate is a parameter, so it shapes the previewed catalog without
    /// ever being installed.
    #[tokio::test]
    async fn the_previewed_tool_gate_applies_without_being_installed() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);

        let wide = engine
            .build_request_manifest(inputs(
                false,
                Some(plan(&config, &identity, false, "do the thing").await),
                "do the thing",
            ))
            .await;

        let mut narrow_inputs = inputs(
            false,
            Some(plan(&config, &identity, false, "do the thing").await),
            "do the thing",
        );
        narrow_inputs.allowed_tools = Some(vec!["Read".to_string()]);
        let narrow = engine.build_request_manifest(narrow_inputs).await;

        let wide_tools = wide.tools.exact().expect("exact");
        let narrow_tools = narrow.tools.exact().expect("exact");
        assert!(
            narrow_tools.active_tool_count < wide_tools.active_tool_count,
            "the passed gate must narrow the previewed catalog: {} vs {}",
            narrow_tools.active_tool_count,
            wide_tools.active_tool_count
        );
        assert_eq!(
            narrow.session.allowed_tool_gate_count,
            Some(1),
            "and the session section reports the gate that was previewed"
        );
        assert_eq!(
            engine.config.allowed_tools, None,
            "…while the engine keeps its own"
        );
    }

    /// A plan failure still happened *because of* a supplied prompt. Reporting
    /// otherwise tells the user to pass the flag they just passed.
    #[tokio::test]
    async fn a_failed_plan_still_reports_that_a_prompt_was_supplied() {
        let (mut engine, _handle, _tmp) = preview_engine(&crate::config::Config::default());
        let mut failed = inputs(false, None, "");
        failed.unresolved = PreviewUnresolved::PlanFailed(
            "no API key configured for route 'my-gateway' at /home/someone/.config".to_string(),
        );

        let manifest = engine.build_request_manifest(failed).await;
        assert!(manifest.session.hypothetical_prompt_supplied);
        assert!(manifest.route.exact().is_none());

        let rendered = manifest.render();
        assert!(
            !rendered.contains("Pass `--prompt <text>`"),
            "the user already did:\n{rendered}"
        );
        // …and the raw host text never reaches a surface verbatim.
        for surface in [rendered, manifest.to_json()] {
            assert!(!surface.contains("my-gateway'"), "{surface}");
            assert!(!surface.contains("/home/someone"), "{surface}");
        }
    }

    /// Pending runtime injections are *counted*, never consumed, and they make
    /// the body unavailable rather than silently absent from it.
    #[tokio::test]
    async fn pending_runtime_injections_make_the_body_unavailable_without_consuming_them() {
        let config = deepseek_config();
        let identity = deepseek_identity();
        let (mut engine, _handle, _tmp) = preview_engine(&config);
        engine.config.features.disable(Feature::Mcp);
        engine.pending_lsp_blocks.push(crate::lsp::DiagnosticBlock {
            file: std::path::PathBuf::from("src/lib.rs"),
            items: Vec::new(),
        });

        let manifest = engine
            .build_request_manifest(inputs(
                false,
                Some(plan(&config, &identity, false, "fix it").await),
                "fix it",
            ))
            .await;

        assert!(
            manifest.body.exact().is_none(),
            "the turn loop would inject diagnostics before the first request"
        );
        assert!(manifest.route.exact().is_some());
        assert_eq!(
            engine.pending_lsp_blocks.len(),
            1,
            "inspecting must not flush the pending blocks"
        );
        assert!(
            manifest
                .to_json()
                .contains("runtime-transforms-before-send"),
            "{}",
            manifest.to_json()
        );
    }
}
