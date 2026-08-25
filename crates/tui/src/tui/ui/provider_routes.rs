//! Provider and route plumbing reached from the UI: switching providers,
//! MCP import/reload, balance and catalog fetches, and onboarding's
//! provider/trust steps.
//!
//! Moved verbatim out of `ui.rs`.

use super::*;

pub(crate) fn complete_trust_directory_onboarding(
    app: &mut App,
    config: &Config,
) -> Result<(), String> {
    let enter_hint = app.tr(MessageId::OnboardTrustEnterHint).into_owned();
    onboarding::mark_trusted(&app.workspace).map_err(|err| err.to_string())?;
    app.trust_mode = true;
    // `rebind`, not `new`: trusting the directory can add project hooks, but
    // it does not start a new session. Hooks that already fired this session
    // reported a `DEEPSEEK_SESSION_ID`, and it has to keep meaning the same
    // session afterwards.
    app.hooks = app.hooks.rebind(
        crate::hooks::HooksConfig::load_with_project_and_plugins(
            config.hooks_config(),
            &app.workspace,
            Some(app.plugin_registry.as_ref()),
        ),
        app.workspace.clone(),
    );
    app.runtime_services.hook_executor = Some(std::sync::Arc::new(app.hooks.clone()));
    app.status_message = None;
    app.status_toasts.retain(|toast| toast.text != enter_hint);
    advance_after_trust_directory_choice(app);
    Ok(())
}

/// Continue past the trust step without recording workspace trust.
///
/// Tools and hooks stay restricted for this session; the next launch will
/// re-prompt until the user trusts (or uses an explicit trust command).
pub(crate) fn continue_without_trusting_directory(app: &mut App) {
    app.trust_mode = false;
    app.status_message = Some(app.tr(MessageId::OnboardTrustUntrustedNotice).to_string());
    advance_after_trust_directory_choice(app);
}

pub(crate) fn advance_after_trust_directory_choice(app: &mut App) {
    if app.onboarding_workspace_trust_gate {
        app.onboarding_workspace_trust_gate = false;
        app.onboarding = OnboardingState::None;
    } else {
        // Both a first run and missing-key recovery end on the ready screen;
        // a trust-gate-only launch (already onboarded) exits directly above.
        app.onboarding = OnboardingState::Ready;
    }
}

/// Decide the onboarding route for one key press.
///
/// Two invariants this encodes, both regressions reported in #4763:
/// Ctrl+C quits from *any* onboarding state — a modal on the stack must not
/// swallow it — and Escape is never intercepted on the picker's behalf, so
/// the picker can back out one stage at a time instead of the shell popping
/// the whole modal from a key/OAuth sub-stage.
pub(crate) fn onboarding_key_route(
    onboarding: OnboardingState,
    top_kind: Option<ModalKind>,
    key: &KeyEvent,
) -> OnboardingKeyRoute {
    if onboarding == OnboardingState::None {
        return OnboardingKeyRoute::Legacy;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return OnboardingKeyRoute::Quit;
    }
    // Checked before the picker claim: the offline exit must stay reachable
    // from behind a modal the user cannot satisfy.
    if onboarding == OnboardingState::Provider && is_explore_offline_shortcut(key) {
        return OnboardingKeyRoute::ExploreOffline;
    }
    if onboarding == OnboardingState::Provider && top_kind == Some(ModalKind::ProviderPicker) {
        return OnboardingKeyRoute::ProviderPicker;
    }
    OnboardingKeyRoute::Legacy
}

pub(crate) fn back_from_provider_onboarding(app: &mut App) {
    if app.onboarding_missing_key_recovery {
        // A returning user declined missing-key recovery: leave onboarding
        // for the offline composer without mutating the saved route.
        app.onboarding = OnboardingState::None;
        app.status_message = None;
        app.needs_redraw = true;
        return;
    }
    // Esc walks back to the previous decision this run actually asked: the
    // language screen when it appeared, otherwise the welcome screen.
    app.onboarding = if app.onboarding_had_language_step {
        OnboardingState::Language
    } else {
        OnboardingState::Welcome
    };
    app.status_message = None;
}

pub(crate) fn complete_provider_picker_onboarding(app: &mut App, provider: ApiProvider) {
    // Ordinary `/provider` changes stay session-local until the operator
    // answers the route-save prompt. Onboarding is different: choosing a
    // provider is the explicit decision that establishes the startup route.
    // Persist the exact live identity/model before advancing, otherwise a
    // clean first run can finish on Ollama (or another non-DeepSeek route)
    // while the next launch silently reconstructs the old DeepSeek default.
    // `settings.toml`, rather than a workspace `config.toml`, is the durable
    // user-global owner for this choice.
    let provider_action_receipt = app.status_message.take();
    let startup_default_receipt = match app.try_save_live_route_as_startup_default() {
        Ok(receipt) => receipt,
        Err(err) => {
            // Persistence is part of completing first-run provider setup. Keep
            // the provider step active on failure so the current session may
            // use the selected route, but onboarding cannot claim that the
            // next launch will restore it. The exact selected provider remains
            // focused for an immediate retry.
            app.onboarding_provider = provider;
            app.onboarding_needs_api_key = true;
            app.status_message = Some(match provider_action_receipt {
                Some(receipt) if !receipt.trim().is_empty() => {
                    format!("{receipt} · Save failed: {err}")
                }
                _ => format!("Save failed: {err}"),
            });
            app.needs_redraw = true;
            return;
        }
    };
    app.onboarding_provider = provider;
    app.onboarding_needs_api_key = false;
    app.api_key_env_only = false;
    app.offline_mode = false;
    onboarding::advance_onboarding_after_provider(app);
    // `advance_onboarding_after_provider` clears the previous switch status.
    // Restore the persistence receipt last so an I/O failure remains visible
    // instead of allowing onboarding to imply that the restart route landed.
    app.status_message = Some(match provider_action_receipt {
        Some(receipt) if !receipt.trim().is_empty() => {
            format!("{receipt} · {startup_default_receipt}")
        }
        _ => startup_default_receipt,
    });
}

pub(crate) fn complete_provider_picker_onboarding_if_switched(
    app: &mut App,
    provider: ApiProvider,
    switched: bool,
) {
    if switched && app.onboarding == OnboardingState::Provider {
        complete_provider_picker_onboarding(app, provider);
    }
}

/// How one prepaid provider publishes remaining credit. Each variant is a
/// distinct wire contract — do not send DeepSeek `/user/balance` to a
/// provider that does not speak it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BalanceApi {
    DeepSeekUserBalance,
    OpenRouterCredits,
    SiliconFlowUserInfo,
}

fn balance_api_for(provider: ApiProvider) -> Option<BalanceApi> {
    match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => Some(BalanceApi::DeepSeekUserBalance),
        ApiProvider::Openrouter => Some(BalanceApi::OpenRouterCredits),
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => {
            Some(BalanceApi::SiliconFlowUserInfo)
        }
        _ => None,
    }
}

/// Fetch remaining credit for the active prepaid provider.
///
/// Returns `None` on any error (network, auth, parse) — callers treat that
/// as "balance unknown" and keep the previous value.
pub(crate) async fn fetch_provider_balance(
    provider: ApiProvider,
    api_key: &str,
    base_url: &str,
) -> Option<crate::pricing::BalanceInfo> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return None;
    }
    match balance_api_for(provider)? {
        BalanceApi::DeepSeekUserBalance => fetch_deepseek_user_balance(api_key, base_url).await,
        BalanceApi::OpenRouterCredits => fetch_openrouter_credits(api_key, base_url).await,
        BalanceApi::SiliconFlowUserInfo => {
            fetch_siliconflow_user_info(api_key, base_url, provider).await
        }
    }
}

async fn fetch_deepseek_user_balance(
    api_key: &str,
    base_url: &str,
) -> Option<crate::pricing::BalanceInfo> {
    let url = format!("{}/user/balance", base_url.trim_end_matches('/'));
    let body: crate::pricing::BalanceResponse = balance_get_json(api_key, &url).await?;
    body.balance_infos.into_iter().next()
}

#[derive(serde::Deserialize)]
struct OpenRouterCreditsResponse {
    data: OpenRouterCreditsData,
}

#[derive(serde::Deserialize)]
struct OpenRouterCreditsData {
    total_credits: f64,
    total_usage: f64,
}

fn openrouter_remaining_credits(total_credits: f64, total_usage: f64) -> f64 {
    (total_credits - total_usage).max(0.0)
}

async fn fetch_openrouter_credits(
    api_key: &str,
    base_url: &str,
) -> Option<crate::pricing::BalanceInfo> {
    let url = format!("{}/credits", base_url.trim_end_matches('/'));
    let body: OpenRouterCreditsResponse = balance_get_json(api_key, &url).await?;
    let remaining = openrouter_remaining_credits(body.data.total_credits, body.data.total_usage);
    Some(crate::pricing::BalanceInfo {
        currency: "USD".to_string(),
        total_balance: format!("{remaining:.2}"),
        topped_up_balance: format!("{:.2}", body.data.total_credits),
        granted_balance: String::new(),
    })
}

#[derive(serde::Deserialize)]
struct SiliconFlowUserInfo {
    data: Option<SiliconFlowUserData>,
}

#[derive(serde::Deserialize)]
struct SiliconFlowUserData {
    #[serde(default, alias = "totalBalance")]
    total_balance: Option<String>,
    #[serde(default, alias = "chargeBalance")]
    charge_balance: Option<String>,
    #[serde(default)]
    balance: Option<String>,
}

async fn fetch_siliconflow_user_info(
    api_key: &str,
    base_url: &str,
    provider: ApiProvider,
) -> Option<crate::pricing::BalanceInfo> {
    let url = format!("{}/user/info", base_url.trim_end_matches('/'));
    let body: SiliconFlowUserInfo = balance_get_json(api_key, &url).await?;
    let data = body.data?;
    let total = data
        .total_balance
        .as_deref()
        .or(data.balance.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let currency = if provider == ApiProvider::SiliconflowCn {
        "CNY"
    } else {
        "USD"
    };
    Some(crate::pricing::BalanceInfo {
        currency: currency.to_string(),
        total_balance: total.to_string(),
        topped_up_balance: data.charge_balance.unwrap_or_default(),
        granted_balance: String::new(),
    })
}

async fn balance_get_json<T: serde::de::DeserializeOwned>(api_key: &str, url: &str) -> Option<T> {
    let client = &*BALANCE_CLIENT;
    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        tracing::debug!(
            "balance API returned {}: {}",
            response.status().as_u16(),
            response.text().await.unwrap_or_default()
        );
        return None;
    }
    response.json().await.ok()
}

pub(crate) fn should_fetch_provider_balance(app: &App) -> bool {
    app.status_items.contains(&StatusItem::Balance)
        && crate::config::provider_has_balance_api(app.api_provider)
}

/// Kick a background remaining-credit fetch for the live route.
///
/// `force` skips the status-item gate (used by `/balance`). Providers without
/// a known endpoint clear the parked chip so a previous route cannot linger.
pub(crate) fn schedule_balance_fetch(app: &mut App, api_key: &str, base_url: &str, force: bool) {
    if !crate::config::provider_has_balance_api(app.api_provider) {
        if let Ok(mut guard) = app.balance_cell.lock() {
            *guard = None;
        }
        return;
    }
    if !force && !should_fetch_provider_balance(app) {
        return;
    }
    if api_key.trim().is_empty() {
        return;
    }
    let cooldown_ok = force
        || app
            .last_balance_fetch
            .is_none_or(|t| t.elapsed() >= BALANCE_FETCH_COOLDOWN);
    if !cooldown_ok {
        return;
    }
    app.last_balance_fetch = Some(Instant::now());
    let cell = app.balance_cell.clone();
    let provider = app.api_provider;
    let api_key = api_key.to_string();
    let base_url = base_url.to_string();
    tokio::spawn(async move {
        if let Some(info) = fetch_provider_balance(provider, &api_key, &base_url).await
            && let Ok(mut guard) = cell.lock()
        {
            *guard = Some(info);
        }
    });
}

#[cfg(test)]
pub(crate) fn openrouter_credits_from_json(json: &str) -> Option<crate::pricing::BalanceInfo> {
    let body: OpenRouterCreditsResponse = serde_json::from_str(json).ok()?;
    let remaining = openrouter_remaining_credits(body.data.total_credits, body.data.total_usage);
    Some(crate::pricing::BalanceInfo {
        currency: "USD".to_string(),
        total_balance: format!("{remaining:.2}"),
        topped_up_balance: format!("{:.2}", body.data.total_credits),
        granted_balance: String::new(),
    })
}

/// Route text from either clipboard transport into the canonical provider
/// picker. Keeping this small seam pure lets tests exercise ordinary
/// Cmd/Ctrl+V without reading the developer's real clipboard.
pub(crate) fn paste_text_into_provider_picker(app: &mut App, text: &str) -> bool {
    if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
        return false;
    }
    let _ = app.view_stack.handle_paste(text);
    true
}

/// Read an ordinary Cmd/Ctrl+V clipboard shortcut for the provider picker.
/// Images are deliberately consumed but ignored: an open credential modal
/// must never leak unsupported clipboard content into the composer beneath it.
pub(crate) fn paste_provider_picker_from_clipboard(app: &mut App) -> bool {
    if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
        return false;
    }
    if app.clipboard.requires_terminal_paste() {
        app.status_message = Some(app.tr(MessageId::ClipboardSshPasteHint).into_owned());
        return true;
    }
    if let Some(ClipboardContent::Text(text)) = app.clipboard.read(app.workspace.as_path()) {
        let _ = paste_text_into_provider_picker(app, &text);
    }
    true
}

pub(crate) async fn fetch_available_models(config: &Config) -> Result<Vec<String>> {
    use crate::client::DeepSeekClient;

    let client = DeepSeekClient::new(config)?;
    let models = tokio::time::timeout(Duration::from_secs(20), client.list_models()).await??;
    let mut ids = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

pub(crate) fn resolve_cache_replay_route(
    app: &App,
    config: &Config,
) -> Result<crate::route_runtime::ResolvedRuntimeRoute> {
    let target = app.cache_replay_target().ok_or_else(|| {
        anyhow::anyhow!("Auto has no concrete route yet; send a turn before warming its cache")
    })?;
    let identity = config
        .resolve_persisted_provider_identity(
            Some(target.provider.as_str()),
            target.provider_id.as_deref(),
        )
        .map_err(anyhow::Error::msg)?;
    if identity.provider != target.provider || identity.key != target.provider_identity {
        anyhow::bail!(
            "saved cache route identity `{}` now resolves as {}/{} instead of {}/{}; send a new turn before warming",
            target.provider_identity,
            identity.provider.as_str(),
            identity.key,
            target.provider.as_str(),
            target.provider_identity
        );
    }
    let route = resolve_runtime_route_for_identity(config, &identity, Some(&target.model))
        .map_err(anyhow::Error::msg)?;
    if let Some(previous_base_url) = target.base_url.as_deref() {
        let previous_endpoint = crate::route_receipt::endpoint_identity(previous_base_url);
        let current_endpoint =
            crate::route_receipt::endpoint_identity(&route.candidate.endpoint().base_url);
        if previous_endpoint != current_endpoint {
            anyhow::bail!(
                "the cache route endpoint changed since the last turn; send a new turn before warming"
            );
        }
    }
    Ok(route)
}

pub(crate) fn error_health_route(
    app: &App,
    fallback_provider: ApiProvider,
) -> (ApiProvider, String) {
    app.active_turn
        .as_ref()
        .and_then(|turn| turn.route.as_ref())
        .map(|route| (route.provider, route.model.clone()))
        .or_else(|| {
            app.pending_turn_route
                .as_ref()
                .map(|(provider, model, _)| (*provider, model.clone()))
        })
        .unwrap_or_else(|| (fallback_provider, app.model.clone()))
}

pub(crate) fn rollback_provider_after_auth_failure(
    app: &mut App,
    config: &mut Config,
) -> Option<String> {
    let pending = app.pending_provider_switch.take()?;
    let PendingProviderSwitch {
        previous_provider,
        previous_model,
        previous_model_ids_passthrough,
        previous_route_limits,
        previous_route_base_url,
        previous_context_window_source,
        previous_context_window_override,
        previous_config,
        previous_onboarding,
        previous_onboarding_needs_api_key,
        previous_api_key_env_only,
    } = pending;

    *config = previous_config;
    if let Ok(identity) = config.active_provider_identity(previous_provider) {
        app.set_provider_identity_record(identity);
    } else {
        app.set_provider_identity(
            previous_provider,
            config.provider_identity_for(previous_provider),
        );
    }
    app.billing_presentation = crate::route_billing::for_route(config, previous_provider);
    app.set_model_selection(previous_model.clone());
    app.provider_models.insert(
        app.provider_identity_for_persistence().to_string(),
        previous_model,
    );
    // The rolled-back switch leaves the session where it started: any pending
    // route-save decision belongs to the failed provider and must not linger.
    app.pending_route_save = None;
    app.model_ids_passthrough = previous_model_ids_passthrough;
    app.active_context_window_override = previous_context_window_override;
    app.active_route_limits = previous_route_limits;
    app.active_route_base_url = previous_route_base_url;
    app.active_context_window_source = previous_context_window_source;
    app.update_model_compaction_budget();
    app.clear_model_scoped_telemetry();
    app.offline_mode = false;
    app.onboarding = previous_onboarding;
    app.onboarding_needs_api_key = previous_onboarding_needs_api_key;
    app.api_key_env_only = previous_api_key_env_only;

    // The failed switch never wrote config or settings, so the rollback has
    // nothing to undo on disk — and it must not leave a pending save decision
    // behind (cleared above). Only the on-screen setup-state receipt is
    // corrected so the record matches reality.
    let mut persistence_errors = Vec::new();
    if let Err(err) = crate::tui::setup::record_provider_model_setup_state_for_app(app, config) {
        persistence_errors.push(format!("setup state was not saved: {err}"));
    }
    let persistence_error = if persistence_errors.is_empty() {
        None
    } else {
        Some(format!(
            "provider rollback not fully persisted: {}",
            persistence_errors.join("; ")
        ))
    };

    Some(match persistence_error {
        Some(warning) => format!(
            "Provider switch failed and has been rolled back to {}. {}",
            previous_provider.as_str(),
            warning
        ),
        None => format!(
            "Provider switch failed and has been rolled back to {}.",
            previous_provider.as_str()
        ),
    })
}

pub(crate) fn validated_app_runtime_route(
    app: &App,
    config: &Config,
) -> Result<crate::route_runtime::ValidatedRuntimeRoute, String> {
    let (identity, scoped) = app_scoped_runtime_config(app, config);
    resolve_runtime_route_for_identity(&scoped, &identity, Some(&app.model))?.validate()
}

pub(crate) fn compaction_for_validated_route(
    app: &App,
    route: &crate::route_runtime::ValidatedRuntimeRoute,
) -> crate::compaction::CompactionConfig {
    let mut config = app.compaction_config_for_route(
        route.identity.provider,
        &route.model,
        crate::route_budget::known_route_limits(route.candidate.limits()),
    );
    config.image_input = route.candidate.capabilities().image_input;
    config
}

pub(crate) fn validated_profile_default_route(
    config: &Config,
) -> Result<crate::route_runtime::ValidatedRuntimeRoute> {
    let provider = config.api_provider();
    let model = config.default_model();
    resolve_runtime_route(config, provider, Some(&model))
        .and_then(crate::route_runtime::ResolvedRuntimeRoute::validate)
        .map_err(anyhow::Error::msg)
}

pub(crate) fn reasoning_effort_receipt_for_route(
    tier: ReasoningEffort,
    provider: ApiProvider,
    endpoint_identity: &str,
    model: &str,
) -> EffectiveReasoningEffort {
    crate::work_graph::constrained_effective_reasoning_for_route(
        tier.into(),
        provider,
        endpoint_identity,
        model,
    )
    .map(Into::into)
    .unwrap_or(EffectiveReasoningEffort::Tier(tier))
}

pub(crate) async fn sync_mode_update(app: &App, engine_handle: &EngineHandle) {
    let _ = engine_handle
        .send(Op::ChangeMode {
            mode: app.mode,
            allow_shell: app.allow_shell,
            trust_mode: app.trust_mode,
            auto_approve: app_auto_approve_enabled(app),
            approval_mode: app.approval_mode,
            configured_sandbox_mode: app.configured_sandbox_mode.clone(),
        })
        .await;
}

/// Apply a `/provider` switch by resolving a complete route candidate before
/// mutating state, then respawning the engine so the API client picks up the
/// new base URL/key. When `model_override` is set, it replaces the active
/// model post-switch after provider-scoped normalization.
pub(crate) async fn switch_provider(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    target: ApiProvider,
    model_override: Option<String>,
) -> bool {
    let previous_provider = app.api_provider;
    let previous_identity = app.provider_identity_for_persistence().to_string();
    let requested_identity = config.provider_identity_for(target);
    let previous_model = app.model.clone();
    let previous_model_ids_passthrough = app.model_ids_passthrough;
    let mut previous_config = config.clone();
    previous_config.provider = Some(previous_identity.clone());
    app.pending_provider_switch = Some(PendingProviderSwitch {
        previous_provider,
        previous_model: previous_model.clone(),
        previous_model_ids_passthrough,
        previous_route_limits: app.active_route_limits,
        previous_route_base_url: app.active_route_base_url.clone(),
        previous_context_window_source: app.active_context_window_source,
        previous_context_window_override: app.active_context_window_override,
        previous_config: previous_config.clone(),
        previous_onboarding: app.onboarding,
        previous_onboarding_needs_api_key: app.onboarding_needs_api_key,
        previous_api_key_env_only: app.api_key_env_only,
    });

    let resolved_route = match resolve_runtime_route(config, target, model_override.as_deref()) {
        Ok(route) => route,
        Err(reason) => {
            app.pending_provider_switch = None;
            // #3830: if the switch failed only because the target provider has
            // no key or local runtime, hand off to /provider already focused
            // on that provider's key prompt instead of dead-ending with an
            // error the user has to translate into an action.
            if !crate::config::has_api_key_for(config, target)
                && app.view_stack.top_kind() != Some(ModalKind::ProviderPicker)
            {
                let runtime_status = query_provider_runtime_status(engine_handle).await;
                if let Some(picker) =
                    crate::tui::provider_picker::ProviderPickerView::new_for_missing_auth(
                        previous_provider,
                        target,
                        config,
                        runtime_status,
                    )
                    .map(|picker| {
                        picker
                            .with_locale(app.ui_locale)
                            .with_provider_health(&app.provider_health)
                    })
                {
                    *config = previous_config;
                    app.view_stack.push(picker);
                    app.status_message = Some(format!(
                        "{} needs a key or local runtime — enter one to switch.",
                        target.display_name()
                    ));
                    app.needs_redraw = true;
                    return false;
                }
            }
            *config = previous_config;
            app.add_message(HistoryCell::System {
                content: format!(
                    "Cannot switch to {}: {reason}\nProvider unchanged ({}).",
                    requested_identity, previous_identity
                ),
            });
            app.status_message = Some(format!(
                "Route rejected before provider switch: {}.",
                target.as_str()
            ));
            return false;
        }
    };
    let validated_route = match resolved_route.validate() {
        Ok(route) => route,
        Err(err) => {
            app.pending_provider_switch = None;
            *config = previous_config;
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to switch provider to {}: {err}\nProvider unchanged ({}).",
                    requested_identity, previous_identity
                ),
            });
            return false;
        }
    };
    let target_identity_record = validated_route.identity.clone();
    let target_identity = target_identity_record.key.clone();
    let resolved_endpoint = validated_route.candidate.endpoint().base_url.clone();
    let route_limits = validated_route.candidate.limits();
    let context_window_source = validated_route.context_window.source;
    let new_model = validated_route.model.clone();
    *config = *validated_route.config;

    let new_base_url = resolved_endpoint;
    let new_endpoint = display_base_url_host(&new_base_url);
    let cache_scope_changed = previous_provider != target
        || previous_identity != target_identity
        || previous_model != new_model;
    app.set_provider_identity_record(target_identity_record);
    app.billing_presentation = crate::route_billing::for_route(config, target);
    app.max_subagents = config
        .max_subagents_for_provider(target)
        .clamp(1, crate::config::MAX_SUBAGENTS);
    app.provider_chain = target
        .kind()
        .map(|kind| codewhale_config::ProviderChain::new(kind, &config.fallback_providers))
        .filter(|chain| chain.providers().len() > 1);
    app.last_fallback_reason = None;
    app.model_ids_passthrough = config.model_ids_pass_through();
    app.set_model_selection(new_model.clone());
    app.apply_provider_switch_reasoning_effort(target, &new_base_url, model_override.as_deref());
    app.set_active_context_window_override(config.context_window_for_provider_config(target));
    app.set_active_route_resolution(new_base_url.clone(), route_limits, context_window_source);
    if model_override.is_some() {
        app.provider_models
            .insert(target_identity.clone(), new_model.clone());
        app.enable_provider_model(&target_identity, &new_model);
    }
    app.update_model_compaction_budget();
    if cache_scope_changed {
        app.clear_model_scoped_telemetry();
    } else {
        app.session.last_prompt_tokens = None;
        app.session.last_completion_tokens = None;
        app.session.last_output_throughput = None;
    }

    let _ = engine_handle.send(Op::Shutdown).await;
    let engine_config = build_engine_config(app, config);
    *engine_handle = spawn_tui_engine(engine_config, config);
    // A successful in-session switch must refresh the same key-scoped live
    // catalog as startup. TelecomJS is currently the only provider using this
    // seam; failures preserve the existing/static rows.
    crate::client::DeepSeekClient::spawn_active_provider_catalog_refresh(config);

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: app.workspace.clone(),
                mode: app.mode,
            })
            .await;
    }
    let _ = engine_handle
        .send(Op::SetCompaction {
            config: app.compaction_config(),
        })
        .await;

    // Route changes are temporary by default: nothing is written here. The
    // route-save prompt offers the explicit persistence choices, so a
    // workspace's config file can never be silently rewritten by a switch
    // made in another folder.
    app.note_session_route_change(&target_identity, &new_model);
    let persist_warning: Option<String> = None;

    let mut switch_summary = format!(
        "Provider switched: {} → {}",
        previous_identity, target_identity,
    );
    switch_summary.push(char::from(10));
    switch_summary.push_str(&format!("Model: {previous_model} → {new_model}"));
    switch_summary.push(char::from(10));
    switch_summary.push_str(&format!("Endpoint: {new_endpoint}"));
    if let Some(ref warning) = persist_warning {
        switch_summary.push(char::from(10));
        switch_summary.push_str(warning);
    }
    app.add_message(HistoryCell::System {
        content: switch_summary,
    });

    let mut status_message = format!("Provider: {target_identity} via {new_endpoint}");
    let persisted = persist_warning.is_none();
    if persist_warning.is_some() {
        status_message.push_str(" (not fully persisted)");
    }
    app.status_message = Some(status_message);
    // #3927: activating a route is the single event that retires the
    // explore-offline label. Nothing time-based or screen-based clears it.
    onboarding::clear_offline_explore_on_route_activation(app);
    if persisted {
        record_provider_model_setup_progress(app, config);
    }
    true
}

pub(crate) fn display_base_url_host(base_url: &str) -> String {
    let without_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    without_scheme
        .split('/')
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or(base_url)
        .to_string()
}

pub(crate) fn sync_config_provider_from_app(config: &mut Config, app: &App) {
    config.provider = Some(app.provider_identity_for_persistence().to_string());
}

pub(crate) fn provider_picker_model_override(
    app: &App,
    config: &Config,
    provider: ApiProvider,
) -> Option<String> {
    (app.api_provider == provider
        && app.provider_identity_for_persistence() == config.provider_identity_for(provider))
    .then(|| app.model.clone())
}

pub(crate) async fn query_provider_runtime_status(
    engine_handle: &EngineHandle,
) -> Option<ProviderRuntimeStatus> {
    tokio::time::timeout(
        Duration::from_millis(100),
        engine_handle.get_provider_runtime_status(),
    )
    .await
    .ok()
    .and_then(|result| result.ok())
}

pub(crate) fn mcp_reload_summary(snapshot: &crate::mcp::McpManagerSnapshot) -> String {
    let connected = snapshot
        .servers
        .iter()
        .filter(|server| server.connected)
        .count();
    let failed = snapshot
        .servers
        .iter()
        .filter(|server| server.enabled && server.error.is_some())
        .count();
    let disabled = snapshot
        .servers
        .iter()
        .filter(|server| !server.enabled)
        .count();
    format!(
        "MCP tool pool reloaded in process: {connected} connected, {failed} failed, {disabled} disabled. The next model turn uses this catalog."
    )
}

pub(crate) fn mcp_ui_action_refreshes_discovery(action: &crate::tui::app::McpUiAction) -> bool {
    matches!(
        action,
        crate::tui::app::McpUiAction::Validate
            | crate::tui::app::McpUiAction::Login { .. }
            | crate::tui::app::McpUiAction::Logout { .. }
            | crate::tui::app::McpUiAction::ImportList
            | crate::tui::app::McpUiAction::ImportApprove { .. }
    )
}

pub(crate) fn mcp_import_consent_path() -> PathBuf {
    codewhale_config::codewhale_home()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("mcp-import-consent.json")
}

pub(crate) fn mcp_external_import_status_text(workspace: &std::path::Path) -> String {
    use crate::mcp::external_import::{discover_external_sources, format_candidates_for_display};
    let home = crate::config::effective_home_dir().unwrap_or_else(|| PathBuf::from("."));
    let market_path = codewhale_config::codewhale_home()
        .ok()
        .map(|h| h.join("mcp-marketplace.json"));
    let markets: Vec<PathBuf> = market_path.into_iter().collect();
    let all = discover_external_sources(&home, workspace, &markets);
    let mut body = format_candidates_for_display(&all);
    body.push_str("\n\nConfigured managed connectors stay in your mcp.json; external sources never auto-merge.");
    body
}

pub(crate) fn mcp_import_apply(
    workspace: &std::path::Path,
    mcp_path: &std::path::Path,
    name: &str,
    approve: bool,
) -> anyhow::Result<String> {
    use crate::mcp::external_import::{
        ImportDecision, apply_approved, discover_external_sources, load_consent_store,
        merge_approved_into_config, record_decisions, save_consent_store,
    };
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    let home = crate::config::effective_home_dir().unwrap_or_else(|| PathBuf::from("."));
    let market_path = codewhale_config::codewhale_home()
        .ok()
        .map(|h| h.join("mcp-marketplace.json"));
    let markets: Vec<PathBuf> = market_path.into_iter().collect();
    let all = discover_external_sources(&home, workspace, &markets);
    let candidate = all
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No external MCP candidate named '{name}'. Run /mcp import to list sources with provenance."
            )
        })?;

    if approve && candidate.hard_blocked {
        anyhow::bail!(
            "Refusing to import '{}': {} (enabled=false is a hard block)",
            candidate.name,
            candidate.block_reason.as_deref().unwrap_or("hard blocked")
        );
    }

    let mut decisions = HashMap::new();
    decisions.insert(
        candidate.name.clone(),
        if approve {
            ImportDecision::Approve
        } else {
            ImportDecision::Decline
        },
    );

    let mut store = load_consent_store(&mcp_import_consent_path());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    record_decisions(&mut store, std::slice::from_ref(candidate), &decisions, now);
    save_consent_store(&mcp_import_consent_path(), &store)?;

    if !approve {
        return Ok(format!(
            "Declined external MCP '{}' from {} (hash {}). Will not re-prompt until the source content changes.",
            candidate.name,
            candidate.source_path.display(),
            &candidate.content_hash[..12.min(candidate.content_hash.len())]
        ));
    }

    let approved = apply_approved(std::slice::from_ref(candidate), &decisions);
    let mut cfg = crate::mcp::load_config(mcp_path)?;
    let inserted = merge_approved_into_config(&mut cfg, &approved);
    if inserted.is_empty() {
        return Ok(format!(
            "MCP '{}' was already present in {} or could not be merged. Provenance: {} @ {}",
            candidate.name,
            mcp_path.display(),
            candidate.source_kind.as_str(),
            candidate.source_path.display()
        ));
    }
    crate::mcp::save_config(mcp_path, &cfg)?;
    Ok(format!(
        "Imported managed MCP connector '{}' into {} (provenance: {} @ {}, hash {}). Run /mcp reload to connect after review.",
        candidate.name,
        mcp_path.display(),
        candidate.source_kind.as_str(),
        candidate.source_path.display(),
        &candidate.content_hash[..12.min(candidate.content_hash.len())]
    ))
}

pub(crate) fn clear_active_provider_api_key_from_memory(app: &App, config: &mut Config) {
    let active_identity = app.provider_identity_for_persistence();
    let clears_legacy_root = matches!(
        app.api_provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN
    ) || (app.api_provider == ApiProvider::Custom
        && active_identity == ApiProvider::Custom.as_str()
        && config.uses_legacy_literal_custom_route());
    if clears_legacy_root {
        config.api_key = None;
    }
    config.set_provider_api_key_override(app.api_provider, None);
    if app.api_provider == ApiProvider::Xai {
        let entry = config.provider_config_for_mut(ApiProvider::Xai);
        entry.auth_mode = None;
        entry.oauth_credential_generation = None;
        entry.external_credentials = None;
    }
}

pub(crate) fn record_provider_model_setup_progress(app: &mut App, config: &Config) {
    if let Err(err) = crate::tui::setup::record_provider_model_setup_state_for_app(app, config) {
        let note = format!("Setup provider/model state was not saved: {err}");
        if let Some(status) = app.status_message.as_mut() {
            status.push_str(" · ");
            status.push_str(&note);
        } else {
            app.status_message = Some(note.clone());
        }
        app.add_message(HistoryCell::System { content: note });
    }
}

/// Persist the typed API key to `~/.codewhale/config.toml`, refresh the
/// in-memory config so the engine can see it, then switch to the provider.
pub(crate) fn set_active_custom_provider_in_memory(config: &mut Config, provider_id: &str) {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return;
    }
    config.provider = Some(provider_id.to_string());
    config
        .providers
        .get_or_insert_with(ProvidersConfig::default)
        .custom
        .entry(provider_id.to_string())
        .or_default();
}

pub(crate) fn picker_provider_identity(
    config: &Config,
    provider: ApiProvider,
    provider_id: Option<&str>,
) -> Result<crate::config::ProviderIdentity, String> {
    let identity = match provider_id {
        Some(provider_id) => config
            .resolve_persisted_provider_identity(Some(provider.as_str()), Some(provider_id))?,
        None if provider == ApiProvider::Custom => config.active_provider_identity(provider)?,
        None => config.resolve_persisted_provider_identity(
            Some(provider.as_str()),
            Some(provider.as_str()),
        )?,
    };
    if identity.provider != provider {
        return Err(format!(
            "provider picker identity '{}' resolved as {}, not {}",
            identity.key,
            identity.provider.as_str(),
            provider.as_str()
        ));
    }
    Ok(identity)
}

pub(crate) fn provider_verification_error_category(
    reason: &str,
) -> crate::error_taxonomy::ErrorCategory {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("http 401") || lower.contains("status 401") {
        crate::error_taxonomy::ErrorCategory::Authentication
    } else if lower.contains("http 403") || lower.contains("status 403") {
        crate::error_taxonomy::ErrorCategory::Authorization
    } else if ["500", "502", "503", "504"]
        .iter()
        .any(|status| lower.contains(&format!("http {status}")))
    {
        crate::error_taxonomy::ErrorCategory::Network
    } else {
        crate::error_taxonomy::classify_error_message(reason)
    }
}
