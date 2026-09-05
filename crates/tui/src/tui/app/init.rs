//! Application initialization: `App` construction lives here so the central
//! `app.rs` module holds state and behavior rather than a ~830-line
//! constructor. `App::new` remains a thin test-only shim over
//! [`App::new_with_plugin_registry`]; all callers construct `App` exactly as
//! before.

use super::*;

impl App {
    #[cfg(test)]
    pub fn new(options: TuiOptions, config: &Config) -> Self {
        let workspace = options.workspace.clone();
        Self::new_with_plugin_registry(
            options,
            config,
            std::sync::Arc::new(crate::plugins::PluginRegistry::empty(&workspace)),
        )
    }

    #[allow(clippy::too_many_lines)]
    pub fn new_with_plugin_registry(
        options: TuiOptions,
        config: &Config,
        plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
    ) -> Self {
        let TuiOptions {
            model,
            workspace,
            config_path,
            config_profile,
            allow_shell,
            screen_mode,
            use_mouse_capture,
            mouse_capture_preference,
            use_bracketed_paste,
            max_subagents,
            skills_dir: global_skills_dir,
            memory_path,
            notes_path: _,
            mcp_config_path,
            use_memory,
            start_in_agent_mode,
            skip_onboarding,
            yolo,
            resume_session_id,
            initial_input,
            // Consumed by `run_app` after the App exists, so it can be shown
            // alongside (or instead of) the resume receipt.
            startup_notice: _,
        } = options;

        // Start from disk-only preferences so one-time migrations can never
        // persist terminal/environment overlays such as NO_ANIMATIONS. Apply
        // those overlays only after any normalized settings write succeeds.
        let mut settings = Settings::load_persisted().unwrap_or_else(|_| Settings::default());
        let legacy_yolo_default = settings.legacy_yolo_default_detected();
        let legacy_yolo_full_access = if legacy_yolo_default {
            let control = config.approval_policy_control(
                config_path.as_deref(),
                config_profile.as_deref(),
                &workspace,
            );
            match control {
                crate::config::ApprovalPolicyControl::Unset => {
                    if let Err(error) = normalize_legacy_yolo_settings() {
                        tracing::warn!(
                            "failed to normalize legacy YOLO settings; retrying next launch: {error:#}"
                        );
                    }
                    true
                }
                crate::config::ApprovalPolicyControl::RootConfig => {
                    let active_config_path = match crate::config::resolve_load_config_path(
                        config_path.clone(),
                    ) {
                        Ok(path) => path,
                        Err(error) => {
                            tracing::error!(
                                error = %error,
                                "could not resolve the active config path for legacy policy migration"
                            );
                            None
                        }
                    };
                    match crate::config_persistence::persist_unset_root_key(
                        active_config_path.as_deref(),
                        "approval_policy",
                    ) {
                        Ok(_) => {
                            if let Err(error) = normalize_legacy_yolo_settings() {
                                tracing::warn!(
                                    "removed legacy approval_policy but could not normalize settings; retrying next launch: {error:#}"
                                );
                            }
                            true
                        }
                        Err(error) => {
                            tracing::warn!(
                                "could not migrate legacy YOLO approval policy; keeping the controlling policy: {error:#}"
                            );
                            false
                        }
                    }
                }
                source => {
                    tracing::warn!(
                        "legacy YOLO setting was not allowed to override {}",
                        source.label()
                    );
                    false
                }
            }
        } else {
            false
        };
        settings.apply_env_overrides();
        // Tideline Startup is the fresh interactive landing surface. It must
        // not be bypassed by a stale historical `launch_screen = false`, a
        // provider/config notice, or a previous session record: only an
        // intentional resume or explicit initial input enters the live session
        // path directly.
        let launch_visible = resume_session_id.is_none() && initial_input.is_none();
        let launch = LaunchState::new(launch_visible, &workspace);

        // If settings.toml exists on disk but couldn't be parsed (we fell back
        // to defaults), surface a warning in the TUI so the user knows their
        // file is broken instead of silently losing all settings.
        let settings_parse_warning = crate::settings::Settings::path().ok().and_then(|p| {
            if p.exists() {
                std::fs::read_to_string(&p).ok().and_then(|raw| {
                    ::toml::from_str::<::toml::Value>(&raw)
                        .err()
                        .map(|e| format!("⚠ settings.toml is malformed — using defaults ({e})"))
                })
            } else {
                None
            }
        });
        let tui_prefs_warning = crate::settings::TuiPrefs::path().ok().and_then(|p| {
            if p.exists() {
                std::fs::read_to_string(&p).ok().and_then(|raw| {
                    ::toml::from_str::<::toml::Value>(&raw)
                        .err()
                        .map(|e| format!("⚠ tui.toml is malformed — using defaults ({e})"))
                })
            } else {
                None
            }
        });

        let mut provider = config.api_provider();

        // A startup route saved explicitly from `/model` is a user choice and
        // must win over a provider merely seeded in config.toml. A one-launch
        // CLI/environment provider override still wins so scripts can pin
        // their route without changing the user's next interactive launch.
        let explicit_launch_provider = crate::config::explicit_launch_provider_override().is_some();
        let mut provider_identity_record = config
            .active_provider_identity(provider)
            .unwrap_or_else(|_| {
                let key = config.provider_identity_for(provider);
                let exact_id = (!(provider == ApiProvider::Custom
                    && config.uses_legacy_literal_custom_route()))
                .then(|| key.clone());
                crate::config::ProviderIdentity {
                    provider,
                    key,
                    exact_id,
                    migrated_legacy_ollama_cloud_route: false,
                }
            });
        if !explicit_launch_provider
            && !config.fleet_operator_route_applied
            && let Some(ref provider_str) = settings.default_provider
            && let Ok(resolved) = config.resolve_provider_identity(provider_str)
        {
            provider = resolved.provider;
            provider_identity_record = resolved;
        }
        let mut effective_auth_config = config.clone();
        effective_auth_config.scope_to_provider_identity(&provider_identity_record);
        let provider_identity = provider_identity_record.key;
        let provider_exact_id = provider_identity_record.exact_id;

        // #5032: a stale `[providers.xai] oauth_credential_generation` pointer
        // whose owned credential file is gone makes `credentials_valid` return
        // false with no recovery, so the generic provider picker reopened on
        // EVERY launch (the dogfood bricked state). Detect that specific
        // corrupted state, best-effort clear the stale pointer from the
        // persisted config, and surface a truthful xAI-specific message. The
        // repair never blocks or aborts launch; after it the state is the
        // normal "needs auth", not a bricked loop.
        // #5032: an onboarded user whose active xAI OAuth credential is missing
        // must be guided to re-authenticate THAT provider — not be re-run through
        // the generic provider picker on every launch. Detect the missing-cred
        // state (broader than a dangling pointer: it also covers a repaired
        // pointer, an expired/revoked token, or a never-completed login), repair
        // a stale pointer once, surface a truthful xAI message, and suppress the
        // picker-recovery path below.
        let xai_oauth_needs_reauth = provider == ApiProvider::Xai
            && effective_auth_config
                .provider_config_for(ApiProvider::Xai)
                .and_then(|entry| entry.auth_mode.as_deref())
                .is_some_and(crate::xai_oauth::auth_mode_uses_xai_oauth)
            && !crate::xai_oauth::credentials_present(&effective_auth_config);
        let xai_dangling_repair_message = if xai_oauth_needs_reauth {
            if crate::xai_oauth::owned_generation_is_dangling(&effective_auth_config) {
                match crate::xai_oauth::clear_dangling_xai_oauth_generation(config_path.as_deref())
                {
                    Ok(()) => {
                        // Keep the in-memory route consistent with the repaired
                        // persisted file so the running app never reaches for
                        // the missing generation.
                        effective_auth_config
                            .provider_config_for_mut(ApiProvider::Xai)
                            .oauth_credential_generation = None;
                    }
                    Err(error) => {
                        tracing::warn!(
                            target: "codewhale::xai_oauth",
                            error = %error,
                            "could not clear the dangling xAI OAuth generation pointer; continuing launch"
                        );
                    }
                }
            }
            Some(
                "⚠ xAI OAuth credentials are missing. Re-authenticate with \
                 `codewhale auth xai-device` or the in-app login, or switch providers."
                    .to_string(),
            )
        } else {
            None
        };
        let model_ids_passthrough = effective_auth_config.model_ids_pass_through();
        let provider_chain = provider
            .kind()
            .map(|kind| ProviderChain::new(kind, &config.fallback_providers))
            .filter(|chain| chain.providers().len() > 1);

        // Snapshot per-provider readiness for the fallback chain (#2574). Uses
        // the same `has_api_key_for` helper the provider picker uses, so hosted
        // providers require a key and self-hosted ones (Ollama/vLLM/SGLang) are
        // reported ready without one. Empty when there is no fallback chain.
        let provider_readiness = provider_chain
            .as_ref()
            .map(|chain| {
                chain
                    .providers()
                    .iter()
                    .map(|kind| {
                        let provider = ApiProvider::from_kind(*kind);
                        (provider, has_api_key_for(config, provider))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Check if the effective provider has an API key. This must happen
        // after settings.default_provider is applied; otherwise a saved
        // third-party provider can be pushed back into DeepSeek onboarding.
        let needs_api_key = !has_api_key(&effective_auth_config);
        let api_key_env_only =
            crate::config::active_provider_uses_env_only_api_key(&effective_auth_config);
        let was_onboarded = crate::tui::onboarding::is_onboarded();
        let settings_auto_compact = settings.auto_compact;
        let auto_compact_user_configured = Settings::auto_compact_explicitly_configured();
        let auto_compact_threshold_percent = settings.auto_compact_threshold_percent;
        let calm_mode = settings.calm_mode;
        let low_motion = settings.low_motion;
        let constrained_frame_rate = settings.constrained_frame_rate;
        let fancy_animations = settings.fancy_animations;
        let focus_texture =
            crate::tui::focus_texture::FocusTextureMode::parse(&settings.focus_texture)
                .unwrap_or_default();
        let work_surface_placement =
            crate::tui::work_surface::WorkSurfacePlacement::parse(&settings.work_surface_placement);
        let work_surface_top_height = settings.work_surface_top_height;
        let work_surface_side_width = settings.work_surface_side_width;
        let synchronized_output_enabled = settings.synchronized_output_enabled();
        let status_indicator = settings.status_indicator.clone();
        let show_thinking = settings.show_thinking;
        let thinking_highlight = settings.thinking_highlight;
        let thinking_default_expanded = settings.thinking_default_expanded;
        let thinking_preview_lines = settings.thinking_preview_lines;
        let help_expand_groups = settings.help_expand_groups;
        let pin_last_prompt = settings.pin_last_prompt;
        let show_tool_details = settings.show_tool_details;
        let inline_diff_mode = InlineDiffMode::parse(&settings.inline_diffs);
        let ui_locale = resolve_locale(&settings.locale);
        let cost_currency = match (settings.cost_currency.as_str(), ui_locale.tag()) {
            ("usd", "zh-Hans") => CostCurrency::Cny,
            _ => CostCurrency::from_setting(&settings.cost_currency).unwrap_or(CostCurrency::Usd),
        };
        let composer_density = ComposerDensity::from_setting(&settings.composer_density);
        let composer_border = settings.composer_border;
        let composer_multiline_mode = settings.composer_multiline_mode;
        let composer_vim_enabled = settings
            .composer_vim_mode
            .trim()
            .eq_ignore_ascii_case("vim");
        let transcript_spacing = TranscriptSpacing::from_setting(&settings.transcript_spacing);
        let max_input_history = settings.max_input_history;
        // The rapid-keystroke heuristic is the fallback for terminals
        // without bracketed paste, not a second guess layered on top of a
        // working one: when bracketed paste is enabled it must stay off, or
        // every fast-typed command goes through hold/buffer windows that
        // scramble the composer (Y-7, 2026-08-31 QA). The setting remains
        // the fallback's switch, honored only when bracketed paste is off.
        let use_paste_burst_detection = settings.paste_burst_detection && !use_bracketed_paste;
        // Resolve the named theme from settings; unknown values were already
        // normalised to the underwater default in Settings::load. The
        // background_color setting still overlays on top.
        let background_color_override = settings
            .background_color
            .as_deref()
            .and_then(palette::parse_hex_rgb_color);
        let background_setting = background_color_override.and_then(palette::hex_rgb_string);
        let resolved_theme =
            palette::resolve_theme_setting(&settings.theme, background_setting.as_deref());
        let theme_warning = resolved_theme.as_ref().err().map(|error| {
            format!(
                "⚠ configured theme '{}' could not be loaded — using System ({error})",
                settings.theme
            )
        });
        let (_, theme_id, ui_theme) = resolved_theme.unwrap_or_else(|_| {
            let id = palette::ThemeId::System;
            let mut theme = id.ui_theme();
            if let Some(background) = background_color_override {
                theme = theme.with_background_color(background);
            }
            (id.name().to_string(), id, theme)
        });
        let provider_models = settings.provider_models.clone().unwrap_or_default();
        // `provider_models` remembers the last `/model` pick per provider. It
        // is a convenience default, not an override: when this launch named a
        // model explicitly (`--model`, forwarded as `CODEWHALE_MODEL`), that
        // request wins. Before this fix the memory won unconditionally, so
        // `codewhale --provider moonshot --model kimi-k3` silently kept running
        // the remembered `kimi-k2.7-code` while `doctor` reported `kimi-k3`.
        let model = if crate::config::explicit_launch_model_override().is_some()
            || config.fleet_operator_route_applied
        {
            model
        } else {
            let configured = model;
            provider_models
                .get(&provider_identity)
                .cloned()
                .or_else(|| {
                    // default_model is a DeepSeek-centric setting; other providers
                    // get their model from config.toml / env (e.g. OPENAI_MODEL).
                    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
                        settings.default_model.clone()
                    } else {
                        None
                    }
                })
                // The remembered pick may be a catalog spelling of the model
                // the config file already names. Case-sensitive self-hosted
                // endpoints reject the wrong spelling, so config.toml wins a
                // case-only disagreement (the selection itself is unchanged).
                .map(|remembered| {
                    crate::config::prefer_configured_model_spelling(&configured, remembered)
                })
                .unwrap_or(configured)
        };
        let auto_model = model.trim().eq_ignore_ascii_case("auto");
        let mut enabled_provider_models = settings.enabled_models.clone().unwrap_or_default();
        for (saved_provider, saved_model) in &provider_models {
            push_enabled_provider_model(&mut enabled_provider_models, saved_provider, saved_model);
        }
        push_enabled_provider_model(&mut enabled_provider_models, &provider_identity, &model);
        let active_context_window_override = config.context_window_for_provider_config(provider);
        let configured_route_base_url = effective_auth_config.deepseek_base_url();
        let (active_route_limits, active_route_base_url, active_context_window_source) =
            if auto_model {
                (
                    active_context_window_override.map(|window| RouteLimits {
                        context_tokens: Some(u64::from(window)),
                        ..RouteLimits::default()
                    }),
                    configured_route_base_url,
                    if active_context_window_override.is_some() {
                        crate::route_runtime::ContextWindowSource::Configured
                    } else {
                        crate::route_runtime::ContextWindowSource::Fallback
                    },
                )
            } else {
                let saved_provider_model = config
                    .provider_config_for(provider)
                    .and_then(|provider| provider.model.as_deref());
                crate::route_runtime::resolve_route_candidate_with_context_metadata(
                    provider,
                    Some(&model),
                    saved_provider_model,
                    Some(configured_route_base_url.clone()),
                    active_context_window_override,
                    None,
                )
                .map(|resolution| {
                    (
                        crate::route_budget::known_route_limits(resolution.candidate.limits()),
                        resolution.candidate.endpoint().base_url.clone(),
                        resolution.context_window.source,
                    )
                })
                .unwrap_or((
                    None,
                    configured_route_base_url,
                    crate::route_runtime::ContextWindowSource::Fallback,
                ))
            };
        let reasoning_effort_explicit = config.fleet_operator_reasoning_applied
            || settings.reasoning_effort.is_some()
            || config.reasoning_effort_is_explicit();
        let configured_reasoning_effort = if config.fleet_operator_reasoning_applied {
            config.reasoning_effort()
        } else {
            settings
                .reasoning_effort
                .as_deref()
                .or_else(|| config.reasoning_effort())
        };
        let reasoning_effort_preference = configured_reasoning_effort
            .filter(|_| reasoning_effort_explicit)
            .map(ReasoningEffort::from_setting);
        let threshold_model = if auto_model {
            DEFAULT_TEXT_MODEL
        } else {
            model.as_str()
        };
        let compact_threshold = crate::route_budget::compaction_threshold_for_route_at_percent(
            provider,
            threshold_model,
            active_route_limits,
            auto_compact_threshold_percent,
        );
        let auto_compact = if auto_compact_user_configured {
            settings_auto_compact
        } else {
            crate::route_budget::auto_compact_default_for_route(
                provider,
                threshold_model,
                active_route_limits,
            )
        };
        let mut reasoning_effort = if auto_model && !reasoning_effort_explicit {
            // A retired fixed-model alias can infer a compatibility effort in
            // Config. That is route metadata, not an explicit user preference,
            // so it must not silently constrain unresolved auto routing.
            ReasoningEffort::Auto
        } else {
            configured_reasoning_effort.map_or_else(
                || {
                    if auto_model {
                        ReasoningEffort::Auto
                    } else {
                        ReasoningEffort::default()
                    }
                },
                |setting| {
                    if auto_model {
                        ReasoningEffort::from_setting(setting)
                    } else {
                        ReasoningEffort::from_setting_for_provider(setting, provider)
                    }
                },
            )
        };
        if !auto_model
            && !reasoning_effort_explicit
            && let Some(effort) = crate::config::legacy_deepseek_alias_effort_for_route(
                provider,
                &effective_auth_config.deepseek_base_url(),
                &model,
            )
        {
            reasoning_effort = ReasoningEffort::from_setting_for_provider(effort, provider);
        }
        if !auto_model
            && crate::config::is_exact_direct_moonshot_k3_route(
                provider,
                &active_route_base_url,
                &model,
            )
        {
            // Keep the visible/effective tier truthful on first launch too;
            // direct K3 cannot honor a persisted `off` setting.
            reasoning_effort =
                reasoning_effort.normalize_for_route(provider, &active_route_base_url, &model);
        } else if !auto_model && !reasoning_effort_explicit {
            if let Some(default) = ReasoningEffort::catalog_default(provider, &model) {
                reasoning_effort = default;
            }
        } else if !auto_model && ReasoningEffort::catalog_effort_values(provider, &model).is_some()
        {
            reasoning_effort =
                reasoning_effort.normalize_for_route(provider, &active_route_base_url, &model);
        }

        // Resolve the saved mode separately from the permission posture.
        let preferred_mode = AppMode::from_setting(&settings.default_mode);
        // Legacy `default_mode = "yolo"` was split into Act plus the
        // full-access posture at the settings edge, so only the CLI flag
        // requests the compat elevation here.
        let yolo_requested = yolo;
        let initial_mode = if yolo_requested || start_in_agent_mode {
            AppMode::Agent
        } else {
            preferred_mode
        };

        // Durable Agent-era permission baseline (#3386). Plan/YOLO derive from
        // and restore to this. When the user starts in YOLO the live shell
        // flag is force-enabled below, so
        // the baseline shell value is taken from the interactive default (the
        // pre-mode Agent surface) rather than the YOLO-forced live mirror;
        // otherwise it mirrors the resolved `allow_shell` option, which already
        // carries that same interactive default. Using `interactive_allow_shell()`
        // here keeps the Agent baseline identical regardless of launch mode, so
        // a YOLO -> Agent downshift exposes shell (approval-gated) exactly as
        // documented, while an explicit `allow_shell = false` still hides it.
        // Trust is never part of the Agent baseline (it is YOLO-only authority).
        // Approval mirrors the configured policy.
        let explicit_approval_mode = (!legacy_yolo_full_access)
            .then_some(config.approval_policy.as_deref())
            .flatten()
            .and_then(ApprovalMode::from_config_value);
        let approval_policy_control = if legacy_yolo_full_access {
            ApprovalPolicyControl::Unset
        } else {
            config.approval_policy_control(
                config_path.as_deref(),
                config_profile.as_deref(),
                &workspace,
            )
        };
        let approval_policy_locked = approval_policy_control != ApprovalPolicyControl::Unset;
        let approval_policy_root_editable =
            approval_policy_control == ApprovalPolicyControl::RootConfig;
        let approval_policy_requirements_managed =
            approval_policy_control == ApprovalPolicyControl::Requirements;
        let shell_access_editable = config
            .allow_shell_control(
                config_path.as_deref(),
                config_profile.as_deref(),
                &workspace,
            )
            .editable_root();
        // YOLO is a permission change. A locked policy must not be sidestepped
        // by --yolo, default_mode=yolo, /zidong, or Alt+Y.
        let yolo_compat = yolo_requested && !approval_policy_locked;
        let needs_workspace_trust = !yolo_compat && crate::tui::onboarding::needs_trust(&workspace);
        // The language screen is required only when the locale cannot be
        // confidently inferred from settings or the environment; returning
        // users never see it.
        let onboarding_needs_language = !was_onboarded
            && !crate::tui::onboarding::locale_confidently_inferred(&settings.locale);
        // Suppress the missing-key provider picker for the xAI-OAuth-missing-
        // credential case: the user already chose xAI and just needs to
        // re-authenticate it, not re-pick a provider every launch.
        let (onboarding, onboarding_missing_key_recovery) = launch_onboarding_decision(
            skip_onboarding,
            was_onboarded,
            onboarding_needs_language,
            needs_api_key,
            needs_workspace_trust,
            xai_oauth_needs_reauth,
        );
        let onboarding_workspace_trust_gate = onboarding_is_workspace_trust_gate(
            skip_onboarding,
            was_onboarded,
            needs_api_key,
            needs_workspace_trust,
        );
        let saved_permission_posture = if approval_policy_locked {
            None
        } else {
            settings
                .permission_posture
                .as_deref()
                .and_then(ApprovalMode::from_config_value)
        };
        let configured_approval_mode = explicit_approval_mode
            .or(saved_permission_posture)
            .unwrap_or_default();
        let configured_trust_mode = configured_approval_mode == ApprovalMode::Bypass;
        let mode_prefs = ModeSessionPrefs {
            agent_allow_shell: if yolo_compat {
                config.interactive_allow_shell()
            } else {
                allow_shell
            },
            agent_trust_mode: configured_trust_mode,
            // The YOLO-compat launch elevates the *live* approval mirror to
            // Bypass below; the durable Agent baseline keeps the configured
            // policy so a YOLO -> Agent downshift restores it.
            agent_approval_mode: configured_approval_mode,
        };
        let allow_shell = if yolo_compat {
            allow_shell || shell_access_editable
        } else {
            allow_shell
        };
        let shell_manager = new_shared_shell_manager(workspace.clone());

        for error in crate::commands::user_registry::install_plugin_registry(
            &workspace,
            plugin_registry.as_ref(),
        ) {
            tracing::warn!(target: "plugins", "{error}");
        }

        // Initialize hooks executor from config, reviewed plugin snapshots,
        // then project-local `.codewhale/hooks.toml` (#3026).
        let hooks_config = crate::hooks::HooksConfig::load_with_project_and_plugins(
            config.hooks_config(),
            &workspace,
            Some(plugin_registry.as_ref()),
        );
        let hooks = HookExecutor::new(hooks_config, workspace.clone());

        // Initialize the lifecycle event outbox (`[lifecycle_outbox]`).
        // Disabled (all emits no-op) when the config has no path.
        let lifecycle_outbox = config
            .lifecycle_outbox
            .as_ref()
            .map(|outbox| {
                codewhale_hooks::LifecycleOutbox::new(
                    outbox.path.clone(),
                    outbox.webhook_url.clone(),
                    outbox.webhook_token.clone(),
                )
            })
            .unwrap_or_else(codewhale_hooks::LifecycleOutbox::disabled);

        // Initialize plan state
        let plan_state = new_shared_plan_state();
        let todos = new_shared_todo_list();
        let work_runtime =
            crate::work_graph::new_shared_work_runtime(todos.clone(), plan_state.clone());

        let skills_scan_codewhale_only = config.skills_config().scan_codewhale_only();
        let skills_dir = resolve_skills_dir(&workspace, &global_skills_dir, config);
        let cached_skills = Self::discover_cached_skills(
            &workspace,
            &skills_dir,
            skills_scan_codewhale_only,
            plugin_registry.as_ref(),
        );

        let input_history = crate::composer_history::load_history();
        let mention_cwd = std::env::current_dir().ok();
        let start_remote_control = matches!(initial_input, Some(InitialInput::RemoteControl));
        let (initial_input_text, initial_input_cursor, auto_submit_initial_input) =
            match initial_input {
                // #451: pre-populate the composer when invoked via
                // `deepseek pr <N>` (or any future caller that wants to
                // drop the model into a session with context already
                // typed). Cursor lands at the end so Enter sends as-is.
                Some(InitialInput::Prefill(text)) if !text.is_empty() => {
                    let cursor = text.chars().count();
                    (text, cursor, false)
                }
                Some(InitialInput::Submit(text)) if !text.is_empty() => {
                    let cursor = text.chars().count();
                    (text, cursor, true)
                }
                Some(InitialInput::RemoteControl) => (String::new(), 0, false),
                _ => (String::new(), 0, false),
            };
        let (mcp_configured_count, mcp_connecting) =
            crate::mcp::load_config_with_workspace_and_plugins(
                &mcp_config_path,
                &workspace,
                plugin_registry.as_ref(),
            )
            .map(|cfg| {
                let mut connecting = cfg
                    .servers
                    .iter()
                    .filter(|(_, server)| server.is_enabled())
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                connecting.sort();
                (cfg.servers.len(), connecting)
            })
            .unwrap_or((0, Vec::new()));
        let mut hotbar_actions = HotbarActionRegistry::with_configured_routes(
            config,
            provider,
            &model,
            &provider_models,
        );
        // #2069: expose the already-discovered skills as bindable hotbar
        // actions. Reuses the startup skill cache, so no extra filesystem I/O.
        hotbar_actions.register_skills(&cached_skills);
        let composer_arrows_scroll_explicit = config
            .tui
            .as_ref()
            .and_then(|tui| tui.composer_arrows_scroll)
            .is_some();
        let mut app = Self {
            mode: initial_mode,
            hotbar_actions,
            composer: ComposerState {
                input: initial_input_text,
                cursor_position: initial_input_cursor,
                kill_buffer: String::new(),
                paste_burst: PasteBurst::default(),
                pending_paste_reference: None,
                oversized_paste_full_text: None,
                input_history,
                draft_history: VecDeque::new(),
                clear_undo_buffer: None,
                history_index: None,
                history_navigation_draft: None,
                composer_history_search: None,
                selected_attachment_index: None,
                slash_menu_selected: 0,
                slash_menu_hidden: false,
                mention_menu_selected: 0,
                mention_menu_hidden: false,
                mention_completion_cache: None,
                mention_discovery: crate::tui::mention_completion::MentionDiscovery::default(),
                mention_cwd,
                vim_enabled: composer_vim_enabled,
                vim_mode: VimMode::Normal,
                vim_pending_d: false,
                selection_anchor: None,
            },
            viewport: ViewportState::default(),
            work_surface: {
                let mut state = crate::tui::work_surface::WorkSurfaceState::with_layout(
                    work_surface_placement,
                    work_surface_top_height,
                    work_surface_side_width,
                );
                state.panel = crate::tui::work_surface::RailPanel::parse(&settings.rail_panel);
                state
            },
            goal: HostGoalState::default(),
            session: SessionState::default(),
            last_billed_input_tokens: None,
            last_compaction: None,
            active_allowed_tools: None,
            pausable: false,
            pending_route_save: None,
            paused: false,
            paused_goal_objective: None,
            history: Vec::new(),
            history_version: 0,
            transcript_identity_epoch: 0,
            history_revisions: Vec::new(),
            tool_run_cache: ToolRunCache::default(),
            next_history_revision: 1,
            api_messages: Vec::new(),
            completed_assistant_outputs: Vec::new(),
            context_token_cache: std::cell::RefCell::new(Default::default()),
            remote_control: crate::remote_control::RemoteControlController::default(),
            start_remote_control_on_launch: start_remote_control,
            is_loading: false,
            dispatch_completion_tx: None,
            dispatch_in_flight: false,
            last_enter_instant: None,
            provider_wait_incident_logged: false,
            prompt_suggestion: None,
            prompt_suggestion_gen: std::sync::atomic::AtomicU64::new(0),
            offline_mode: false,
            turn_error_posted: false,
            // Surface parse warnings so the user knows their config file is
            // broken instead of silently losing all settings.
            status_message: xai_dangling_repair_message
                .or(settings_parse_warning)
                .or(tui_prefs_warning)
                .or(theme_warning),
            status_toasts: VecDeque::new(),
            update_available: None,
            sticky_status: None,
            last_status_message_seen: None,
            context_pressure_warning_dismissed: None,
            plugin_reload_nudge_stamp: None,
            plugin_prompt_suggest_names: HashSet::new(),
            plugin_prompt_suggest_count: 0,
            last_plugin_catalog_poll: None,
            plugin_cta: crate::tui::plugin_suggestions::PluginCtaState::default(),
            model,
            provider_models,
            enabled_provider_models,
            pinned_models: settings.pinned_models.clone(),
            auto_model,
            last_effective_model: None,
            last_effective_provider: None,
            last_effective_provider_identity: None,
            last_auto_route_receipt: None,
            pending_turn_route: None,
            pending_auto_route_receipt: None,
            active_turn: None,
            api_provider: provider,
            provider_identity,
            provider_exact_id,
            provider_chain,
            provider_readiness,
            provider_health: crate::provider_readiness::ProviderReadinessSnapshot::default(),
            last_fallback_reason: None,
            model_ids_passthrough,
            active_route_limits,
            active_route_base_url,
            active_context_window_source,
            active_context_window_override,
            pending_provider_switch: None,
            reasoning_effort,
            reasoning_effort_preference,
            last_effective_reasoning_effort: None,
            workspace,
            workflow_config: config.workflow_config(),
            goal_max_continuations: config.goal_max_continuations(),
            goal_continuation_waiting: false,
            configured_sandbox_mode: config.sandbox_mode.clone(),
            configured_sandbox_network: config.sandbox_network_access,
            sandbox_backend: crate::sandbox::get_platform_sandbox_with_bwrap_preference(
                config.prefer_bwrap.unwrap_or(false),
            ),
            // #4022: the worker thread is spawned lazily on first submit, so
            // constructing an App never costs a thread.
            lane_control: crate::lane_control::LaneControlQueue::new(),
            plugin_registry,
            config_path,
            config_profile,
            legacy_plugin_tools_dir: config
                .tools
                .as_ref()
                .and_then(|tools| tools.plugin_dir.as_deref())
                .map(PathBuf::from),
            mcp_config_path: mcp_config_path.clone(),
            skills_dir,
            skills_scan_codewhale_only,
            project_context_pack_enabled: config.project_context_pack_enabled(),
            memory_path,
            use_memory,
            screen_mode,
            use_mouse_capture,
            mouse_capture_preference,
            use_bracketed_paste,
            use_paste_burst_detection,
            bracketed_paste_seen: false,
            system_prompt: None,
            auto_compact,
            auto_compact_user_configured,
            auto_compact_threshold_percent,
            stopped_turn: false,
            calm_mode,
            low_motion,
            constrained_frame_rate,
            ambient_clock_ms: 0,
            ambient_clock_sampled_at: None,
            ambient_idle_since: None,
            ocean_completion_started_at: None,
            ocean_turn_history_start: 0,
            ocean_receipt_settle_start: None,
            fancy_animations,
            focus_texture,
            launch,
            pending_launch_action: None,
            pending_composer_submit: None,
            pending_hotbar_slot: None,
            synchronized_output_enabled,
            status_indicator,
            show_thinking,
            thinking_highlight,
            thinking_default_expanded,
            thinking_preview_lines,
            help_expand_groups,
            pin_last_prompt,
            verbose_transcript: false,
            show_tool_details,
            inline_diff_mode,
            ui_locale,
            cost_currency,
            billing_presentation: crate::route_billing::for_route(config, provider),
            composer_density,
            composer_border,
            composer_multiline_mode,
            voice_enabled: false,
            voice_send_enabled: false,
            voice_control_enabled: false,
            transcript_spacing,
            sidebar_hover: SidebarHoverState::default(),
            sidebar_hover_tooltip: None,
            model_picker_memory: None,
            provider_picker_memory: None,
            last_mouse_pos: None,
            context_panel: settings.context_panel,
            sessions_rail: settings.sessions_rail,
            tool_collapse_threshold: 3,
            expanded_tool_runs: HashSet::new(),
            tool_collapse_mode: ToolCollapseMode::from_setting(&settings.tool_collapse_mode),
            file_tree: None,
            file_tree_visible: false,
            compact_threshold,
            max_input_history,
            allow_shell,
            verbosity: config.verbosity.clone(),
            max_subagents,
            stream_chunk_timeout_secs: config.stream_chunk_timeout_secs(),
            subagent_cache: Vec::new(),
            subagent_terminal_seen_at: HashMap::new(),
            agent_progress: HashMap::new(),
            agent_progress_meta: HashMap::new(),
            subagent_card_index: HashMap::new(),
            last_fanout_card_index: None,
            pending_subagent_dispatch: None,
            agent_activity_started_at: None,
            agent_counter: 0,
            agent_label_map: HashMap::new(),
            agent_focus: None,
            agent_queued_follow_ups: HashMap::new(),
            agent_role_counters: HashMap::new(),
            last_agent_progress_redraw: None,
            last_workflow_budget_redraw: None,
            ui_theme,
            background_color_override,
            theme_id,
            onboarding,
            onboarding_needs_api_key: needs_api_key,
            onboarding_provider: provider,
            onboarding_workspace_trust_gate,
            onboarding_missing_key_recovery,
            onboarding_explore_offline: false,
            onboarding_had_language_step: onboarding_needs_language,
            onboarding_had_provider_step: !was_onboarded && needs_api_key,
            onboarding_had_trust_step: !was_onboarded && needs_workspace_trust,
            api_key_env_only,
            hooks,
            lifecycle_outbox,
            yolo: yolo_compat,
            yolo_compat_notified: false,
            startup_defaults: Default::default(),
            keybinding_migration_notified: false,
            mode_prefs,
            approval_policy_locked,
            approval_policy_root_editable,
            approval_policy_requirements_managed,
            shell_access_editable,
            clipboard: ClipboardHandler::new(),
            approval_session_approved: HashSet::new(),
            approval_session_denied: HashSet::new(),
            approval_mode: if yolo_compat {
                ApprovalMode::Bypass
            } else {
                configured_approval_mode
            },
            view_stack: ViewStack::new(),
            pending_user_input_prompt: None,
            backtrack: crate::tui::backtrack::BacktrackState::new(),
            current_session_id: None,
            last_known_work_state: None,
            last_known_goal_state: None,
            pending_goal_controls: VecDeque::new(),
            current_session_metadata: None,
            session_artifacts: Vec::new(),
            trust_mode: yolo_compat || configured_trust_mode,
            translation_enabled: false,
            mini_window: config.mini_window.clone().unwrap_or_default(),
            status_items: config
                .tui
                .as_ref()
                .and_then(|tui| tui.status_items.clone())
                .unwrap_or_else(crate::config::StatusItem::default_footer),
            // Prose wrap cap (`[transcript] prose_measure`, #5436). Resolved
            // once here so every render pass — main cache and full-screen
            // overlay — shares one effective width; `None` = full width.
            prose_measure: config.prose_measure(),
            header_items: config
                .tui
                .as_ref()
                .and_then(|tui| tui.header_items.clone())
                .unwrap_or_else(crate::config::HeaderItem::default_header),
            project_doc: None,
            plan_state,
            todos,
            runtime_services: RuntimeToolServices {
                shell_manager: Some(shell_manager),
                work: Some(work_runtime),
                media_originals_dir: crate::media_originals::default_store_dir(),
                ..RuntimeToolServices::default()
            },
            coordination_detail: None,
            mcp_snapshot: None,
            mcp_initializing: !mcp_connecting.is_empty()
                && config.features().enabled(crate::features::Feature::Mcp),
            mcp_snapshot_generation: 0,
            mcp_snapshot_generation_invalidated: false,
            mcp_connecting,
            // Read the MCP config once at boot to know how many servers
            // the user has declared. The footer chip uses this even when
            // no live snapshot is available (#502). Cheap (just reads
            // the JSON files); errors fall through to zero so a missing
            // or malformed config simply hides the chip.
            mcp_configured_count,
            mcp_reload_required: false,
            tool_log: Vec::new(),
            active_skill: None,
            active_skill_provenance: None,
            cached_skills,
            tool_cells: HashMap::new(),
            tool_details_by_cell: HashMap::new(),
            context_references_by_cell: HashMap::new(),
            session_context_references: Vec::new(),
            active_cell: None,
            active_cell_revision: 0,
            active_tool_details: HashMap::new(),
            agent_roster: Vec::new(),
            agent_roster_print_requested: false,
            active_tool_entry_completed_at: HashMap::new(),
            exploring_cell: None,
            exploring_entries: HashMap::new(),
            ignored_tool_calls: HashSet::new(),
            last_exec_wait_command: None,
            streaming_message_index: None,
            streaming_source_receipt: None,
            suppress_stream_events_until_turn_complete: false,
            streaming_thinking_active_entry: None,
            thinking_revision_last_bump_at: None,
            streaming_state: StreamingState::new(),
            streaming_output_token_estimate: 0,
            reasoning_buffer: String::new(),
            reasoning_header: None,
            last_reasoning: None,
            pending_tool_uses: Vec::new(),
            pending_gate_receipts: Vec::new(),
            child_gate_receipts: std::collections::HashMap::new(),
            queued_messages: VecDeque::new(),
            queued_draft: None,
            pending_steers: VecDeque::new(),
            rejected_steers: VecDeque::new(),
            submit_pending_steers_after_interrupt: false,
            turn_started_at: None,
            turn_last_activity_at: None,
            cumulative_turn_duration: std::time::Duration::ZERO,
            session_metrics: crate::tui::session_metrics::SessionMetrics::default(),
            balance_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            draft_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            fleet_draft_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            constitution_draft_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            prompt_suggestion_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            balance_initiated: false,
            last_balance_fetch: None,
            runtime_turn_id: None,
            runtime_turn_status: None,
            turn_counter: 0,
            dispatch_started_at: None,
            workspace_context: None,
            workspace_context_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            workspace_context_refreshed_at: None,
            memory_size_hint: None,
            task_panel: Vec::new(),
            automation_panel: crate::tui::automation_panel::AutomationPanelState::default(),
            automation_scan: None,
            behavioral_tips: crate::tui::behavioral_tips::BehavioralTipState::default(),
            workflow_panel: None,
            session_started_at: chrono::Utc::now(),
            needs_redraw: true,
            fleet_roster_stale: false,
            force_next_full_repaint: false,
            thinking_started_at: None,
            is_compacting: false,
            active_compaction: None,
            manual_compaction_queued: false,
            manual_compaction_id: None,
            deferred_manual_compaction: None,
            is_purging: false,
            user_scrolled_during_stream: false,
            last_send_at: None,
            last_submitted_prompt: None,
            auto_submit_initial_input,
            quit_armed_until: None,
            prefix_change_count: 0,
            prefix_checks_total: 0,
            prefix_stability_pct: None,
            last_prefix_change_desc: None,
            last_pinned_prefix_hash: None,
            prefix_pin_reason: None,
            prefix_last_miss_reason: None,
            prefix_drift_count: 0,
            prefix_context_updates: 0,
            collapsed_cells: HashSet::new(),
            folded_thinking: HashSet::new(),
            collapsed_cell_map: Vec::new(),
            edit_in_progress: false,
            lsp_enabled: config.lsp.as_ref().and_then(|l| l.enabled).unwrap_or(true),
            lsp_repair: LspRepairState::default(),
            composer_arrows_scroll: config
                .tui
                .as_ref()
                .and_then(|tui| tui.composer_arrows_scroll)
                .unwrap_or_else(|| default_composer_arrows_scroll(use_mouse_capture)),
            composer_arrows_scroll_explicit,
            mention_menu_limit: settings.mention_menu_limit,
            mention_walk_depth: settings.mention_walk_depth,
            mention_menu_behavior: settings.mention_menu_behavior.clone(),
            workspace_follow_symlinks: settings.workspace_follow_symlinks,
            session_title: None,
            window_title: None,
            title_default: config
                .title
                .as_deref()
                .map(crate::session_manager::sanitize_session_title)
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty()),
            receipt_text: None,
            receipt_started_at: None,
            tool_evidence: Vec::new(),
        };
        if yolo_compat {
            app.notify_yolo_compat_once();
        }
        app
    }
}

/// Rewrite `settings.toml` with the legacy `default_mode = "yolo"` value
/// normalized away.
///
/// The normalization happens during parsing, so an empty transaction *is* the
/// migration: load (which normalizes), then save. Doing it as its own
/// [`crate::settings::Settings::transact`] rather than saving the snapshot
/// `App::new` already loaded matters twice over. It cannot write back a stale
/// pre-image, and — because `App::new` runs on the same hot path as several
/// hundred tests — it keeps the transaction lock out of the common construction
/// path entirely, taking it only when a legacy file actually needs migrating.
fn normalize_legacy_yolo_settings() -> anyhow::Result<()> {
    crate::settings::Settings::transact(|_normalized_on_load| Ok(()))
}
