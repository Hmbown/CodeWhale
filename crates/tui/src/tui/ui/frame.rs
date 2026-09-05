//! Frame composition: the draw entry point, the builders that assemble what a
//! frame needs, and streaming-text accumulation into history cells.
//!
//! Moved verbatim out of `ui.rs`.

use super::*;
use crate::models::Role;
use crate::tui::infoline::{InfoLine, InfoSegment, InfoSegmentId, infoline_hitboxes};

/// Context window percentage for the metrics line's reading — the same
/// snapshot the posture bar's ≥80% microcopy reads, so the two can never
/// disagree.
pub(crate) fn info_context_percent(app: &App) -> u8 {
    crate::tui::phase_strip::context_percent_from_app(app)
}

/// The session cost as the one price string every surface prints
/// (SHELL-DESIGN-20260901 §2.11 item 5): the metrics line, the roster's
/// right column, the price widget and the turn summary all read this. Empty
/// until the session has a priced or counted turn.
pub(crate) fn session_cost_label(app: &App) -> String {
    let usage_chip = app.cumulative_usage_chip();
    match &usage_chip {
        crate::route_billing::UsageChip::Money(amount) => Some(amount.clone()),
        crate::route_billing::UsageChip::PricedSubtotal { .. }
        | crate::route_billing::UsageChip::Unknown => {
            crate::route_billing::format_usage_chip(&usage_chip)
        }
        _ => None,
    }
    .unwrap_or_default()
}

/// Output tokens and output rate for the metrics line: the live stream's
/// running estimate while a turn is producing text, else the last turn's
/// provider-reported figures. `None` before any turn has produced output.
fn output_figures(app: &App) -> Option<(u64, Option<f64>)> {
    if app.is_loading && app.streaming_output_token_estimate > 0 {
        let rate = app
            .turn_started_at
            .map(|started| started.elapsed().as_secs_f64())
            .filter(|secs| *secs > 0.0)
            .map(|secs| app.streaming_output_token_estimate as f64 / secs);
        return Some((app.streaming_output_token_estimate, rate));
    }
    if let Some(throughput) = app.session.last_output_throughput {
        return Some((
            throughput.output_tokens,
            Some(throughput.tokens_per_second()),
        ));
    }
    app.session
        .last_completion_tokens
        .filter(|tokens| *tokens > 0)
        .map(|tokens| (u64::from(tokens), None))
}

/// Build the metrics line's segments from live `App` state. Shedding is the
/// widget's job; this only states the facts, in display order: model,
/// context, cost, time to first token, output rate, output tokens.
///
/// Repository and branch left this row (2026-09-02): the launch header and
/// the git bottom view own them. Fleet, whale and automation counts left too —
/// the posture bar's live counts own activity.
pub(crate) fn info_segments(app: &App, width: u16) -> Vec<InfoSegment> {
    use crate::localization::MessageId;
    use crate::palette::ChromeInk;
    let mut segments = Vec::new();
    let tier = crate::tui::underwater::ShellTier::for_chrome_width(width);

    // Route identity — the old identity band's fact, same shed discipline:
    // provider first, then effort, whole names or none. When no model is
    // configured the segment says so and waits.
    let (_, model) = app.effective_route_identity_display();
    if model.is_empty() {
        segments.push(InfoSegment::new(
            InfoSegmentId::Model,
            app.tr(MessageId::StartupDefaultSubjectModel).as_ref(),
            app.tr(MessageId::InfoLineNotConnected).as_ref(),
            ChromeInk::Waiting,
        ));
    } else {
        // The context reading and the metrics claim the rest of the row;
        // the route sheds its own qualifiers first.
        let budget = (usize::from(width)).saturating_sub(60).max(24);
        let fields = crate::tui::phase_strip::route_identity_fields(app, tier, budget)
            .unwrap_or_else(|| vec![model]);
        segments.push(InfoSegment::new(
            InfoSegmentId::Model,
            "",
            fields.join(" · "),
            ChromeInk::Identity,
        ));
    }

    // The context reading: painted here and nowhere else. Only displayed
    // when context fullness >= 50%; below 50% it remains silent. At the 80%
    // cap the whole reading turns to the error token — it is the one fact on
    // this row that becomes a problem rather than a status.
    let pct = info_context_percent(app);
    if pct >= 50 {
        segments.push(InfoSegment::new(
            InfoSegmentId::Context,
            app.tr(MessageId::InfoLineContext).as_ref(),
            format!("{pct}%"),
            if pct >= 80 {
                ChromeInk::Failure
            } else {
                ChromeInk::Info
            },
        ));
    }

    let cost = session_cost_label(app);
    if !cost.is_empty() {
        segments.push(InfoSegment::new(
            InfoSegmentId::Cost,
            "",
            cost,
            ChromeInk::MetadataValue,
        ));
    }

    // The DeepSeek-harness session metrics, from the same accumulators
    // `/cost` prints: nothing here is estimated except the live stream's
    // running token count, which the provider's receipt replaces.
    if let Some(ttft) = app.session_metrics.ttft_average() {
        segments.push(InfoSegment::new(
            InfoSegmentId::Ttft,
            app.tr(MessageId::InfoLineTtft).as_ref(),
            crate::tui::session_metrics::format_duration(ttft),
            ChromeInk::MetadataValue,
        ));
    }
    if let Some((tokens, rate)) = output_figures(app) {
        if let Some(rate) = rate {
            segments.push(InfoSegment::new(
                InfoSegmentId::Rate,
                "",
                format!(
                    "{} {}",
                    crate::tui::session_metrics::format_rate(rate),
                    app.tr(MessageId::SessionMetricsTokensPerSecond)
                ),
                ChromeInk::MetadataValue,
            ));
        }
        let hit = u64::from(app.session.displayed_total_cache_hit_tokens());
        let miss = u64::from(app.session.displayed_total_cache_miss_tokens());
        let cache_total = hit + miss;
        if cache_total > 0 {
            let cache_pct = (hit * 100 + cache_total / 2)
                .checked_div(cache_total)
                .and_then(|pct| u8::try_from(pct).ok())
                .unwrap_or(100);
            segments.push(InfoSegment::new(
                InfoSegmentId::Cache,
                "cache",
                format!("{cache_pct}%"),
                ChromeInk::MetadataValue,
            ));
        }
        segments.push(InfoSegment::new(
            InfoSegmentId::OutputTokens,
            "↓",
            crate::tui::session_metrics::format_tokens(tokens),
            ChromeInk::MetadataValue,
        ));
    } else {
        let hit = u64::from(app.session.displayed_total_cache_hit_tokens());
        let miss = u64::from(app.session.displayed_total_cache_miss_tokens());
        let cache_total = hit + miss;
        if cache_total > 0 {
            let cache_pct = (hit * 100 + cache_total / 2)
                .checked_div(cache_total)
                .and_then(|pct| u8::try_from(pct).ok())
                .unwrap_or(100);
            segments.push(InfoSegment::new(
                InfoSegmentId::Cache,
                "cache",
                format!("{cache_pct}%"),
                ChromeInk::MetadataValue,
            ));
        }
    }

    segments
}

/// The info line's controls that actually painted in this frame.
///
/// The route target intentionally contains no copied route metadata. The
/// provider picker retains catalog, readiness, credential, and apply
/// authority; chrome only exposes its entry point.
#[derive(Debug, Clone, Copy, Default)]
struct InfoLineInteractionHitboxes {
    context: Option<Rect>,
    route: Option<Rect>,
}

/// Render the info line into its one row and record its segment
/// hitboxes (spec §5b `Constraint::Length(1)`). The ONE header on every
/// screen: the session shell and the launch screen both call this, so the
/// brand lockup, contextual segments, and the pinned meter + clock never
/// change identity between pre- and post-session states. Segment rects are
/// recorded for hover (this frame's highlight resolves against the previous
/// frame's rects, the standard one-frame-lag registry pattern) and for typed
/// click routing.
fn render_info_row(f: &mut Frame, app: &mut App, area: Rect) -> InfoLineInteractionHitboxes {
    if area.height == 0 {
        app.viewport.last_infoline_hitboxes.clear();
        return InfoLineInteractionHitboxes::default();
    }
    let segments = info_segments(app, area.width);
    let hovered = app.last_mouse_pos.and_then(|(mx, my)| {
        app.viewport
            .last_infoline_hitboxes
            .iter()
            .find(|hb| {
                matches!(hb.id, InfoSegmentId::Model | InfoSegmentId::Context)
                    && hb.area.x <= mx
                    && mx < hb.area.right()
                    && hb.area.y == my
            })
            .map(|hb| hb.id)
    });
    let help_hint = crate::tui::shell_key_routing::info_help_hint(app.ui_locale);
    let info = InfoLine::new(&app.ui_theme, &help_hint, &segments)
        .ascii_safe(crate::tui::color_compat::ascii_safe_enabled())
        .hovered(hovered);
    let hitboxes = infoline_hitboxes(&info, area);
    let interaction_hitboxes = InfoLineInteractionHitboxes {
        context: crate::tui::infoline::context_meter_hitbox(&info, area),
        route: hitboxes
            .iter()
            .find(|hitbox| hitbox.id == InfoSegmentId::Model)
            .map(|hitbox| hitbox.area),
    };
    // Keep the row's quiet background under the widget itself.
    let buf = f.buffer_mut();
    Block::default()
        .style(Style::default().bg(app.ui_theme.header_bg))
        .render(area, buf);
    ratatui::widgets::Widget::render(info, area, buf);
    app.viewport.last_infoline_hitboxes = hitboxes;
    interaction_hitboxes
}

/// Register the info line's drawn controls as one typed input surface.
///
/// Both the launch stage and a live session use this exact registration, so
/// mouse routing cannot advertise a header segment on only one shell state.
fn register_info_interaction_targets(app: &mut App, hitboxes: InfoLineInteractionHitboxes) {
    if let (Some(hitbox), Some(context_budget)) = (
        hitboxes.context,
        crate::tui::tideline::ContextBudgetSnapshot::from_app(app),
    ) {
        app.viewport
            .interaction_targets
            .register(crate::tui::tideline::InteractionTarget {
                id: crate::tui::tideline::InteractionTargetId::HEADER_CONTEXT,
                area: hitbox,
                focus: crate::tui::tideline::InteractionFocus::Direct,
                keyboard_action: Some(crate::tui::tideline::InteractionAction::InspectContext),
                mouse_action: Some(crate::tui::tideline::InteractionAction::InspectContext),
                inspect_detail: crate::tui::tideline::InspectDetail::ContextBudget(context_budget),
            });
    }
    if let Some(hitbox) = hitboxes.route {
        app.viewport
            .interaction_targets
            .register(crate::tui::tideline::InteractionTarget {
                id: crate::tui::tideline::InteractionTargetId::HEADER_ROUTE,
                area: hitbox,
                focus: crate::tui::tideline::InteractionFocus::Direct,
                keyboard_action: Some(crate::tui::tideline::InteractionAction::OpenProviderPicker),
                mouse_action: Some(crate::tui::tideline::InteractionAction::OpenProviderPicker),
                inspect_detail: crate::tui::tideline::InspectDetail::Route,
            });
    }

    for target in app.viewport.interaction_targets.iter() {
        let label = match target.mouse_action {
            Some(crate::tui::tideline::InteractionAction::InspectContext) => format!(
                "{} · {}",
                crate::localization::tr(
                    app.ui_locale,
                    crate::localization::MessageId::CtxMenuContextInspector,
                ),
                crate::localization::tr(
                    app.ui_locale,
                    crate::localization::MessageId::CtxMenuContextInspectorDesc,
                ),
            ),
            Some(crate::tui::tideline::InteractionAction::OpenProviderPicker) => format!(
                "{} · {}",
                crate::localization::tr(
                    app.ui_locale,
                    crate::localization::MessageId::RoutePanelHeader,
                ),
                crate::localization::tr(
                    app.ui_locale,
                    crate::localization::MessageId::CmdProviderDescription,
                ),
            ),
            Some(crate::tui::tideline::InteractionAction::ShowDockPanel(panel)) => {
                panel.title().to_string()
            }
            Some(crate::tui::tideline::InteractionAction::DismissDock) => {
                crate::localization::tr(app.ui_locale, crate::localization::MessageId::KbCloseMenu)
                    .into_owned()
            }
            None => continue,
        };
        crate::tui::hover_layer::register_rect(
            crate::tui::hover_hit::HoverTargetKind::Link,
            target.area,
            label,
            false,
        );
    }
}

/// Map the host terminal rect onto the session shell canvas.
///
/// Wide terminals use the full available width (v0.8.65 behavior; #5322). A
/// brief v0.9 gutter capped usable columns beyond 112 and left dead margins on
/// large displays / tmux panes; that cap is gone. Keep this helper so layout
/// and PTY oracles share one geometry entry point if a future setting wants a
/// configurable measure again.
pub(crate) fn session_shell_area(area: Rect) -> Rect {
    area
}

/// Snapshot the posture a real `Op::SendMessage` would carry, and — when the
/// user supplied a hypothetical prompt — resolve the next turn's route with
/// the **same shared planner** dispatch uses (#1004).
///
/// The hypothetical prompt is taken through the deterministic part of the real
/// submit path, in the real order: the **active skill** it would be wrapped
/// with, file and git mention resolution with the same error propagation, and
/// the paused-command note a real submit appends. That is what makes the body
/// the engine hashes the body a real turn would build. It is never added to
/// the conversation, no state is consumed, and the previewed request itself is
/// never sent.
///
/// Two things a real submit does that an inspection must not, and what happens
/// instead:
///
/// - **`message_submit` hooks.** They run first, before mentions, skill
///   wrapping, route planning, and the tool policy, and they may replace the
///   text or block the turn outright. Running them would give a *preview* the
///   side effects of a submit. So when any are configured, nothing downstream
///   of the text can be claimed exact and the whole manifest reports
///   [`crate::core::engine::preview::PreviewUnresolved::MessageSubmitHooksConfigured`] —
///   including under a
///   fixed model, because the tool policy is derived from the content too.
/// - **Consuming the active skill.** A real submit *takes* `app.active_skill`.
///   The preview clones it: the skill is still pending after an inspection,
///   and the previewed body is the one it would have produced. Dropping it
///   instead — which the first pass did — previewed an unwrapped prompt and
///   quietly under-reported the request by the whole skill instruction.
///
/// Without a prompt there is no next-turn route to resolve under auto model
/// routing and no next-turn body under any routing, so this reports a typed
/// unresolved state instead of recycling the installed route.
pub(crate) async fn build_preview_request_inputs(
    app: &App,
    config: &Config,
    engine_handle: &EngineHandle,
    hypothetical_prompt: Option<String>,
) -> crate::core::engine::preview::PreviewRequestInputs {
    use crate::core::engine::preview::{PreviewNextTurn, PreviewRequestInputs, PreviewUnresolved};

    let requested_model = if app.auto_model {
        "auto".to_string()
    } else {
        app.model.clone()
    };
    let prompt_supplied = hypothetical_prompt.is_some();
    let posture = |next_turn, unresolved| PreviewRequestInputs {
        mode: app.mode,
        allow_shell: app.allow_shell,
        trust_mode: app.trust_mode,
        auto_approve: app_auto_approve_enabled(app),
        approval_mode: app.approval_mode,
        allowed_tools: app.active_allowed_tools.clone(),
        dynamic_tools: Vec::new(),
        provenance: crate::core::ops::UserInputProvenance::ExternalUser,
        requested_model: requested_model.clone(),
        requested_reasoning: app.reasoning_effort.as_setting().to_string(),
        auto_model: app.auto_model,
        hypothetical_prompt_supplied: prompt_supplied,
        next_turn,
        unresolved,
    };

    let Some(prompt) = hypothetical_prompt else {
        // Never clear the unresolved flag just because a session has a route:
        // under auto routing the next prompt is what decides it.
        return posture(
            None,
            if app.auto_model {
                PreviewUnresolved::AutoRouteNeedsPrompt
            } else {
                PreviewUnresolved::NoPrompt
            },
        );
    };

    // Auto routing runs a model classifier. `/preview-request` is an offline
    // inspection command, so it stops before prompt resolution or the shared
    // planner can reach that call. Production remains responsible for Auto.
    if auto_router::should_resolve_auto_model_selection(app) {
        return posture(None, PreviewUnresolved::AutoRouteClassificationNotExecuted);
    }

    if app
        .hooks
        .has_hooks_for_event(crate::hooks::HookEvent::MessageSubmit)
    {
        return posture(None, PreviewUnresolved::MessageSubmitHooksConfigured);
    }

    // Clone, never `take`: an inspection may not consume the pending skill.
    let message = QueuedMessage {
        display: prompt.clone(),
        skill_instruction: app.active_skill.clone(),
        skill_provenance: app.active_skill_provenance.clone(),
        history_echoed: false,
    };
    let mut git_cache = crate::tui::git_mention::GitMentionCache::default();
    // Same failure surface as a real submit: a plugin-skill authority mismatch
    // aborts the turn there and must not be papered over with the raw prompt
    // here — that would describe a request the user could not send.
    let mut content = match queued_message_content_for_app(
        app,
        &message,
        std::env::current_dir().ok(),
        &mut git_cache,
    ) {
        Ok(content) => content,
        Err(error) => {
            return posture(
                None,
                PreviewUnresolved::PromptResolutionFailed(error.to_string()),
            );
        }
    };
    // A real submit appends the paused-command note before planning the route.
    // `plan_paused_command_message` is pure — it decides, it does not resume or
    // discard anything — so the preview can use the same value.
    let paused_dispatch = plan_paused_command_message(app, &prompt);
    if let Some(note) = paused_dispatch.note() {
        content.push_str(note);
    }

    let (app_route_identity, route_config) = app_scoped_runtime_config(app, config);
    let planned = plan_turn_route(TurnRoutePlanRequest {
        route_config: &route_config,
        app_route_identity: &app_route_identity,
        api_provider: app.api_provider,
        app_model: &app.model,
        auto_model: app.auto_model,
        reasoning_effort: app.reasoning_effort,
        mode: app.mode,
        content: &content,
        display_text: &prompt,
        auto_router_context: &auto_router::recent_auto_router_context(&app.api_messages),
        should_auto_resolve: false,
        allow_auto_router_response_cache: false,
        preflight_required: engine_handle.client_preflight_required(),
        auto_compact_user_configured: app.auto_compact_user_configured,
        auto_compact: app.auto_compact,
        auto_compact_threshold_percent: app.auto_compact_threshold_percent,
    })
    .await;

    match planned {
        Ok(planned) => {
            let prompt_context = crate::core::engine::NextTurnPromptContext::for_planned_turn(
                planned.route.identity.provider,
                planned.route.model.clone(),
                crate::route_budget::known_route_limits(planned.route.candidate.limits()),
                app.mode,
                paused_dispatch.goal_objective(app),
                app.goal.status,
                app.goal.token_budget,
                app.translation_enabled,
                app.verbosity.clone(),
            );
            posture(
                Some(Box::new(PreviewNextTurn {
                    content,
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
                })),
                PreviewUnresolved::NoPrompt,
            )
        }
        Err(error) => posture(None, PreviewUnresolved::PlanFailed(error)),
    }
}

pub(crate) fn build_engine_config(app: &App, config: &Config) -> EngineConfig {
    let provider = app.api_provider;
    let max_subagents = app.max_subagents.clamp(1, crate::config::MAX_SUBAGENTS);
    EngineConfig {
        model: app.model.clone(),
        active_route_limits: app.active_route_limits,
        workspace: app.workspace.clone(),
        // The App owns the session id (claimed before the Runtime store lock
        // and used for every checkpoint/autosave); the engine adopts it so the
        // engine conversation and the persisted session are the same record.
        session_id: app.current_session_id.clone(),
        subagent_state_root: None,
        allow_shell: app.allow_shell,
        trust_mode: app.trust_mode,
        notes_path: config.notes_path(),
        mcp_config_path: config.mcp_config_path(),
        mcp_oauth_callback_port: config.mcp_oauth_callback_port,
        mcp_oauth_callback_url: config.mcp_oauth_callback_url.clone(),
        skills_dir: app.skills_dir.clone(),
        skills_scan_codewhale_only: app.skills_scan_codewhale_only,
        plugin_registry: Some(std::sync::Arc::clone(&app.plugin_registry)),
        instructions: configured_instruction_sources(config),
        project_context_pack_enabled: config.project_context_pack_enabled(),
        translation_enabled: app.translation_enabled,
        verbosity: app.verbosity.clone(),
        // R1: finite, not `u32::MAX`. The old comment argued a runaway is
        // "human-noticeable", but an interactive session left running is
        // exactly where an unbounded loop spends real money unattended.
        // The default (200) is far above what a long multi-step plan needs;
        // operators who want more raise `[tui].max_model_steps`, and the
        // clamp keeps even the maximum finite.
        max_steps: config.max_model_steps(),
        max_subagents,
        max_admitted_subagents: config
            .max_admitted_subagents_for_provider(provider)
            .max(max_subagents),
        launch_concurrency: config
            .launch_concurrency_for_provider(provider)
            .max(app.mode.mode_delegation_launch_floor()),
        subagents_enabled: config.subagents_enabled_for_provider(provider),
        features: config.features(),
        auto_review_policy: config.auto_review_policy(),
        compaction: app.compaction_config(),
        todos: app.todos.clone(),
        plan_state: app.plan_state.clone(),
        goal_state: crate::tools::goal::new_shared_goal_state_from_host_status(
            app.goal.objective.clone(),
            app.goal.token_budget,
            app.goal.status,
        ),
        max_spawn_depth: config.subagent_max_spawn_depth_for_provider(provider),
        subagent_token_budget: config.subagent_token_budget_for_provider(provider),
        allowed_tools: app.active_allowed_tools.clone(),
        disallowed_tools: None,
        max_tool_calls: None,
        hook_executor: app.runtime_services.hook_executor.clone(),
        network_policy: config.network.clone().map(|toml_cfg| {
            crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
        }),
        snapshots_enabled: config.snapshots_config().enabled,
        snapshots_max_workspace_bytes: config
            .snapshots_config()
            .max_workspace_gb
            .saturating_mul(1024 * 1024 * 1024),
        lsp_config: config
            .lsp
            .clone()
            .map(crate::config::LspConfigToml::into_runtime),
        runtime_services: app.runtime_services.clone(),
        subagent_model_overrides: config.subagent_model_overrides(),
        fleet_roster: std::sync::Arc::new(crate::fleet::identity::load_effective_roster(
            &config.fleet_config(),
            &app.workspace,
            Some(app.plugin_registry.as_ref()),
        )),
        subagent_api_timeout: Duration::from_secs(
            config.subagent_api_timeout_secs_for_provider(provider),
        ),
        stream_chunk_timeout: Duration::from_secs(app.stream_chunk_timeout_secs),
        turn_wall_clock: config.turn_wall_clock(),
        stream_max_content_bytes: config.stream_max_content_bytes(),
        stream_max_duration: config.stream_max_duration(),
        subagent_heartbeat_timeout: Duration::from_secs(
            config.subagent_heartbeat_timeout_secs_for_provider(provider),
        ),
        prefer_bwrap: config.prefer_bwrap.unwrap_or(false),
        bwrap_extensions: crate::sandbox::BwrapMountExtensions {
            read_only_roots: config.bwrap_ro_roots.clone(),
            device_roots: config.bwrap_dev_roots.clone(),
        },
        read_denylist: config.read_denylist(),
        memory_enabled: config.memory_enabled(),
        memory_path: config.memory_path(),
        speech_output_dir: config.speech_output_dir(),
        vision_config: config.vision_model_config(),
        strict_tool_mode: config.strict_tool_mode.unwrap_or(false),
        goal_objective: app.goal.objective.clone(),
        goal_token_budget: app.goal.token_budget,
        goal_status: app.goal.status,
        goal_max_continuations: config.goal_max_continuations(),
        goal_continuation_delay_seconds: config.goal_continuation_delay_seconds(),
        locale_tag: app.ui_locale.tag().to_string(),
        workshop: {
            crate::tools::large_output_router::WorkshopConfig::install_active(
                config.workshop.as_ref(),
            );
            config.workshop.clone()
        },
        search_provider: config.search_provider(),
        search_api_key: config.search.as_ref().and_then(|s| s.api_key.clone()),
        search_base_url: config.search.as_ref().and_then(|s| s.base_url.clone()),
        tools_always_load: config.tools_always_load(),
        tools: config.tools.clone(),
        workspace_follow_symlinks: app.workspace_follow_symlinks,
        exec_policy_engine: config.exec_policy_engine.clone(),
        terminal_chrome_enabled: true,
        advisor_config: config
            .advisor
            .as_ref()
            .map(crate::tools::subagent::AdvisorConfig::from_toml)
            .unwrap_or_else(crate::tools::subagent::AdvisorConfig::disabled),
    }
}

#[cfg(test)]
pub(crate) fn build_app_system_prompt(app: &App, config: &Config) -> SystemPrompt {
    build_app_system_prompt_with_goal(app, config, app.goal.objective.as_deref())
}

pub(crate) fn build_app_system_prompt_with_goal(
    app: &App,
    config: &Config,
    goal_objective: Option<&str>,
) -> SystemPrompt {
    let instructions = configured_instruction_sources(config);
    let user_memory_block = crate::native_memory::native_prompt_block(
        config.memory_enabled(),
        &config.memory_path(),
        &app.workspace,
    );
    prompts::system_prompt_for_mode_with_context_skills_and_session(
        &app.workspace,
        None,
        Some(&app.skills_dir),
        Some(&instructions),
        prompts::PromptSessionContext {
            user_memory_block: user_memory_block.as_deref(),
            goal_objective,
            project_context_pack_enabled: config.project_context_pack_enabled(),
            locale_tag: app.ui_locale.tag(),
            translation_enabled: app.translation_enabled,
            model_id: &app.model,
            context_window_override: Some(crate::route_budget::route_context_window_tokens(
                app.api_provider,
                &app.model,
                app.active_route_limits,
            )),
            verbosity: app.verbosity.as_deref(),
            skills_scan_codewhale_only: app.skills_scan_codewhale_only,
            plugin_registry: Some(app.plugin_registry.as_ref()),
            mode: app.mode,
        },
    )
}

pub(crate) fn build_session_snapshot(
    app: &mut App,
    manager: &SessionManager,
) -> Result<SavedSession, String> {
    let model = app.model_selection_for_persistence();
    let work_state = match app.try_work_state_snapshot() {
        Ok(work_state) => work_state,
        Err(err) => app.last_known_work_state.clone().ok_or_else(|| {
            format!("automatic session snapshot skipped while Work state is busy: {err}")
        })?,
    };
    let mut session = if let Some(existing_id) = app.current_session_id.as_ref() {
        create_saved_session_with_id_and_mode(
            existing_id.clone(),
            &app.api_messages,
            &model,
            &app.workspace,
            u64::from(app.session.total_tokens),
            app.system_prompt.as_ref(),
            Some(app.mode.as_setting()),
        )
    } else {
        create_saved_session_with_mode(
            &app.api_messages,
            &model,
            &app.workspace,
            u64::from(app.session.total_tokens),
            app.system_prompt.as_ref(),
            Some(app.mode.as_setting()),
        )
    };
    let computed_title = session.metadata.title.clone();
    if let Some(cached) = app
        .current_session_metadata
        .as_ref()
        .filter(|cached| cached.id == session.metadata.id)
    {
        session.metadata.created_at = cached.created_at;
        session
            .metadata
            .parent_session_id
            .clone_from(&cached.parent_session_id);
        session.metadata.forked_from_message_count = cached.forked_from_message_count;
        session.metadata.archived = cached.archived;
    }
    // The cache above is a hint; disk is the authority for lifecycle state.
    // Re-reading here is what makes "an archive or rename cannot be reverted
    // by autosave" true regardless of which surface applied it or when
    // (#2934 / #4397). One bounded metadata-prefix read, not a transcript scan.
    let merged = manager.merge_persisted_lifecycle(&mut session.metadata);
    // Title resolution, in priority order:
    // 1. Disk, when the session already exists (#2934/#4397: a rename applied
    //    through the session manager is persisted and must survive autosave).
    // 2. The in-memory cache, when there is no disk record for the session
    //    yet. (The session picker normally persists renames to disk first via
    //    `rename_selected`; this branch covers sessions that have never been
    //    saved, where the cache is the only title source.)
    // 3. The title computed from the conversation (first user message).
    //    The cache is NOT a candidate on its own: it is only refreshed at the
    //    end of this function, so a snapshot taken before any user message
    //    pins it to the `DEFAULT_SESSION_TITLE` placeholder, and restoring it
    //    would prevent every later title update (the bug this block fixes).
    if !merged
        && let Some(cached) = app.current_session_metadata.as_ref()
        && cached.id == session.metadata.id
    {
        session.metadata.title.clone_from(&cached.title);
    }
    if session.metadata.title == crate::session_manager::DEFAULT_SESSION_TITLE
        && computed_title != crate::session_manager::DEFAULT_SESSION_TITLE
    {
        // The placeholder survived from an earlier snapshot; the conversation
        // now has a real first user message, so let the computed title win.
        // Known edge: a session deliberately renamed to the literal
        // placeholder title is treated the same way and yields to the
        // computed title on the next snapshot.
        session.metadata.title = computed_title;
    }
    if let Some(cached) = app.current_session_metadata.as_mut()
        && cached.id == session.metadata.id
    {
        cached.title.clone_from(&session.metadata.title);
        cached.archived = session.metadata.archived;
    }
    session
        .metadata
        .set_model_provider_route(app.api_provider.as_str(), app.provider_id_for_persistence());
    app.sync_cost_to_metadata(&mut session.metadata);
    session.context_references = app.session_context_references.clone();
    session.artifacts = app.session_artifacts.clone();
    session.work_state = work_state;
    session.last_auto_route = app.auto_route_for_persistence();
    session.window_title.clone_from(&app.window_title);
    app.current_session_metadata = Some(session.metadata.clone());
    // Claim ownership of this session for the process. From here on the
    // Runtime API refuses external renames/archives of it with a typed 409
    // rather than writing something the next snapshot would revert.
    //
    // Claiming here rather than at each of the ten `current_session_id`
    // assignment sites is deliberate: this is the function that establishes
    // "the TUI holds the authoritative copy", which is exactly the condition
    // the conflict protects. A session that has never been snapshotted has no
    // in-memory state to lose, so leaving it unclaimed is correct, not a gap.
    crate::session_manager::set_live_session(Some(&session.metadata.id));
    Ok(session)
}

/// Strip ANSI control codes / non-printable bytes from a streaming
/// text chunk. `pub(super)` because `tui::notifications` consumes it
/// from `crate::tui::ui` for its per-turn message composition.
pub(crate) fn sanitize_stream_chunk(chunk: &str) -> String {
    // Keep printable characters and common whitespace; drop control bytes.
    chunk
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// Ensure an in-flight streaming Assistant cell exists in history and return
/// its index. Thinking cells go through `streaming_thinking::ensure_active_entry`
/// (active cell) instead.
pub(crate) fn ensure_streaming_assistant_history_cell(app: &mut App) -> usize {
    if let Some(index) = app.streaming_message_index {
        return index;
    }
    app.add_message(HistoryCell::Assistant {
        content: String::new(),
        streaming: true,
    });
    let index = app.history.len().saturating_sub(1);
    app.streaming_message_index = Some(index);
    index
}

pub(crate) fn append_streaming_text(app: &mut App, index: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    app.resync_history_revisions();
    let Some(previous_revision) = app.history_revisions.get(index).copied() else {
        return;
    };
    let chained_from_revision = app
        .streaming_source_receipt
        .filter(|receipt| receipt.cell_index == index && receipt.to_revision == previous_revision)
        .map_or(previous_revision, |receipt| receipt.from_revision);
    let mut content_len = None;
    if let Some(HistoryCell::Assistant { content, .. }) = app.history.get_mut(index) {
        content.push_str(text);
        content_len = Some(content.len());
        // Bump only the streaming cell's per-cell revision so the transcript
        // cache re-renders just this cell. Without this, the cache would
        // either skip the update entirely (now that the global
        // history_version is no longer fanned out across every cell) or fall
        // back to a full re-wrap of the entire transcript every chunk.
        app.bump_history_cell(index);
    }
    let Some(content_len) = content_len else {
        return;
    };
    if let Some(to_revision) = app.history_revisions.get(index).copied() {
        app.streaming_source_receipt = Some(crate::tui::transcript::StreamingSourceReceipt {
            cell_index: index,
            from_revision: chained_from_revision,
            to_revision,
            content_len,
        });
    }
}

pub(crate) fn accrue_streaming_token_estimate(app: &mut App, visible_text: &str) {
    if visible_text.is_empty() {
        return;
    }
    app.streaming_output_token_estimate = app
        .streaming_output_token_estimate
        .saturating_add(estimate_output_tokens_from_text(visible_text));
}

pub(crate) fn commit_streaming_display_tick(
    app: &mut App,
    stream_display_clock: &mut StreamDisplayClock,
    now: Instant,
) -> bool {
    if !stream_display_clock.take_due(now) {
        return false;
    }

    let mut updated = false;
    if let Some(index) = app.streaming_message_index {
        let committed = app.streaming_state.commit_text(0);
        if !committed.is_empty() {
            append_streaming_text(app, index, &committed);
            accrue_streaming_token_estimate(app, &committed);
            updated = true;
        }
    } else if let Some(entry_idx) = app.streaming_thinking_active_entry {
        let committed = app.streaming_state.commit_text(0);
        if !committed.is_empty() {
            if app.translation_enabled {
                streaming_thinking::set_placeholder(app, entry_idx);
            } else {
                streaming_thinking::append(app, entry_idx, &committed);
            }
            updated = true;
        }
    }

    if app.streaming_state.has_pending_stream_text(0) {
        stream_display_clock.note_delta(now);
    }

    updated
}

pub(crate) fn live_tool_receipt_messages(
    app: &App,
    id: &str,
    raw: &str,
    success: bool,
) -> Vec<Message> {
    let mut messages = Vec::with_capacity(2);
    if let Some(tool_use_msg) = app.api_messages.iter().rev().find(|message| {
        message.content.iter().any(|block| {
            matches!(block, ContentBlock::ToolUse { id: tool_use_id, ..} if tool_use_id == id)
        })
    }) {
        messages.push(tool_use_msg.clone());
    }
    messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: raw.to_string(),
            is_error: Some(!success),
            content_blocks: None,
        }],
    });
    messages
}

pub(crate) fn compact_live_tool_receipt(
    messages: Vec<Message>,
    artifacts: Vec<crate::artifacts::ArtifactRecord>,
    raw: String,
) -> Option<String> {
    let (compacted, _) =
        crate::tool_output_receipts::compact_messages_for_persistence(&messages, &artifacts);
    let content = compacted
        .last()
        .and_then(|message| message.content.first())
        .and_then(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })?;
    if content != &raw && live_tool_content_is_receipt(content) {
        Some(content.clone())
    } else {
        None
    }
}

pub(crate) fn live_tool_content_is_receipt(content: &str) -> bool {
    content.trim_start().starts_with("[TOOL_OUTPUT_RECEIPT]")
}

/// Build the pending-input preview widget from current `App` state.
///
/// v0.6.6 (#122) wires all three buckets:
/// - `pending_steers` — typed during a running turn + Esc; held until the
///   abort lands and gets resubmitted as a fresh merged turn.
/// - `rejected_steers` — engine declined a mid-turn steer (scaffolding;
///   no engine path produces these yet but the bucket renders with a distinct
///   rejected-steer label).
/// - `queued_messages` — Enter while busy; drained at end-of-turn. In Operate,
///   the foreground operator dispatches these as additional background tasks.
pub(crate) fn build_pending_input_preview(app: &App) -> PendingInputPreview {
    let mut preview = PendingInputPreview::new();
    preview.locale = app.ui_locale;
    let selected_attachment = app.selected_composer_attachment_index();
    let mut attachment_index = 0usize;
    preview.context_items = crate::tui::file_mention::pending_context_previews(&app.input)
        .into_iter()
        .map(|item| {
            let selected = if item.removable {
                let selected = selected_attachment == Some(attachment_index);
                attachment_index += 1;
                selected
            } else {
                false
            };
            ContextPreviewItem {
                kind: item.kind,
                label: item.label,
                detail: item.detail,
                included: item.included,
                removable: item.removable,
                selected,
            }
        })
        .collect();
    preview.pending_steers = app
        .pending_steers
        .iter()
        .map(|m| m.display.clone())
        .collect();
    preview.rejected_steers = app.rejected_steers.iter().cloned().collect();
    preview.queued_messages = app
        .queued_messages
        .iter()
        .map(|m| m.display.clone())
        .collect();
    preview.editing_queued_message = app.queued_draft.as_ref().map(|draft| {
        if app.input.trim().is_empty() {
            draft.display.clone()
        } else {
            app.input.clone()
        }
    });
    preview
}

pub(crate) fn render(f: &mut Frame, app: &mut App, _config: &Config) -> Option<(u16, u16)> {
    let size = f.area();
    // The sixel block is re-reserved by the launch paint below when the
    // sixel tier is active; resetting first means any other screen (or a
    // dissolved card) reads as "no block" and the reconciler clears a
    // stranded image instead of re-emitting it.
    app.launch.sixel_mark_area = None;
    // Hover targets belong to the whole composed frame. Resetting inside the
    // transcript erased targets registered later by the composer and modals.
    crate::tui::hover_layer::begin_frame();
    let shell_area = session_shell_area(size);
    // Keep the view stack's focus-context texture prototype (#4823) in step
    // with the parsed setting each frame: a plain enum/theme copy, no
    // allocation. `Off` leaves the render byte-identical to before.
    app.view_stack
        .set_focus_texture(app.focus_texture, app.ui_theme);
    app.sidebar_hover = crate::tui::app::SidebarHoverState::default();
    app.viewport.last_approval_area = None;
    app.viewport.interaction_targets.clear();
    // Keep the OSC-0 whale title truthful to the current shell phase so
    // alt-tabbed sessions communicate state without a second in-app spinner.
    crate::tui::underwater::sync_title_activity(app);

    // Clear entire area with the configured app background.
    let background = Block::default().style(Style::default().bg(app.ui_theme.surface_bg));
    f.render_widget(background, size);

    // Show onboarding screen if needed
    if app.onboarding != OnboardingState::None {
        onboarding::render(f, size, app);
        // Onboarding is a backdrop, not a separate screen manager. Render any
        // native view above every onboarding step so shared pickers and the
        // first-run privacy disclosure cannot become invisible outside the
        // Provider step.
        if !app.view_stack.is_empty() {
            let buf = f.buffer_mut();
            app.view_stack.render(size, buf);
        }
        return None;
    }

    if app.launch.visible {
        // The launch screen lives inside the session shell frame (spec
        // §5b): the Tideline startup stage as the body, then the posture row
        // and the info line beneath it — the same chrome every post-session
        // screen wears, so opening Codewhale and working in it are one
        // design. Nothing paints above the stage; the launch header is the
        // stage's own. The pre-session composer docks in the stage's bottom
        // rows; completion entries are computed here — the same way the
        // session path below computes them for ComposerWidget — so the
        // stage can paint its popup (#5698 review finding 2); the mention
        // walker needs &mut App, rendering does not.
        let launch_slash_menu_entries = visible_slash_menu_entries(app, SLASH_MENU_LIMIT);
        let launch_mention_menu_entries =
            crate::tui::file_mention::visible_mention_menu_entries(app, app.mention_menu_limit);
        // The posture bar and the metrics line appear only once a session
        // exists: while the launch card is up the stage owns every row.
        let card_up = {
            let motion = app.motion_policy().allows_decorative() && !app.low_motion;
            app.launch
                .card_dissolve_progress(app.ambient_clock_ms, motion)
                < 1.0
        };
        let areas = if card_up {
            Layout::default()
                .direction(Direction::Vertical)
                .flex(ratatui::layout::Flex::Start)
                .constraints([Constraint::Min(1)])
                .split(size)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .flex(ratatui::layout::Flex::Start)
                .constraints([
                    Constraint::Min(1),    // stage: Tideline startup
                    Constraint::Length(1), // posture row (merged footer, slots 6+8)
                    Constraint::Length(1), // info line
                ])
                .split(size)
        };
        let stage_area = areas[0];
        let footer_area = areas.get(1).copied().unwrap_or_default();
        let info_area = areas.get(2).copied().unwrap_or_default();
        let startup = crate::tui::underwater::tideline_startup_from_app(app);
        let mut hitboxes = if startup.composer.enclosed {
            crate::tui::underwater::tideline_startup_hitboxes(stage_area)
        } else {
            crate::tui::underwater::tideline_startup_hitboxes_with_composer(stage_area, false)
        };
        // The card's clickable rows share the painter's plan geometry, so
        // hover and click rects match painted cells.
        hitboxes.rows = crate::tui::underwater::tideline_startup_row_hitboxes(stage_area, &startup);
        let sixel_area =
            crate::tui::underwater::render_tideline_startup(stage_area, f.buffer_mut(), &startup);
        app.launch.sixel_mark_area = if sixel_area.width > 0 {
            Some(sixel_area)
        } else {
            None
        };
        // The completion popup paints above the docked composer's input row,
        // over the stage rows it needs — the same caller-computed entries
        // the session popup rides.
        if let Some(input_row) = hitboxes
            .input
            .map(|area| area.y.saturating_sub(stage_area.y))
        {
            crate::tui::underwater::render_launch_completion_popup(
                stage_area,
                f.buffer_mut(),
                app,
                input_row,
                &launch_slash_menu_entries,
                &launch_mention_menu_entries,
            );
        }
        crate::tui::underwater::apply_launch_hitboxes(&hitboxes, &mut app.launch);
        // The merged footer is the screen's last row on every screen.
        if footer_area.height > 0 {
            let facts = crate::tui::phase_strip::tideline_footer_from_app(app, footer_area.width);
            let footer = facts.widget(
                &app.ui_theme,
                crate::tui::color_compat::ascii_safe_enabled(),
            );
            let buf = f.buffer_mut();
            Block::default()
                .style(Style::default().bg(app.ui_theme.footer_bg))
                .render(footer_area, buf);
            crate::tui::phase_strip::render_tideline_footer(footer_area, buf, &footer);
        }
        // The info line is the screen's last row, under the posture row. At
        // a height with no row for it, the stale rects must go too, or a
        // model/context click could route against cells nothing paints.
        let mut info_interactions = InfoLineInteractionHitboxes::default();
        if info_area.height > 0 {
            info_interactions = render_info_row(f, app, info_area);
        } else {
            app.viewport.last_infoline_hitboxes.clear();
        }
        register_info_interaction_targets(app, info_interactions);
        if !app.view_stack.is_empty() {
            if app.view_stack.top_kind() == Some(ModalKind::Approval) {
                app.viewport.last_approval_area = app.view_stack.top_occupied_region(size);
            }
            let buf = f.buffer_mut();
            app.view_stack.render(size, buf);
        }
        return None;
    }

    // Mini-window mode: when the host terminal window is pinned into its
    // small always-on-top form, hide the shell chrome and keep only what the
    // user opted to keep (`[mini_window]` in config.toml, or mutated live by
    // `/config mini_window.keep_*`). The message stream takes the rest.
    let mini = crate::tui::window_control::pinned();
    let mini_cfg = app.mini_window.clone();
    // The info line owns the shell's last row as exactly one row (spec §5b:
    // `Constraint::Length(1)`). It used to be the header; the founder moved
    // it to the bottom (SHELL-DESIGN-20260901 §2.0) so scrolling up reads as
    // intentional. `keep_header` still governs it in mini mode — the row it
    // names moved, not the preference.
    let info_height = if mini && !mini_cfg.keep_header {
        0
    } else {
        info_row_height_for(size.height)
    };
    // Evaluate the fully-idle predicate exactly once per frame. It decides
    // how many rows the rail may reserve and whether the idle ocean draws
    // its brand mark (in ChatWidget); calling it twice would let the
    // reservation and the render disagree inside a single frame.
    let idle_empty = crate::tui::widgets::should_render_empty_state(app);
    // The merged Tideline footer is the single bottom row (spec §3: slots
    // 6+8 collapsed; §5b `Constraint::Length(1)`): phase·cost·posture on the
    // left, depth·keys on the right. It hides with the rest of the footer
    // chrome in mini mode, never with the composer.
    let footer_height = if mini && !mini_cfg.keep_footer {
        0
    } else {
        crate::tui::phase_strip::height()
    };
    let slash_menu_entries = visible_slash_menu_entries(app, SLASH_MENU_LIMIT);
    let mention_menu_limit = app.mention_menu_limit;
    let mention_menu_entries =
        crate::tui::file_mention::visible_mention_menu_entries(app, mention_menu_limit);
    if !mention_menu_entries.is_empty() && app.mention_menu_selected >= mention_menu_entries.len() {
        app.mention_menu_selected = mention_menu_entries.len().saturating_sub(1);
    }
    let rail_budget = rail_row_budget(app, shell_area.width, shell_area.height, idle_empty);
    let top_work_strip_height = if mini && !mini_cfg.keep_todo {
        // Mini mode hides the strip; when the side rail is also hidden (the
        // default), drop the work-surface interaction state so stale
        // hitboxes from the pre-pin layout cannot swallow transcript clicks
        // or trigger phantom strip actions (review M1). A visible rail/strip
        // refreshes that state during its own render.
        if !mini_cfg.keep_sidebar {
            crate::tui::work_surface::collapse_strip(app);
        }
        0
    } else {
        crate::tui::work_surface::height(app, shell_area.width, shell_area.height, rail_budget)
    };

    // Nothing paints above the stage any more, so the body is the whole
    // shell area. The old two-pass split existed only to pin a header to row
    // zero against ratatui's Flex defaults (#1834); with no header there is
    // nothing to pin.
    let body_area = shell_area;

    let body_height = body_area.height;
    let composer_max_height = body_height
        .saturating_sub(
            MIN_CHAT_HEIGHT
                .saturating_add(footer_height)
                .saturating_add(info_height)
                .saturating_add(top_work_strip_height),
        )
        .max(MIN_COMPOSER_HEIGHT);
    let composer_height = if mini && !mini_cfg.keep_input {
        0
    } else {
        let composer_widget = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        );
        composer_widget.desired_height(shell_area.width)
    };

    // Pending-input preview (queued / steered messages). Empty when nothing's
    // queued, so zero height when idle. Phase 2 of #85 — solves the
    // "messages typed during a running turn vanish" complaint by giving the
    // user immediate visible feedback above the composer.
    let pending_preview = build_pending_input_preview(app);
    let desired_preview_height = if mini {
        0
    } else {
        pending_preview.desired_height(shell_area.width)
    };

    // The background-work chip (#5286) that used to pin a row above the
    // composer is gone: the posture bar's live counts own "what is in
    // flight" (one owner per fact), and nothing sits between the transcript
    // and the composer that is not a queued draft or an expanded panel.

    // WorkflowPanel unified activity surface (#4121). Expanded while running
    // (interactive drill-in above the composer); when collapsed the panel
    // takes no rows — its persistent status lives in the top status bar as a
    // header chip instead (#5040). Zero height when no panel.
    let desired_workflow_panel_height = if mini {
        0
    } else {
        app.workflow_panel
            .as_ref()
            .filter(|panel| panel.expanded)
            .map(|panel| panel.desired_height(shell_area.width))
            .unwrap_or(0)
    };
    let plugin_cta_height = if mini && !mini_cfg.keep_input {
        0
    } else {
        app.plugin_cta_row_height()
    };
    let auxiliary_budget = body_height.saturating_sub(
        top_work_strip_height
            .saturating_add(MIN_CHAT_HEIGHT)
            .saturating_add(composer_height)
            .saturating_add(footer_height)
            .saturating_add(info_height)
            .saturating_add(plugin_cta_height),
    );
    // Queued-only previews author the direct controls in row two (and fall
    // back to controls-only when just one row remains). Mixed previews retain
    // up to three compact rows at the release floor.
    let preview_cap = if size.height >= 20 { 4 } else { 3 };
    let preview_height = desired_preview_height.min(auxiliary_budget.min(preview_cap));
    let workflow_panel_height =
        desired_workflow_panel_height.min(auxiliary_budget.saturating_sub(preview_height));

    // Two pinned rows bracket the composer from below (SHELL-DESIGN-20260901
    // §2.0 item 3, §2.3b): the posture bar — permission · mode · live counts
    // · the one hint that applies now, with the remote-control state or a
    // live notice pinned right — then the metrics line — model · ctx · cost
    // · ttft · tok/s · ↓ tokens, with the help hint pinned right. Both rows
    // are reserved in every phase, so a turn moving between idle, thinking,
    // tool use, approval, completion, failure, and cancellation rewrites
    // text inside fixed rows — the composer is never displaced.
    // The work surface (roster, to-do) lives BELOW those two rows by default
    // and only when it has content — scrolling up is intentional history —
    // while `top` placement keeps the strip above the transcript. The strip
    // owns a slot at each end and only one has height, so every other slot
    // keeps its index in both placements (the stage and preview are
    // addressed by position below).
    // Bottom never falls back (only side rails do), so the configured
    // placement is the effective one here.
    let strip_below =
        app.work_surface.placement == crate::tui::work_surface::WorkSurfacePlacement::Bottom;
    let (strip_above_height, strip_below_height) = if strip_below {
        (0, top_work_strip_height)
    } else {
        (top_work_strip_height, 0)
    };
    let body_chunks = Layout::default()
        .direction(Direction::Vertical)
        .flex(ratatui::layout::Flex::Start)
        .constraints([
            Constraint::Length(strip_above_height), // Tasks + To-do above transcript (`top`)
            Constraint::Min(1),                     // Chat area
            Constraint::Length(workflow_panel_height), // Workflow panel (#4121)
            Constraint::Length(preview_height),     // Pending input preview (0 if empty)
            Constraint::Length(plugin_cta_height),  // Live plugin CTA (0 unless matched)
            Constraint::Length(composer_height),    // Composer
            Constraint::Length(footer_height),      // Posture bar
            Constraint::Length(info_height),        // Metrics line
            Constraint::Length(strip_below_height), // Roster + To-do under the chrome (`bottom`)
        ])
        .split(body_area);
    let strip_slot = if strip_below { 8 } else { 0 };
    let plugin_cta_slot = 4;
    let composer_slot = 5;
    let footer_slot = 6;
    let info_slot = 7;

    let (work_chat_area, side_work_area) = if mini && !mini_cfg.keep_sidebar {
        // Mini mode without the side rail: the transcript takes the whole
        // chat row. split_chat is skipped so the rail never reserves columns.
        (body_chunks[1], None)
    } else {
        crate::tui::work_surface::split_chat(app, body_chunks[1], rail_min_chat_width(idle_empty))
    };

    if top_work_strip_height > 0 {
        crate::tui::work_surface::render(f, body_chunks[strip_slot], app);
    } else if let Some(work_area) = side_work_area {
        crate::tui::work_surface::render(f, work_area, app);
    }

    // Render the transcript and optional file-tree sidecar. The underwater
    // default deliberately has no legacy right sidebar: Tasks and To-do own
    // the strip above, Fleet owns `/fleet`, and dense context owns its
    // inspector. Keeping the sidebar here was the architectural reason the
    // rejected build still read as the old TUI under a gradient.
    let shell_ocean;
    {
        // Defensive backstop (#400): fill the entire body area with ink
        // background before any sub-widgets render, so cells that end up
        // uncovered by layout splits (e.g. after file-tree toggle or
        // resize) don't retain stale content from a previous frame.
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(work_chat_area, f.buffer_mut());

        // When the file-tree pane is visible and the terminal is wide
        // enough, reserve the left ~25% for the file tree.
        let chat_area =
            if app.file_tree.is_some() && work_chat_area.width >= FILE_TREE_MIN_HOST_WIDTH {
                app.file_tree_visible = true;
                let split = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
                    .split(work_chat_area);
                let tree_area = split[0];
                let remaining = split[1];

                // Render the file-tree pane.
                if let Some(ref mut state) = app.file_tree {
                    crate::tui::file_tree::render_file_tree(f, tree_area, state, app.ui_theme.mode);
                }

                remaining
            } else {
                app.file_tree_visible = false;
                work_chat_area
            };
        app.sidebar_hover_tooltip = None;

        if app.agent_focus.is_some() {
            // A focused worker's full transcript owns the conversation area;
            // the ocean column and every other shell surface stay as they are.
            //
            // The widget below is built only to sample the ocean column, but
            // its constructor also consumes `pending_scroll_delta` into the
            // (invisible) main-transcript scroll state — which would starve
            // the focused transcript of every PageUp/PageDown and wheel
            // event. Park the delta across the sample so `render_focus`
            // receives it and the focused pane scrolls exactly like the main
            // transcript.
            let parked_scroll_delta = app.viewport.pending_scroll_delta;
            app.viewport.pending_scroll_delta = 0;
            {
                let chat_widget = ChatWidget::new(app, chat_area).with_ocean_viewport(size);
                shell_ocean = chat_widget.ocean_column();
            }
            app.viewport.pending_scroll_delta = parked_scroll_delta;
            crate::tui::agent_focus::refresh_focus(app);
            let buf = f.buffer_mut();
            crate::tui::agent_focus::render_focus(app, chat_area, buf);
        } else {
            let chat_widget = ChatWidget::new(app, chat_area).with_ocean_viewport(size);
            shell_ocean = chat_widget.ocean_column();
            let buf = f.buffer_mut();
            chat_widget.render(chat_area, buf);
        }
    }

    // Workflow panel between chat and pending-input preview (#4121).
    if workflow_panel_height > 0 {
        if let Some(panel) = app.workflow_panel.as_ref() {
            let area = body_chunks[2];
            app.viewport.last_workflow_panel_area = Some(area);
            app.viewport.last_workflow_cancel_area =
                panel.cancel_hint_span(area.width).map(|(start, end)| Rect {
                    x: area.x.saturating_add(start),
                    y: area.y,
                    width: end.saturating_sub(start),
                    height: 1,
                });
            let buf = f.buffer_mut();
            panel.render(area, buf);
        }
    } else {
        app.viewport.last_workflow_panel_area = None;
        app.viewport.last_workflow_cancel_area = None;
    }

    // Render pending-input preview (queued/steered messages, if any).
    if preview_height > 0 {
        let buf = f.buffer_mut();
        pending_preview.render(body_chunks[3], buf);
    }

    if plugin_cta_height > 0 {
        let buf = f.buffer_mut();
        crate::tui::plugin_suggestions::draw_plugin_cta(app, body_chunks[plugin_cta_slot], buf);
    } else {
        app.viewport.last_plugin_cta_area = None;
        app.viewport.last_plugin_cta_review_area = None;
        app.viewport.last_plugin_cta_dismiss_area = None;
    }

    // Render composer
    let cursor_pos = {
        let composer_widget = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        );
        let buf = f.buffer_mut();
        composer_widget.render(body_chunks[composer_slot], buf);
        composer_widget.cursor_pos(body_chunks[composer_slot])
    };
    app.viewport.last_composer_area = Some(body_chunks[composer_slot]);
    {
        let area = body_chunks[composer_slot];
        let composer_widget = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        );
        let input_plane = composer_widget.inner_area(area);
        app.viewport.last_composer_content = Some(input_plane);

        // Compute scroll offset and top padding for mouse coordinate mapping.
        let input_text = app.composer_display_input();
        let input_cursor = app.composer_display_cursor();
        let content_geometry = crate::tui::widgets::composer_content_geometry(
            input_plane,
            app.is_history_search_active(),
        );
        let content_width = content_geometry.text_width();
        let menu_lines = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        )
        .active_menu_reserved_rows();
        let budget =
            crate::tui::widgets::composer_input_rows_budget(input_plane.height, menu_lines);
        let (_, _, _, scroll_offset) = crate::tui::widgets::layout_input_with_scroll(
            input_text,
            input_cursor,
            content_width,
            budget,
        );
        let visual_rows = if input_text.is_empty() {
            let hint: Option<std::borrow::Cow<'_, str>> = if let Some(ref suggestion) =
                app.prompt_suggestion
                && !app.is_history_search_active()
            {
                Some(std::borrow::Cow::Borrowed(suggestion.as_str()))
            } else {
                Some(crate::tui::widgets::composer_empty_hint_text(app))
            };
            crate::tui::widgets::empty_composer_visual_rows(hint.as_deref(), content_width, budget)
        } else {
            // Count wrapped lines (approximation matching the render path).
            crate::tui::widgets::wrap_input_lines_for_mouse(input_text, content_width).len()
        };
        let top_padding = budget.saturating_sub(visual_rows.clamp(1, budget));
        app.viewport.last_composer_scroll_offset = scroll_offset;
        app.viewport.last_composer_top_padding = top_padding;
    }
    // The posture bar is the first row under the composer: permission chip
    // (never sheds), mode, live counts, the one hint that applies now, with
    // the remote-control state or a live notice pinned right.
    if footer_height > 0 {
        let area = body_chunks[footer_slot];
        let facts = crate::tui::phase_strip::tideline_footer_from_app(app, area.width);
        let footer = facts.widget(
            &app.ui_theme,
            crate::tui::color_compat::ascii_safe_enabled(),
        );
        let buf = f.buffer_mut();
        Block::default()
            .style(Style::default().bg(app.ui_theme.footer_bg))
            .render(area, buf);
        crate::tui::phase_strip::render_tideline_footer(area, buf, &footer);
    }

    // The metrics line sits directly under the posture bar: model · ctx ·
    // cost · ttft · tok/s · ↓ tokens, with the help hint pinned right.
    let mut info_interactions = InfoLineInteractionHitboxes::default();
    if info_height > 0 {
        info_interactions = render_info_row(f, app, body_chunks[info_slot]);
    } else {
        app.viewport.last_infoline_hitboxes.clear();
    }
    register_info_interaction_targets(app, info_interactions);

    // The underwater shell is one water column, not a stack of independently
    // shaded panels. Continue the transcript's absolute-row ramp through each
    // ordinary shell surface after its foreground has rendered. Semantic
    // backgrounds such as selection, hover, errors, and code blocks do not
    // match these base colors and therefore remain intact.
    if let Some(column) = shell_ocean {
        // The working canvas may keep a small responsive gutter, but the water
        // does not stop at that content edge. Paint the cleared terminal floor
        // first so wide layouts read as one ocean rather than a blue card
        // floating between black banks. `paint_matching` leaves every semantic
        // widget background untouched.
        column.paint_matching(size, f.buffer_mut(), app.ui_theme.surface_bg);
        if top_work_strip_height > 0 {
            column.paint_matching(
                body_chunks[strip_slot],
                f.buffer_mut(),
                app.ui_theme.surface_bg,
            );
        }
        if let Some(side_area) = side_work_area {
            column.paint_matching(side_area, f.buffer_mut(), app.ui_theme.surface_bg);
        }
        column.paint_matching(work_chat_area, f.buffer_mut(), app.ui_theme.surface_bg);
        column.paint_matching(body_chunks[2], f.buffer_mut(), app.ui_theme.surface_bg);
        column.paint_matching(body_chunks[3], f.buffer_mut(), app.ui_theme.surface_bg);
        if plugin_cta_height > 0 {
            column.paint_matching(
                body_chunks[plugin_cta_slot],
                f.buffer_mut(),
                app.ui_theme.composer_bg,
            );
        }
        column.paint_matching(
            body_chunks[composer_slot],
            f.buffer_mut(),
            app.ui_theme.composer_bg,
        );
        if footer_height > 0 {
            column.paint_matching(
                body_chunks[footer_slot],
                f.buffer_mut(),
                app.ui_theme.footer_bg,
            );
        }
    }
    crate::tui::hover_layer::apply_resolved_effects(
        f.buffer_mut(),
        app.effective_low_motion_for_status(),
        &app.ui_theme,
    );
    if !app.view_stack.is_empty() {
        // The live transcript overlay snapshots the app's history + active
        // cell on each render so streaming mutations propagate. Other views
        // are static and skip this refresh.
        if app.view_stack.top_kind() == Some(ModalKind::LiveTranscript) {
            refresh_live_transcript_overlay(app);
        } else if app.view_stack.top_kind() == Some(ModalKind::ContextInspector) {
            refresh_context_inspector_overlay(app);
        }
        if app.view_stack.top_kind() == Some(ModalKind::Approval) {
            app.viewport.last_approval_area = app.view_stack.top_occupied_region(size);
        }
        let buf = f.buffer_mut();
        app.view_stack.render(size, buf);
    }

    cursor_pos
}

/// Hide the real terminal caret before ratatui applies a frame diff.
///
/// A diff moves the terminal cursor through every changed run. Electron/xterm
/// IME bridges (notably Tabby on Windows, #5023) can observe those transient
/// positions even though the final frame is correct, which makes the native
/// candidate window jump around the screen. Keep the caret hidden for the
/// whole diff and pair this with [`finish_frame_cursor`] after the draw.
pub(super) fn prepare_frame_cursor<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
) -> std::result::Result<(), B::Error> {
    terminal.hide_cursor()
}

/// Restore the composer caret in IME-safe order: position first, reveal last.
///
/// Ratatui's `Frame::set_cursor_position` path currently calls `show_cursor`
/// before `set_cursor_position`. That briefly exposes the stale or last-diff
/// position to the terminal's IME bridge. Owning the final two operations here
/// preserves ratatui's internal cursor tracking while ensuring there is only
/// one visible caret position per completed frame (#5023).
pub(super) fn finish_frame_cursor<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    cursor_pos: Option<(u16, u16)>,
) -> std::result::Result<(), B::Error> {
    if let Some(cursor_pos) = cursor_pos {
        terminal.set_cursor_position(cursor_pos)?;
        terminal.show_cursor()?;
    }
    Ok(())
}

/// Draw a complete application frame, optionally with a full viewport reset.
///
/// When `full_repaint` is true, the terminal scroll margins and origin mode
/// are reset, the screen is cleared, ratatui's buffer is emptied, and then
/// the full UI is drawn — all within a single DEC 2026 synchronized-update
/// batch so GPU-accelerated terminals (Ghostty, VS Code, Kitty) render one
/// complete frame instead of a blank intermediate frame followed by the UI.
///
/// When `full_repaint` is false, only the diff from the previous draw is
/// written (normal incremental update path).
/// Reconcile the sixel tier's live image with this frame's reservation, in
/// the frame's own synchronized update so the pixels land atomically with
/// the cells around them. Steady state (same block as last frame) emits no
/// bytes at all: ratatui never rewrites the reserved blank cells, so the
/// image survives redraws untouched. A move clears the old block first;
/// a tier exit clears and stops. Write errors are logged, never fatal —
/// the blank block simply stays blank until the next frame retries.
pub(crate) fn reconcile_launch_sixel(writer: &mut impl std::io::Write, app: &mut App) {
    use crate::tui::mark;
    let field_bg = mark::sixel_field_bg(&app.ui_theme, app.launch.sixel_terminal_bg);
    // Fullscreen stage coordinates already are screen cells (both 0-based;
    // the 1-based CUP shift happens in the sequence builders). Inline
    // viewports have no stable origin, so the tier never reserves there
    // and this maps nothing.
    let want = if mark::sixel_graphics_supported() && app.use_alt_screen() && field_bg.is_some() {
        app.launch.sixel_mark_area
    } else {
        None
    };
    if want == app.launch.sixel_emitted {
        return;
    }
    let Some(bg) = field_bg else {
        // No exact field colour to paint with: hold the current image and
        // retry next frame rather than flashing a wrong background.
        tracing::debug!(target: "sixel_graphics", "no RGB field; holding sixel state");
        return;
    };
    if let Some(old) = app.launch.sixel_emitted {
        let bytes = mark::sixel_clear_sequence(old, bg);
        if writer.write_all(&bytes).is_err() {
            tracing::debug!(target: "sixel_graphics", "sixel clear failed");
            return;
        }
        app.launch.sixel_emitted = None;
    }
    if let Some(block) = want {
        let sequence = app
            .launch
            .sixel_cell_px
            .and_then(|cell_px| mark::sixel_mark_sequence(bg, cell_px));
        if let Some(sequence) = sequence {
            let bytes = mark::sixel_positioned_sequence(block, &sequence);
            if writer.write_all(&bytes).is_err() {
                tracing::debug!(target: "sixel_graphics", "sixel emission failed");
                return;
            }
            app.launch.sixel_emitted = Some(block);
        } else {
            tracing::debug!(
                target: "sixel_graphics",
                "sixel raster unavailable; the blank block holds"
            );
        }
    }
}

pub(crate) fn draw_app_frame_inner(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &Config,
    full_repaint: bool,
) -> Result<()> {
    terminal.backend_mut().set_palette_mode(app.ui_theme.mode);
    terminal.backend_mut().set_theme(app.theme_id, app.ui_theme);
    // DEC 2026 wrapping is on by default but can be turned off for
    // terminals that mishandle it (Ptyxis 50.x + VTE 0.84.x flashes the
    // whole viewport on every wrapped frame instead of deferring as the
    // standard requires). Settings::synchronized_output_enabled resolves
    // the user's setting against the Ptyxis env auto-detect.
    let wrap_in_sync_update = app.synchronized_output_enabled;
    if wrap_in_sync_update {
        let _ = terminal.backend_mut().write_all(BEGIN_SYNC_UPDATE);
    }

    // Run fallible draw operations in a closure so END_SYNC_UPDATE is
    // always sent even if an intermediate step fails. Without this, a
    // failing `?` would return early and leave the terminal stuck in
    // synchronized-update mode (screen frozen).
    let result = (|| -> Result<()> {
        // The terminal cursor itself is also input-method geometry. Hide it
        // before clear/diff operations move it, then restore the one composer
        // position after ratatui finishes drawing (#5023).
        prepare_frame_cursor(terminal)?;
        if full_repaint {
            terminal.backend_mut().write_all(TERMINAL_ORIGIN_RESET)?;
            terminal.clear()?;
            // A repaint wipes sixel pixels with everything else; forget the
            // live image so the reconciler below re-emits it this frame.
            app.launch.sixel_emitted = None;
        }
        let mut cursor_pos = None;
        terminal.draw(|f| cursor_pos = render(f, app, config))?;
        finish_frame_cursor(terminal, cursor_pos)?;
        // Inside the synchronized update: the pixels land atomically with
        // the cells. Steady state emits nothing.
        reconcile_launch_sixel(terminal.backend_mut(), app);
        Ok(())
    })();

    // Always end the synchronized update, regardless of success or failure.
    if wrap_in_sync_update {
        let _ = terminal.backend_mut().write_all(END_SYNC_UPDATE);
    }
    let _ = terminal.backend_mut().flush();
    result
}

/// Count how many `HistoryCell::User` entries currently live in the
/// transcript. Used by the backtrack state machine to decide whether
/// there's anything to rewind to. Walks `app.history` directly so it
/// stays accurate even mid-stream (the streaming Assistant cell never
/// counts as a user turn).
pub(crate) fn count_user_history_cells(app: &App) -> usize {
    app.history
        .iter()
        .filter(|cell| matches!(cell, HistoryCell::User { .. }))
        .count()
}

/// Find the absolute index of the Nth-from-tail `HistoryCell::User` in
/// `app.history`. `depth` of 0 selects the most recent user cell.
/// Returns `None` if `depth` is out of range.
pub(crate) fn find_user_cell_index_from_tail(app: &App, depth: usize) -> Option<usize> {
    let mut count = 0usize;
    for (idx, cell) in app.history.iter().enumerate().rev() {
        if matches!(cell, HistoryCell::User { .. }) {
            if count == depth {
                return Some(idx);
            }
            count += 1;
        }
    }
    None
}

/// Truncate `text` to at most `max_chars` characters, cutting at the last
/// natural phrase boundary (`.`, `,`, `:`, `;`, `—`, `-`, or whitespace)
/// so words are never split. Appends `…` only when text was actually cut.
pub(crate) fn short_title_truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    // Find the boundary as a character index. `str::rfind` returns a byte
    // offset, which mis-counts multi-byte UTF-8 text when fed back into
    // `chars().take()`, so operate on `Vec<char>` instead.
    let candidate: Vec<char> = text.chars().take(max_chars).collect();
    let boundary = candidate
        .iter()
        .rposition(|&c| matches!(c, '.' | ',' | ':' | ';' | '—' | '-'))
        .or_else(|| candidate.iter().rposition(|&c| c == ' '))
        .unwrap_or(max_chars.min(candidate.len()).saturating_sub(1));
    let cut: String = text.chars().take(boundary.max(1)).collect();
    format!("{cut}…")
}

pub(crate) fn compact_user_context_display(content: &str) -> String {
    content
        .split("\n\n---\n\nLocal context from @mentions:")
        .next()
        .unwrap_or(content)
        .to_string()
}

#[cfg(test)]
pub(crate) fn transcript_scroll_percent(top: usize, visible: usize, total: usize) -> Option<u16> {
    if total <= visible {
        return None;
    }

    let max_top = total.saturating_sub(visible);
    if max_top == 0 {
        return None;
    }

    let clamped_top = top.min(max_top);
    let percent = ((clamped_top as f64 / max_top as f64) * 100.0).round() as u16;
    Some(percent.min(100))
}

pub(crate) fn estimated_context_tokens(app: &App) -> Option<i64> {
    let message_count = app.api_messages.len();
    let mut cache = app.context_token_cache.borrow_mut();
    if cache.message_tokens.len() > message_count {
        cache.message_tokens.truncate(message_count);
    }
    while cache.message_tokens.len() < message_count {
        let index = cache.message_tokens.len();
        cache
            .message_tokens
            .push(estimate_tokens(&app.api_messages[index..=index]));
    }
    // The final assistant/tool message may grow while streaming. Recompute
    // only that tail entry; historical messages remain O(1) on steady frames.
    if message_count > 0 {
        let last = message_count - 1;
        cache.message_tokens[last] = estimate_tokens(&app.api_messages[last..=last]);
    }
    let message_tokens = cache
        .message_tokens
        .iter()
        .copied()
        .sum::<usize>()
        .saturating_mul(3)
        .div_ceil(2);
    let system_tokens =
        estimate_input_tokens_conservative(&[], app.system_prompt.as_ref()).saturating_sub(48);
    let estimated = message_tokens
        .saturating_add(system_tokens)
        .saturating_add(message_count.saturating_mul(12))
        .saturating_add(48);
    i64::try_from(estimated).ok()
}

pub(crate) fn context_usage_snapshot(app: &App) -> Option<(i64, u32, f64)> {
    let max = crate::route_budget::route_context_window_tokens(
        app.api_provider,
        app.effective_model_for_budget(),
        app.active_route_limits,
    );
    context_usage_snapshot_for_window(app, max)
}

pub(crate) fn context_usage_snapshot_for_window(app: &App, max: u32) -> Option<(i64, u32, f64)> {
    let max_i64 = i64::from(max);
    let reported = app
        .session
        .last_prompt_tokens
        .map(i64::from)
        .map(|tokens| tokens.max(0));
    let estimated = estimated_context_tokens(app).map(|tokens| tokens.max(0));

    // Always prefer the estimated current-context size (computed from
    // `app.api_messages`) when we have it. Reported `last_prompt_tokens`
    // comes from `Event::TurnComplete.usage`, which the engine builds with
    // `turn.add_usage` — that SUMS input_tokens across every round in the
    // turn, so a multi-round tool-call turn reports a value much larger
    // than the actual context window state, then the next single-round
    // turn drops back to a single round's input_tokens. User-visible %
    // was bouncing 31% → 9% (#115) because of this. The estimate is
    // monotonic wrt conversation growth, which is what a "context filling
    // up" indicator should show. We still consult `reported` only as a
    // fallback when no estimate is available (e.g., immediately after a
    // session restore before the api_messages are populated).
    let used = match (estimated, reported) {
        (Some(estimated), _) => estimated.min(max_i64),
        (None, Some(reported)) => reported.min(max_i64),
        (None, None) => return None,
    };

    let max_f64 = f64::from(max);
    let used_f64 = used as f64;
    let percent = ((used_f64 / max_f64) * 100.0).clamp(0.0, 100.0);
    Some((used, max, percent))
}

/// True while a `workflow` tool is executing in the foreground (active cell)
/// or still shown as running in history. Used to keep per-subagent completion
/// notifications quiet during a workflow run under `final-only`.
pub(crate) fn workflow_tool_is_running(app: &App) -> bool {
    fn is_running_workflow(cell: &HistoryCell) -> bool {
        matches!(
            cell,
            HistoryCell::Tool(ToolCell::Generic(tool))
                if tool.name == "workflow" && tool.status == ToolStatus::Running
        )
    }
    app.history.iter().any(is_running_workflow)
        || app
            .active_cell
            .as_ref()
            .is_some_and(|active| active.entries().iter().any(is_running_workflow))
}

#[cfg(test)]
mod tests {
    use super::{register_info_interaction_targets, render_info_row, short_title_truncate};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn infoline_route_segment_registers_interaction_target() {
        let mut app =
            crate::test_support::test_app_with_options(crate::test_support::test_tui_options("."));
        let mut terminal =
            Terminal::new(TestBackend::new(160, 1)).expect("info-line test terminal should build");

        terminal
            .draw(|frame| {
                let area = frame.area();
                let hitboxes = render_info_row(frame, &mut app, area);
                register_info_interaction_targets(&mut app, hitboxes);
            })
            .expect("info line should render");

        let segment = app
            .viewport
            .last_infoline_hitboxes
            .iter()
            .find(|hitbox| hitbox.id == crate::tui::infoline::InfoSegmentId::Model)
            .expect("a wide info line should paint its model segment");
        let target = app
            .viewport
            .interaction_targets
            .iter()
            .find(|target| target.id == crate::tui::tideline::InteractionTargetId::HEADER_ROUTE)
            .expect("painted route segment should have a typed target");

        assert_eq!(target.area, segment.area);
        assert_eq!(
            target.keyboard_action,
            Some(crate::tui::tideline::InteractionAction::OpenProviderPicker)
        );
        assert_eq!(target.mouse_action, target.keyboard_action);
        assert_eq!(
            target.inspect_detail,
            crate::tui::tideline::InspectDetail::Route
        );
    }

    /// "Where did the github info go?" — the workspace segment names the
    /// repository when `origin` resolves to a forge slug, and only falls back
    /// to the folder basename when it does not. The basename rides along as
    /// the segment's shorter form so a long slug never costs the row a whole
    /// fact.
    #[test]
    fn truncates_at_ascii_word_boundary() {
        assert_eq!(short_title_truncate("hello world foo", 10), "hello…");
    }

    #[test]
    fn truncates_non_ascii_titles_by_char_count_not_bytes() {
        // `str::rfind` returns a byte offset; using it as a char count used to
        // cut past the limit and mid-word on multi-byte input.
        assert_eq!(
            short_title_truncate("你好 world and more", 10),
            "你好 world…"
        );
    }

    #[test]
    fn truncates_at_punctuation_boundary() {
        assert_eq!(short_title_truncate("hello, world", 8), "hello…");
    }

    #[test]
    fn truncates_mid_word_when_no_boundary_exists() {
        assert_eq!(short_title_truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn leaves_short_titles_untouched() {
        assert_eq!(short_title_truncate("short", 10), "short");
    }
}

#[cfg(test)]
mod one_owner_tests;
