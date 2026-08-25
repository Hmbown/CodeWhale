//! Config load/merge pipeline: profile application, deep `Config` merge,
//! single-file loading, managed/requirements overlays, and the
//! misplaced-top-level-key warning.
//!
//! Extracted verbatim from `config.rs` (#5586). Visibility note: the seven
//! pipeline entry points the parent (`Config::load` and friends) calls are
//! `pub(super)` here purely so `config.rs` can name them; the merge helpers
//! stay module-private and nothing is re-exported beyond `crate::config`,
//! so the crate's external surface is unchanged.

use super::*;

pub(super) fn apply_profile(config: ConfigFile, profile: Option<&str>) -> Result<Config> {
    if let Some(profile_name) = profile {
        let profiles = config.profiles.as_ref();
        match profiles.and_then(|profiles| profiles.get(profile_name)) {
            Some(override_cfg) => Ok(merge_config(config.base, override_cfg.clone())),
            None => {
                let available = profiles
                    .map(|profiles| {
                        let mut keys = profiles.keys().cloned().collect::<Vec<_>>();
                        keys.sort();
                        if keys.is_empty() {
                            "none".to_string()
                        } else {
                            keys.join(", ")
                        }
                    })
                    .unwrap_or_else(|| "none".to_string());
                anyhow::bail!("Profile '{profile_name}' not found. Available profiles: {available}")
            }
        }
    } else {
        Ok(config.base)
    }
}

pub(super) fn merge_config(base: Config, override_cfg: Config) -> Config {
    // Captured before the struct literal moves the field out of `override_cfg`.
    let override_defines_root_base_url = override_cfg.base_url.is_some();
    Config {
        provider: override_cfg.provider.or(base.provider),
        telemetry: override_cfg.telemetry.or(base.telemetry),
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        http_headers: override_cfg.http_headers.or(base.http_headers),
        default_text_model: override_cfg.default_text_model.or(base.default_text_model),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        reasoning_effort: override_cfg.reasoning_effort.or(base.reasoning_effort),
        reasoning_effort_inferred_from_legacy_alias: override_cfg
            .reasoning_effort_inferred_from_legacy_alias
            || base.reasoning_effort_inferred_from_legacy_alias,
        fleet_operator_route_applied: override_cfg.fleet_operator_route_applied
            || base.fleet_operator_route_applied,
        fleet_operator_reasoning_applied: override_cfg.fleet_operator_reasoning_applied
            || base.fleet_operator_reasoning_applied,
        migrated_legacy_ollama_cloud_route: override_cfg.migrated_legacy_ollama_cloud_route
            || base.migrated_legacy_ollama_cloud_route,
        migrated_deepseek_model_alias: override_cfg
            .migrated_deepseek_model_alias
            .or(base.migrated_deepseek_model_alias),
        tools: override_cfg.tools.or(base.tools),
        skills_dir: override_cfg.skills_dir.or(base.skills_dir),
        mcp_config_path: override_cfg.mcp_config_path.or(base.mcp_config_path),
        mcp_oauth_callback_port: override_cfg
            .mcp_oauth_callback_port
            .or(base.mcp_oauth_callback_port),
        mcp_oauth_callback_url: override_cfg
            .mcp_oauth_callback_url
            .or(base.mcp_oauth_callback_url),
        notes_path: override_cfg.notes_path.or(base.notes_path),
        memory_path: override_cfg.memory_path.or(base.memory_path),
        vision_model: override_cfg.vision_model.or(base.vision_model),
        // #454: user-owned overlays such as profiles and managed config may
        // replace the instruction array. Project-scope config is filtered in
        // main.rs and cannot set instruction paths.
        instructions: override_cfg.instructions.or(base.instructions),
        stop_words: override_cfg.stop_words.or(base.stop_words),
        allow_shell: override_cfg.allow_shell.or(base.allow_shell),
        prompt_suggestion: override_cfg.prompt_suggestion.or(base.prompt_suggestion),
        yolo: override_cfg.yolo.or(base.yolo),
        verbosity: override_cfg.verbosity.or(base.verbosity),
        approval_policy: override_cfg.approval_policy.or(base.approval_policy),
        sandbox_mode: override_cfg.sandbox_mode.or(base.sandbox_mode),
        sandbox_network_access: override_cfg
            .sandbox_network_access
            .or(base.sandbox_network_access),
        project_instruction_imports: if override_cfg.project_instruction_imports.is_empty() {
            base.project_instruction_imports
        } else {
            override_cfg.project_instruction_imports
        },
        fallback_providers: if override_cfg.fallback_providers.is_empty() {
            base.fallback_providers
        } else {
            override_cfg.fallback_providers
        },
        sandbox_backend: override_cfg.sandbox_backend.or(base.sandbox_backend),
        sandbox_url: override_cfg.sandbox_url.or(base.sandbox_url),
        sandbox_api_key: override_cfg.sandbox_api_key.or(base.sandbox_api_key),
        prefer_bwrap: override_cfg.prefer_bwrap.or(base.prefer_bwrap),
        bwrap_ro_roots: if override_cfg.bwrap_ro_roots.is_empty() {
            base.bwrap_ro_roots
        } else {
            override_cfg.bwrap_ro_roots
        },
        bwrap_dev_roots: if override_cfg.bwrap_dev_roots.is_empty() {
            base.bwrap_dev_roots
        } else {
            override_cfg.bwrap_dev_roots
        },
        sandbox_denied_read_paths: if override_cfg.sandbox_denied_read_paths.is_empty() {
            base.sandbox_denied_read_paths
        } else {
            override_cfg.sandbox_denied_read_paths
        },
        managed_config_path: override_cfg
            .managed_config_path
            .or(base.managed_config_path),
        requirements_path: override_cfg.requirements_path.or(base.requirements_path),
        max_subagents: override_cfg.max_subagents.or(base.max_subagents),
        retry: override_cfg.retry.or(base.retry),
        auto_review: override_cfg.auto_review.or(base.auto_review),
        tui: override_cfg.tui.or(base.tui),
        transcript: override_cfg.transcript.or(base.transcript),
        hooks: override_cfg.hooks.or(base.hooks),
        lifecycle_outbox: override_cfg.lifecycle_outbox.or(base.lifecycle_outbox),
        control_socket: override_cfg.control_socket.or(base.control_socket),
        providers: merge_providers(base.providers, override_cfg.providers),
        features: merge_features(base.features, override_cfg.features),
        notifications: override_cfg.notifications.or(base.notifications),
        approval: override_cfg.approval.or(base.approval),
        network: override_cfg.network.or(base.network),
        verifier: override_cfg.verifier.or(base.verifier),
        advisor: override_cfg.advisor.or(base.advisor),
        skills: merge_skills_config(base.skills, override_cfg.skills),
        snapshots: override_cfg.snapshots.or(base.snapshots),
        search: override_cfg.search.or(base.search),
        goal: override_cfg.goal.or(base.goal),
        memory: override_cfg.memory.or(base.memory),
        speech: override_cfg.speech.or(base.speech),
        auto: override_cfg.auto.or(base.auto),
        hotbar: override_cfg.hotbar.or(base.hotbar),
        update: override_cfg.update.or(base.update),
        lsp: override_cfg.lsp.or(base.lsp),
        context: ContextConfig {
            enabled: override_cfg.context.enabled.or(base.context.enabled),
            project_pack: override_cfg
                .context
                .project_pack
                .or(base.context.project_pack),
            verbatim_window_turns: override_cfg
                .context
                .verbatim_window_turns
                .or(base.context.verbatim_window_turns),
            l1_threshold: override_cfg
                .context
                .l1_threshold
                .or(base.context.l1_threshold),
            l2_threshold: override_cfg
                .context
                .l2_threshold
                .or(base.context.l2_threshold),
            l3_threshold: override_cfg
                .context
                .l3_threshold
                .or(base.context.l3_threshold),
            seam_model: override_cfg.context.seam_model.or(base.context.seam_model),
        },
        fleet: override_cfg.fleet.or(base.fleet),
        workflow: override_cfg.workflow.or(base.workflow),
        subagents: override_cfg.subagents.or(base.subagents),
        strict_tool_mode: override_cfg.strict_tool_mode.or(base.strict_tool_mode),
        runtime_api: override_cfg.runtime_api.or(base.runtime_api),
        workshop: override_cfg.workshop.or(base.workshop),
        exec_policy_engine: override_cfg.exec_policy_engine,
        base_url_env_receipt: match override_cfg.base_url_env_receipt {
            BaseUrlEnvReceipt::Unrecorded => base.base_url_env_receipt,
            recorded => recorded,
        },
        // A layer that supplies its own root `base_url` replaces the
        // environment's write, so that layer's ownership wins outright.
        root_base_url_owner: if override_defines_root_base_url {
            override_cfg.root_base_url_owner
        } else {
            match override_cfg.root_base_url_owner {
                BaseUrlEnvReceipt::Unrecorded => base.root_base_url_owner,
                recorded => recorded,
            }
        },
        runtime_chat_isolated: override_cfg.runtime_chat_isolated || base.runtime_chat_isolated,
        runtime_thread_inference_unrelated: override_cfg.runtime_thread_inference_unrelated
            || base.runtime_thread_inference_unrelated,
        mini_window: override_cfg.mini_window.or(base.mini_window),
        title: override_cfg.title.or(base.title),
    }
}

pub(super) fn load_sibling_exec_policy_engine(
    config_path: Option<&Path>,
) -> Result<ExecPolicyEngine> {
    let Some(config_path) = config_path else {
        return Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()));
    };
    let permissions_path = codewhale_config::permissions_path_for_config_path(config_path);
    if !permissions_path.exists() {
        return Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()));
    }

    let raw = fs::read_to_string(&permissions_path).with_context(|| {
        format!(
            "Failed to read permissions file: {}",
            permissions_path.display()
        )
    })?;
    let permissions: codewhale_config::PermissionsToml = toml::from_str(&raw).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse permissions file {}; file contents were omitted",
            codewhale_config::quote_os_path(&permissions_path)
        )
    })?;
    if permissions.is_empty() {
        Ok(ExecPolicyEngine::new(Vec::new(), Vec::new()))
    } else {
        Ok(ExecPolicyEngine::with_rulesets(vec![permissions.ruleset()]))
    }
}

fn merge_skills_config(
    base: Option<SkillsConfig>,
    override_cfg: Option<SkillsConfig>,
) -> Option<SkillsConfig> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
        (Some(base), Some(override_cfg)) => Some(SkillsConfig {
            registry_url: override_cfg.registry_url.or(base.registry_url),
            max_install_size_bytes: override_cfg
                .max_install_size_bytes
                .or(base.max_install_size_bytes),
            scan_codewhale_only: override_cfg
                .scan_codewhale_only
                .or(base.scan_codewhale_only),
        }),
    }
}

fn merge_provider_config(base: ProviderConfig, override_cfg: ProviderConfig) -> ProviderConfig {
    ProviderConfig {
        api_key: override_cfg.api_key.or(base.api_key),
        base_url: override_cfg.base_url.or(base.base_url),
        model: override_cfg.model.or(base.model),
        context_window: override_cfg.context_window.or(base.context_window),
        mode: override_cfg.mode.or(base.mode),
        wire: override_cfg.wire.or(base.wire),
        auth_mode: override_cfg.auth_mode.or(base.auth_mode),
        oauth_credential_generation: override_cfg
            .oauth_credential_generation
            .or(base.oauth_credential_generation),
        insecure_skip_tls_verify: override_cfg
            .insecure_skip_tls_verify
            .or(base.insecure_skip_tls_verify),
        http_headers: override_cfg.http_headers.or(base.http_headers),
        path_suffix: override_cfg.path_suffix.or(base.path_suffix),
        reasoning_stream_style: override_cfg
            .reasoning_stream_style
            .or(base.reasoning_stream_style),
        max_concurrency: override_cfg.max_concurrency.or(base.max_concurrency),
        auth: override_cfg.auth.or(base.auth),
        external_credentials: override_cfg
            .external_credentials
            .or(base.external_credentials),
        kind: override_cfg.kind.or(base.kind),
        api_key_env: override_cfg.api_key_env.or(base.api_key_env),
    }
}

/// Merge the per-name custom provider maps (#1519): the union of both key sets,
/// with each shared key deep-merged via [`merge_provider_config`] (override
/// wins field-by-field). Keys present in only one map are carried through as-is.
fn merge_custom_providers(
    mut base: HashMap<String, ProviderConfig>,
    override_cfg: HashMap<String, ProviderConfig>,
) -> HashMap<String, ProviderConfig> {
    for (name, entry) in override_cfg {
        let merged = match base.remove(&name) {
            Some(base_entry) => merge_provider_config(base_entry, entry),
            None => entry,
        };
        base.insert(name, merged);
    }
    base
}

fn merge_providers(
    base: Option<ProvidersConfig>,
    override_cfg: Option<ProvidersConfig>,
) -> Option<ProvidersConfig> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
        (Some(base), Some(override_cfg)) => Some(ProvidersConfig {
            deepseek: merge_provider_config(base.deepseek, override_cfg.deepseek),
            deepseek_cn: merge_provider_config(base.deepseek_cn, override_cfg.deepseek_cn),
            deepseek_anthropic: merge_provider_config(
                base.deepseek_anthropic,
                override_cfg.deepseek_anthropic,
            ),
            nvidia_nim: merge_provider_config(base.nvidia_nim, override_cfg.nvidia_nim),
            openai: merge_provider_config(base.openai, override_cfg.openai),
            anthropic: merge_provider_config(base.anthropic, override_cfg.anthropic),
            openmodel: merge_provider_config(base.openmodel, override_cfg.openmodel),
            atlascloud: merge_provider_config(base.atlascloud, override_cfg.atlascloud),
            wanjie_ark: merge_provider_config(base.wanjie_ark, override_cfg.wanjie_ark),
            openrouter: merge_provider_config(base.openrouter, override_cfg.openrouter),
            orcarouter: merge_provider_config(base.orcarouter, override_cfg.orcarouter),
            xiaomi_mimo: merge_provider_config(base.xiaomi_mimo, override_cfg.xiaomi_mimo),
            novita: merge_provider_config(base.novita, override_cfg.novita),
            fireworks: merge_provider_config(base.fireworks, override_cfg.fireworks),
            siliconflow: merge_provider_config(base.siliconflow, override_cfg.siliconflow),
            siliconflow_cn: merge_provider_config(base.siliconflow_cn, override_cfg.siliconflow_cn),
            arcee: merge_provider_config(base.arcee, override_cfg.arcee),
            moonshot: merge_provider_config(base.moonshot, override_cfg.moonshot),
            sglang: merge_provider_config(base.sglang, override_cfg.sglang),
            vllm: merge_provider_config(base.vllm, override_cfg.vllm),
            ollama: merge_provider_config(base.ollama, override_cfg.ollama),
            ollama_cloud: merge_provider_config(base.ollama_cloud, override_cfg.ollama_cloud),
            volcengine: merge_provider_config(base.volcengine, override_cfg.volcengine),
            huggingface: merge_provider_config(base.huggingface, override_cfg.huggingface),
            deepinfra: merge_provider_config(base.deepinfra, override_cfg.deepinfra),
            together: merge_provider_config(base.together, override_cfg.together),
            qianfan: merge_provider_config(base.qianfan, override_cfg.qianfan),
            openai_codex: merge_provider_config(base.openai_codex, override_cfg.openai_codex),
            zai: merge_provider_config(base.zai, override_cfg.zai),
            stepfun: merge_provider_config(base.stepfun, override_cfg.stepfun),
            minimax: merge_provider_config(base.minimax, override_cfg.minimax),
            minimax_anthropic: merge_provider_config(
                base.minimax_anthropic,
                override_cfg.minimax_anthropic,
            ),
            sakana: merge_provider_config(base.sakana, override_cfg.sakana),
            longcat: merge_provider_config(base.longcat, override_cfg.longcat),
            opencode_go: merge_provider_config(base.opencode_go, override_cfg.opencode_go),
            opencode_zen: merge_provider_config(base.opencode_zen, override_cfg.opencode_zen),
            meta: merge_provider_config(base.meta, override_cfg.meta),
            xai: merge_provider_config(base.xai, override_cfg.xai),
            mistral: merge_provider_config(base.mistral, override_cfg.mistral),
            google: merge_provider_config(base.google, override_cfg.google),
            antigravity: merge_provider_config(base.antigravity, override_cfg.antigravity),
            telecomjs: merge_provider_config(base.telecomjs, override_cfg.telecomjs),
            edenai: merge_provider_config(base.edenai, override_cfg.edenai),
            modelstudio_token_plan: merge_provider_config(
                base.modelstudio_token_plan,
                override_cfg.modelstudio_token_plan,
            ),
            modelstudio_token_plan_anthropic: merge_provider_config(
                base.modelstudio_token_plan_anthropic,
                override_cfg.modelstudio_token_plan_anthropic,
            ),
            modelstudio_coding_plan: merge_provider_config(
                base.modelstudio_coding_plan,
                override_cfg.modelstudio_coding_plan,
            ),
            modelstudio_coding_plan_anthropic: merge_provider_config(
                base.modelstudio_coding_plan_anthropic,
                override_cfg.modelstudio_coding_plan_anthropic,
            ),
            custom: merge_custom_providers(base.custom, override_cfg.custom),
        }),
    }
}

pub(super) fn load_single_config_file(path: &Path) -> Result<Config> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    let parsed: ConfigFile = parse_config_file_str(&contents).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse config file {}; file contents were omitted",
            codewhale_config::quote_os_path(path)
        )
    })?;
    Ok(parsed.base)
}

/// Build a one-line warning when top-level-only keys are nested under a section
/// Codewhale does not define (`[general]` / `[sandbox]`). TOML silently drops
/// those keys, so e.g. `[general]\nallow_shell = true` never takes effect and
/// the shell tools (`exec_shell`, `task_shell_start`, …) are absent from the
/// catalog with no explanation. Returns `None` when nothing is misplaced.
///
/// This is the exact confusion behind #2589: `allow_shell` and `sandbox_mode`
/// belong at the top of the file, above any `[section]` header.
pub(super) fn warn_on_misplaced_top_level_keys(raw: &str) -> Option<String> {
    let doc = toml::from_str::<toml::Value>(raw).ok()?;
    // Sections Codewhale does not recognize but users nest settings under.
    const UNKNOWN_SECTIONS: &[&str] = &["general", "sandbox"];
    // Keys that are only ever read from the top level of the config.
    const TOP_LEVEL_KEYS: &[&str] = &[
        "allow_shell",
        "sandbox_mode",
        "approval_policy",
        "verbosity",
    ];

    let mut hits: Vec<String> = Vec::new();
    for section in UNKNOWN_SECTIONS {
        let Some(table) = doc.get(*section).and_then(toml::Value::as_table) else {
            continue;
        };
        for key in TOP_LEVEL_KEYS {
            if table.contains_key(*key) {
                hits.push(format!("`{section}.{key}`"));
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    Some(format!(
        "Ignoring {} — Codewhale has no `[general]` or `[sandbox]` section, so these \
         keys are silently dropped. Move them to the TOP of the config file (above any \
         `[section]` header), e.g. `allow_shell = true`. Until then, shell tools stay \
         disabled. (#2589)",
        hits.join(", ")
    ))
}

pub(super) fn apply_managed_overrides(config: &mut Config) -> Result<()> {
    let path = config
        .managed_config_path
        .as_deref()
        .map(expand_path)
        .or_else(default_managed_config_path);
    let Some(path) = path else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    let mut managed = load_single_config_file(&path)?;
    strip_external_credential_consent(&mut managed);
    let prior_route = (
        config.api_provider(),
        config.provider_identity_for(config.api_provider()),
    );
    let mut merged = merge_config(config.clone(), managed.clone());
    let merged_route = (
        merged.api_provider(),
        merged.provider_identity_for(merged.api_provider()),
    );
    if prior_route != merged_route || config_defines_base_url_for_effective_route(&managed, &merged)
    {
        // Managed configuration is a higher-precedence file layer. If it
        // selects a different route or supplies that route's endpoint, the
        // lower environment layer no longer owns the effective base URL.
        //
        // Record that as an explicit "nobody owns it" rather than clearing the
        // receipt. Clearing it would read as "this config never met the
        // environment layer", which re-enables the generic
        // `CODEWHALE_BASE_URL` fallback for every route — including pinned
        // cross-provider children, which would then borrow an ambient host
        // that managed routing had just taken authority over.
        merged.base_url_env_receipt = BaseUrlEnvReceipt::NoOwner;
        // The shared legacy root field is the same ambient host by another
        // name. If the environment wrote it, managed authority takes it from
        // every route rather than leaving it addressed to the identity that
        // was active before the overlay. A *file*-owned root is left alone:
        // managed did not override it, so it stays the user's value.
        if matches!(merged.root_base_url_owner, BaseUrlEnvReceipt::Route(..)) {
            merged.root_base_url_owner = BaseUrlEnvReceipt::NoOwner;
        }
    }
    *config = merged;
    Ok(())
}

/// Organization-managed overlays may constrain routing and policy, but they
/// cannot consent on a user's behalf to credential files owned by another
/// CLI. Only the user config/profile loaded before this layer may carry these
/// grants. A managed `disabled` record is a tightening tombstone and is kept
/// so a lower-precedence user grant cannot survive an administrator deny.
fn strip_external_credential_consent(config: &mut Config) {
    if config.providers.is_none() {
        return;
    }
    for provider in ApiProvider::all()
        .iter()
        .copied()
        .filter(|provider| *provider != ApiProvider::Custom)
    {
        let external = &mut config
            .provider_config_for_mut(provider)
            .external_credentials;
        if external.as_ref().is_some_and(|consent| {
            consent.access != codewhale_config::ExternalCredentialAccess::Disabled
        }) {
            *external = None;
        }
    }
    if let Some(providers) = config.providers.as_mut() {
        for provider in providers.custom.values_mut() {
            if provider
                .external_credentials
                .as_ref()
                .is_some_and(|consent| {
                    consent.access != codewhale_config::ExternalCredentialAccess::Disabled
                })
            {
                provider.external_credentials = None;
            }
        }
    }
}

fn config_defines_base_url_for_effective_route(source: &Config, effective: &Config) -> bool {
    let provider = effective.api_provider();
    let mut source = source.clone();
    source.provider.clone_from(&effective.provider);
    let provider_base = source
        .provider_config_string_with_runtime_fallback(provider, |entry| entry.base_url.clone());
    let configured = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => provider_base.or(source.base_url),
        ApiProvider::NvidiaNim => provider_base.or_else(|| {
            source
                .base_url
                .filter(|base| base.contains("integrate.api.nvidia.com"))
        }),
        ApiProvider::Custom if effective.uses_legacy_literal_custom_route() => source.base_url,
        _ => provider_base,
    };
    configured.is_some_and(|base| !base.trim().is_empty())
}

pub(super) fn apply_requirements(config: &mut Config) -> Result<()> {
    let path = config
        .requirements_path
        .as_deref()
        .map(expand_path)
        .or_else(default_requirements_path);
    let Some(path) = path else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read requirements file: {}", path.display()))?;
    let requirements: RequirementsFile = toml::from_str(&contents).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse requirements file {}; file contents were omitted",
            codewhale_config::quote_os_path(&path)
        )
    })?;

    if !requirements.allowed_approval_policies.is_empty()
        && let Some(policy) = config.approval_policy.as_ref()
    {
        let policy = policy.to_ascii_lowercase();
        if !requirements
            .allowed_approval_policies
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&policy))
        {
            anyhow::bail!(
                "approval_policy '{policy}' is not allowed by requirements ({})",
                requirements.allowed_approval_policies.join(", ")
            );
        }
    }
    if !requirements.allowed_sandbox_modes.is_empty()
        && let Some(mode) = config.sandbox_mode.as_ref()
    {
        let mode = mode.to_ascii_lowercase();
        if !requirements
            .allowed_sandbox_modes
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&mode))
        {
            anyhow::bail!(
                "sandbox_mode '{mode}' is not allowed by requirements ({})",
                requirements.allowed_sandbox_modes.join(", ")
            );
        }
    }

    Ok(())
}

fn merge_features(
    base: Option<FeaturesToml>,
    override_cfg: Option<FeaturesToml>,
) -> Option<FeaturesToml> {
    match (base, override_cfg) {
        (None, None) => None,
        (Some(mut base), Some(override_cfg)) => {
            for (key, value) in override_cfg.entries {
                base.entries.insert(key, value);
            }
            Some(base)
        }
        (Some(base), None) => Some(base),
        (None, Some(override_cfg)) => Some(override_cfg),
    }
}
