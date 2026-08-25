//! Doctor CLI/report layer: the human `codewhale doctor` report, the
//! JSON/context reporters, legacy-state and recoverable-session report
//! types, and the per-check `doctor_*` helpers.
//!
//! Extracted verbatim from `lib.rs` (#5586). Items were crate-private in
//! the root and are `pub(crate)` here purely so the root's glob re-export
//! keeps every dispatch site and test-module reference resolving
//! unchanged; nothing is exported beyond the crate.

use super::*;

/// Run system diagnostics
pub(crate) async fn run_doctor(
    config: &Config,
    workspace: &Path,
    config_path_override: Option<&Path>,
    probes: crate::doctor::DoctorProbeRequest,
    plugins: &crate::plugins::PluginRegistry,
) {
    use crate::palette;
    use colored::Colorize;

    let (accent_r, accent_g, accent_b) = palette::WHALE_HUMAN_RGB;
    let (sky_r, sky_g, sky_b) = palette::WHALE_INFO_RGB;
    let (aqua_r, aqua_g, aqua_b) = palette::WHALE_INFO_RGB;
    let (red_r, red_g, red_b) = palette::WHALE_ERROR_RGB;

    println!(
        "{}",
        "codewhale Doctor"
            .truecolor(accent_r, accent_g, accent_b)
            .bold()
    );
    println!("{}", "==================".truecolor(sky_r, sky_g, sky_b));
    println!();

    // Version info
    println!("{}", "Version Information:".bold());
    println!("  codewhale-tui: {}", env!("CODEWHALE_BUILD_VERSION"));
    println!("  rust: {}", rustc_version());
    println!();

    println!("{}", "Updates:".bold());
    crate::doctor::print_update_report(probes).await;
    println!();

    // Configuration summary
    let doctor_paths = match crate::doctor::DoctorPathReport::resolve(config_path_override) {
        Ok(paths) => paths,
        Err(error) => {
            println!("{}", "Resolved User Paths:".bold());
            println!(
                "  {} unavailable: {error:#}",
                "✗".truecolor(red_r, red_g, red_b)
            );
            return;
        }
    };
    println!("{}", "Configuration:".bold());
    let config_path = &doctor_paths.config;

    if config_path.exists() {
        println!(
            "  {} config.toml found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(config_path)
        );
        // Secret hygiene: name the keys, never the values. Plain-text config
        // is not a secret store.
        if let Ok(raw) = std::fs::read_to_string(config_path) {
            let flagged = crate::doctor::config_credential_shaped_keys(&raw);
            if !flagged.is_empty() {
                println!(
                    "  {} credential-shaped value(s) in config.toml ({}): move them to the secret backend, then scrub the file — config.toml is plain text",
                    "!".truecolor(sky_r, sky_g, sky_b),
                    flagged.join(", ")
                );
            }
        }
    } else {
        println!(
            "  {} config.toml not found at {} (using defaults/env)",
            "!".truecolor(sky_r, sky_g, sky_b),
            crate::utils::display_path(config_path)
        );
    }
    println!("  workspace: {}", crate::utils::display_path(workspace));
    println!("  {}", doctor_search_provider_line(config));

    println!();
    println!("{}", "Resolved User Paths (read-only):".bold());
    for (label, path) in doctor_paths.entries() {
        println!("  · {label}: {}", crate::utils::display_path(path));
    }

    let secret_backend = codewhale_secrets::diagnose_secret_backend();
    println!();
    println!("{}", "Secret Backend (structural only):".bold());
    for line in crate::doctor::secret_backend_human_lines(&secret_backend) {
        println!("  · {line}");
    }

    // State root (v0.8.44)
    println!();
    println!("{}", "State Root:".bold());
    let (code_home, legacy_home) = doctor_state_roots();
    let active_root = if code_home.exists() {
        &code_home
    } else if legacy_home.exists() {
        &legacy_home
    } else {
        &code_home
    };
    println!("  active: {}", crate::utils::display_path(active_root));
    if active_root != &code_home {
        println!(
            "  note: legacy {} found; start Codewhale once to trigger safe migration where available.",
            crate::utils::display_path(&legacy_home)
        );
    }
    if legacy_home.exists() && code_home.exists() {
        println!(
            "  dual roots: {} (primary) + {} (legacy)",
            crate::utils::display_path(&code_home),
            crate::utils::display_path(&legacy_home)
        );
    }
    let legacy_state_report = doctor_legacy_state_report(&code_home, &legacy_home);
    let session_recovery = doctor_session_recovery_report(
        &code_home,
        &legacy_home,
        codewhale_config::codewhale_home_is_explicit(),
    );
    print_doctor_legacy_state_report(
        &legacy_state_report,
        &session_recovery,
        (aqua_r, aqua_g, aqua_b),
        (sky_r, sky_g, sky_b),
    );

    let (setup_state, setup_source) = doctor_setup_state(config, workspace);
    print_doctor_setup_report(
        config,
        workspace,
        &setup_state,
        setup_source,
        (aqua_r, aqua_g, aqua_b),
        (sky_r, sky_g, sky_b),
    );
    print_doctor_fleet_roster_layers(config, workspace);

    // Check API keys
    println!();
    println!("{}", "API Keys:".bold());

    // Per-provider state: env + config file only (no values printed).
    // Keep doctor/status prompt-free and credential-value-free even for
    // unsigned rebuilt binaries.
    for provider in crate::config::ApiProvider::all().iter().copied() {
        let slot = provider.as_str();
        let provider_config = config.provider_config_for(provider);
        let config_declared = provider_config.is_some_and(|entry| {
            entry.api_key.as_deref().is_some_and(|key| {
                crate::config::classify_config_api_key_value(key)
                    == crate::config::ConfigApiKeyValueKind::Literal
            })
        }) || (matches!(provider, crate::config::ApiProvider::Deepseek)
            && config.api_key.as_deref().is_some_and(|key| {
                crate::config::classify_config_api_key_value(key)
                    == crate::config::ConfigApiKeyValueKind::Literal
            }));
        let env_source_declared = provider_config
            .and_then(|entry| entry.api_key_env.as_deref())
            .is_some_and(|name| !name.trim().is_empty());
        let icon = if config_declared || env_source_declared {
            "·".truecolor(aqua_r, aqua_g, aqua_b)
        } else {
            "·".dimmed()
        };
        println!(
            "  {} {slot}: env_source={}, config_source={}",
            icon,
            if env_source_declared {
                "declared (value not inspected)"
            } else {
                "not inspected"
            },
            if config_declared {
                "declared (value not inspected)"
            } else {
                "not declared"
            }
        );
    }
    println!("  · credential precedence is unchanged; doctor does not inspect credential values");
    println!();
    println!(
        "{}",
        "External credential consent (configuration only):".bold()
    );
    for line in doctor_external_credential_consent_lines(config) {
        println!("  {line}");
    }

    println!();
    println!(
        "{}",
        "DeepSeek Harness integration (read-only detection):".bold()
    );
    for line in doctor_dsh_integration_lines(config, workspace) {
        println!("  {line}");
    }

    let credential = resolve_credential_diagnostic(config);
    let source_label = match credential.source {
        ApiKeySource::ConfigDeclared => "literal config value structurally present",
        ApiKeySource::EnvDeclared => "environment source declared; value not inspected",
        ApiKeySource::ExternalAuthDeclared => {
            "external auth source declared; credential not resolved"
        }
        ApiKeySource::SecretStoreUnprobed => "secret store eligible; store not probed",
        ApiKeySource::SecretStoreUnavailable => {
            "secret-store sentinel declared, but this route cannot use that store"
        }
        ApiKeySource::OAuth => "OAuth route configured; token availability not probed",
        ApiKeySource::ExternalConsent => "external consent configured; token file not read",
        ApiKeySource::NoAuth => "no-auth route",
        ApiKeySource::LocalRuntime => "local runtime; credentials not required",
        ApiKeySource::Unknown => "unknown; credential environment and stores not inspected",
    };
    println!(
        "  {} active provider credential source: {source_label}",
        "·".dimmed()
    );
    println!(
        "  · active provider credential availability: {}",
        credential.availability.label()
    );

    // API connectivity test
    println!();
    println!("{}", "API Connectivity:".bold());
    let api_target = doctor_api_target(config);
    // Configured-vs-active honesty (DGF-01): doctor describes the route a
    // session launched NOW would resolve. It cannot see inside an already
    // running session, which keeps the route it resolved at its own launch.
    println!(
        "  · scope: configured route — what a session launched now would use; a running session keeps the route it resolved at launch (its TUI header shows the live route)"
    );
    println!("  · provider: {}", api_target.provider);
    println!(
        "  · base_url: {}",
        crate::doctor::structural_url_authority(&api_target.base_url)
    );
    match api_target.resolution {
        DoctorModelResolution::Resolved => {
            println!("  · model: {} (resolved)", api_target.model);
        }
        DoctorModelResolution::ConfiguredOnly => {
            println!(
                "  · model: {} (configured; route resolution unavailable)",
                api_target.model
            );
        }
    }
    let tls_status = doctor_tls_status(config);
    if !tls_status.certificate_verification {
        println!("  ! {}", tls_status.message);
        println!("    Prefer SSL_CERT_FILE with a trusted custom CA bundle when possible.");
    }
    let strict_tool_mode = doctor_strict_tool_mode_status(config);
    let strict_icon = match strict_tool_mode.status {
        "ready" => "✓".truecolor(aqua_r, aqua_g, aqua_b),
        "fallback_non_beta" | "custom_endpoint" => "!".truecolor(sky_r, sky_g, sky_b),
        _ => "·".dimmed(),
    };
    println!(
        "  {} strict_tool_mode: {}",
        strict_icon, strict_tool_mode.message
    );
    if let Some(recommended) = strict_tool_mode.recommended_base_url.as_deref() {
        println!(
            "    Use the {} endpoint for DeepSeek strict schemas.",
            crate::doctor::structural_url_authority(recommended)
        );
    }
    let capability = crate::config::provider_capability(config.api_provider(), &api_target.model);
    if let Some(alias) = capability.alias_deprecation.as_ref() {
        println!(
            "  ! model alias {} retires {}; switch to {}",
            alias.alias, alias.retirement_date, alias.replacement
        );
    }
    let live_api_requested =
        doctor_should_probe_api(config.api_provider(), &api_target.base_url, probes);
    let endpoint_is_local = crate::config::provider_route_is_keyless_self_hosted(
        config.api_provider(),
        &api_target.base_url,
    ) || crate::config::base_url_uses_local_host(&api_target.base_url);
    if doctor_should_probe_auth(config) && live_api_requested {
        print!("  {} Testing connection...", "·".dimmed());
        use std::io::Write;
        std::io::stdout().flush().ok();

        // Resolve a credential through the diagnostic-only store first, then
        // probe with an in-memory clone. Constructing the normal client from
        // the original config could otherwise trigger its legacy secret-store
        // migration while a user merely asks doctor to test connectivity.
        let connectivity_result = match config.with_read_only_api_key_for_diagnostic() {
            Ok(diagnostic_config) => test_api_connectivity(&diagnostic_config).await,
            Err(error) => Err(error),
        };
        match connectivity_result {
            Ok(()) => {
                println!(
                    "\r  {} API connection successful",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b)
                );
            }
            Err(e) => {
                let error_msg = e.to_string();
                println!(
                    "\r  {} API connection failed",
                    "✗".truecolor(red_r, red_g, red_b)
                );
                if error_msg.contains("401") || error_msg.contains("Unauthorized") {
                    println!(
                        "    Invalid API key. Check `codewhale auth status`, DEEPSEEK_API_KEY, or config.toml"
                    );
                } else if error_msg.contains("403") || error_msg.contains("Forbidden") {
                    println!(
                        "    API key lacks permissions. Verify key is active at platform.deepseek.com"
                    );
                } else if error_msg.contains("timeout") || error_msg.contains("Timeout") {
                    for line in doctor_timeout_recovery_lines(config) {
                        println!("    {line}");
                    }
                } else if error_msg.contains("dns") || error_msg.contains("resolve") {
                    println!("    DNS resolution failed. Check your network connection");
                } else if error_msg.contains("connect") {
                    println!("    Connection failed. Check firewall settings or try again");
                } else if crate::doctor::is_keyless_ds4_route(config) {
                    println!("    {error_msg}");
                } else {
                    println!(
                        "    Error details omitted because provider failures can contain credential material."
                    );
                }
            }
        }
    } else if !doctor_should_probe_auth(config) {
        println!(
            "  {} Live OAuth connectivity not checked by non-mutating doctor",
            "·".dimmed()
        );
        println!(
            "    Doctor never refreshes or rewrites credentials; exercise the route with a normal request."
        );
    } else {
        if endpoint_is_local {
            println!(
                "  {} Live connectivity not checked for this local endpoint",
                "·".dimmed()
            );
            println!(
                "    Run `codewhale doctor --probe-local` to opt in; the request may start a local service."
            );
        } else {
            println!(
                "  {} Live hosted connectivity not checked (offline default)",
                "·".dimmed()
            );
            println!("    Run `codewhale doctor --probe-api` to opt in.");
        }
    }

    println!();
    println!("{}", "Search Provider Reachability:".bold());
    let search_probe = crate::doctor::doctor_search_probe(config, probes).await;
    for line in crate::doctor::doctor_search_probe_lines(&search_probe) {
        println!("  {line}");
    }

    // MCP configuration
    println!();
    println!("{}", "MCP Servers (configuration only):".bold());
    println!("  · Static check only; no server process was started.");
    let features = config.features();
    if features.enabled(Feature::Mcp) {
        println!(
            "  {} MCP feature flag enabled",
            "✓".truecolor(aqua_r, aqua_g, aqua_b)
        );
    } else {
        println!(
            "  {} MCP feature flag disabled",
            "!".truecolor(sky_r, sky_g, sky_b)
        );
    }

    let mcp_config_path = config.mcp_config_path();
    let project_mcp_config_path = crate::mcp::workspace_mcp_config_path(workspace);
    if mcp_config_path.exists() {
        println!(
            "  {} MCP config found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&mcp_config_path)
        );
    } else {
        println!(
            "  {} MCP config not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&mcp_config_path)
        );
    }
    if project_mcp_config_path.exists() {
        println!(
            "  {} Project MCP config found at {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&project_mcp_config_path)
        );
    } else {
        println!(
            "  {} Project MCP config not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&project_mcp_config_path)
        );
    }

    match crate::mcp::load_config_with_workspace_and_plugins(&mcp_config_path, workspace, plugins) {
        Ok(cfg) if cfg.servers.is_empty() => {
            println!("  {} 0 merged server(s) configured", "·".dimmed());
            if !mcp_config_path.exists() && !project_mcp_config_path.exists() {
                println!("    Run `codewhale mcp init` or add `.codewhale/mcp.json`.");
            }
        }
        Ok(cfg) => {
            println!(
                "  {} {} merged server(s) configured",
                "·".dimmed(),
                cfg.servers.len()
            );
            for (name, server) in &cfg.servers {
                let status = doctor_check_mcp_server(server);
                let icon = match &status {
                    McpServerDoctorStatus::Ok(detail) => {
                        format!(
                            "  {} {name}: configuration valid; {}",
                            "✓".truecolor(aqua_r, aqua_g, aqua_b),
                            detail
                        )
                    }
                    McpServerDoctorStatus::Warning(detail) => {
                        format!(
                            "  {} {name}: configuration warning; {}",
                            "!".truecolor(sky_r, sky_g, sky_b),
                            detail
                        )
                    }
                    McpServerDoctorStatus::Error(detail) => {
                        format!(
                            "  {} {name}: configuration invalid; {}",
                            "✗".truecolor(red_r, red_g, red_b),
                            detail
                        )
                    }
                };
                println!("{icon}");
                if !server.is_enabled() {
                    println!("      disabled; live health not checked");
                } else {
                    println!(
                        "      process/protocol/backend: not checked; `codewhale mcp validate` explicitly starts and initializes configured servers"
                    );
                }
            }
            if probes.should_probe_mcp() {
                println!();
                println!(
                    "  {} Live MCP probe enabled: starting enabled servers; backend tool health remains untested.",
                    "!".truecolor(sky_r, sky_g, sky_b)
                );
                match crate::mcp::McpPool::from_config_path_with_workspace_and_plugins(
                    &mcp_config_path,
                    workspace,
                    std::sync::Arc::new(plugins.clone()),
                ) {
                    Ok(mut pool) => {
                        let errors = pool.connect_all().await;
                        let failed = errors
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .collect::<std::collections::BTreeSet<_>>();
                        for (name, server) in &cfg.servers {
                            if !server.is_enabled() {
                                continue;
                            }
                            if failed.contains(name.as_str()) {
                                println!(
                                    "      {} {name}: process/protocol unreachable; error details omitted",
                                    "✗".truecolor(red_r, red_g, red_b)
                                );
                            } else {
                                println!(
                                    "      {} {name}: process reachable and protocol initialized; backend tool health not checked",
                                    "✓".truecolor(aqua_r, aqua_g, aqua_b)
                                );
                            }
                        }
                    }
                    Err(_) => println!(
                        "      {} live MCP probe could not load merged configuration; details omitted",
                        "✗".truecolor(red_r, red_g, red_b)
                    ),
                }
            } else {
                println!(
                    "    Use codewhale doctor --probe-mcp to opt in to live process/protocol checks; it may start configured servers."
                );
            }
        }
        Err(_) => {
            println!(
                "  {} MCP configuration could not be loaded; details omitted",
                "✗".truecolor(red_r, red_g, red_b)
            );
        }
    }

    // Skills configuration
    println!();
    println!("{}", "Skills:".bold());
    let global_skills_dir = config.skills_dir();
    let agents_skills_dir = workspace.join(".agents").join("skills");
    let local_skills_dir = workspace.join("skills");
    let agents_global_skills_dir = crate::skills::agents_global_skills_dir();
    // #432: cross-tool skill discovery dirs. Presence is reported here
    // even though they sit lower in the precedence chain so users can
    // see at a glance whether a `.opencode/skills/`, `.claude/skills/`,
    // `.cursor/skills/`, or global agentskills.io directory is contributing
    // to the merged catalogue.
    let opencode_skills_dir = workspace.join(".opencode").join("skills");
    let claude_skills_dir = workspace.join(".claude").join("skills");
    let selected_skills_dir = if agents_skills_dir.exists() {
        agents_skills_dir.clone()
    } else if local_skills_dir.exists() {
        local_skills_dir.clone()
    } else if config.skills_dir.is_none()
        && let Some(global_agents) = agents_global_skills_dir.as_ref()
        && global_agents.exists()
    {
        global_agents.clone()
    } else {
        global_skills_dir.clone()
    };

    let describe_dir = |dir: &Path| -> usize {
        std::fs::read_dir(dir)
            .map(|entries| entries.filter_map(std::result::Result::ok).count())
            .unwrap_or(0)
    };

    if local_skills_dir.exists() {
        println!(
            "  {} local skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&local_skills_dir),
            describe_dir(&local_skills_dir)
        );
    } else {
        println!(
            "  {} local skills dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&local_skills_dir)
        );
    }

    if agents_skills_dir.exists() {
        println!(
            "  {} .agents skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&agents_skills_dir),
            describe_dir(&agents_skills_dir)
        );
    } else {
        println!(
            "  {} .agents skills dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&agents_skills_dir)
        );
    }

    if let Some(agents_global_skills_dir) = agents_global_skills_dir.as_ref() {
        if agents_global_skills_dir.exists() {
            println!(
                "  {} global .agents skills dir found at {} ({} items)",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                crate::utils::display_path(agents_global_skills_dir),
                describe_dir(agents_global_skills_dir)
            );
        } else {
            println!(
                "  {} global .agents skills dir not found at {}",
                "·".dimmed(),
                crate::utils::display_path(agents_global_skills_dir)
            );
        }
    }

    if global_skills_dir.exists() {
        println!(
            "  {} global skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&global_skills_dir),
            describe_dir(&global_skills_dir)
        );
    } else {
        println!(
            "  {} global skills dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&global_skills_dir)
        );
    }

    // #432: only print interop dirs when they're populated — empty
    // .opencode/.claude folders are common and would just clutter
    // the report with false-positive "absent" lines.
    if opencode_skills_dir.exists() {
        println!(
            "  {} .opencode skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&opencode_skills_dir),
            describe_dir(&opencode_skills_dir)
        );
    }
    if claude_skills_dir.exists() {
        println!(
            "  {} .claude skills dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&claude_skills_dir),
            describe_dir(&claude_skills_dir)
        );
    }

    println!(
        "  {} selected skills dir: {}",
        "·".dimmed(),
        crate::utils::display_path(&selected_skills_dir)
    );
    if !agents_skills_dir.exists()
        && !local_skills_dir.exists()
        && !agents_global_skills_dir
            .as_ref()
            .is_some_and(|dir| dir.exists())
        && !global_skills_dir.exists()
    {
        println!("    Run `codewhale setup --skills` (or add --local for ./skills).");
    }

    // Tools directory
    println!();
    println!("{}", "Tools:".bold());
    let tools_dir = default_tools_dir();
    if tools_dir.exists() {
        let count = count_dir_entries(&tools_dir);
        println!(
            "  {} tools dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&tools_dir),
            count
        );
    } else {
        println!(
            "  {} tools dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&tools_dir)
        );
        println!("    Run `codewhale setup --tools` to scaffold a starter dir.");
    }

    // Plugins directory
    println!();
    println!("{}", "Plugins:".bold());
    let plugins_dir = default_plugins_dir();
    if plugins_dir.exists() {
        let count = count_dir_entries(&plugins_dir);
        println!(
            "  {} plugins dir found at {} ({} items)",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            crate::utils::display_path(&plugins_dir),
            count
        );
    } else {
        println!(
            "  {} plugins dir not found at {}",
            "·".dimmed(),
            crate::utils::display_path(&plugins_dir)
        );
        println!("    Run `codewhale setup --plugins` to scaffold a starter dir.");
    }

    // Storage surfaces (#422 / #440 / #500)
    println!();
    println!("{}", "Storage:".bold());
    if let Some(spillover_root) = crate::tools::truncate::spillover_root() {
        let (present, count) = if spillover_root.is_dir() {
            (true, count_dir_entries(&spillover_root))
        } else {
            (false, 0)
        };
        if present {
            println!(
                "  {} tool-output spillover at {} ({} file{})",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                crate::utils::display_path(&spillover_root),
                count,
                if count == 1 { "" } else { "s" }
            );
        } else {
            println!(
                "  {} tool-output spillover dir not yet created at {}",
                "·".dimmed(),
                crate::utils::display_path(&spillover_root)
            );
        }
    }
    let stash = crate::composer_stash::diagnostic_stash_report();
    if let Some(stash_path) = stash.path.as_ref() {
        if let Some(error) = stash.error.as_deref() {
            println!(
                "  {} composer stash was not inspected at {}: {error}",
                "!".truecolor(sky_r, sky_g, sky_b),
                crate::utils::display_path(stash_path),
            );
        } else if stash.present {
            println!(
                "  {} composer stash at {} ({} parked draft{})",
                "✓".truecolor(aqua_r, aqua_g, aqua_b),
                crate::utils::display_path(stash_path),
                stash.count,
                if stash.count == 1 { "" } else { "s" }
            );
        } else {
            println!(
                "  {} composer stash empty (Ctrl+G or Ctrl+S in the composer to park a draft)",
                "·".dimmed()
            );
        }
    } else if let Some(error) = stash.error.as_deref() {
        println!(
            "  {} composer stash was not inspected: {error}",
            "!".truecolor(sky_r, sky_g, sky_b),
        );
    }

    // Tool dependencies — probe external binaries that individual
    // tools rely on (Python for code_execution, pdftotext for PDF
    // reading) so users see explicit ✓/✗ rather than the tool failing
    // at execution time with "program not found". New in v0.8.31.
    println!();
    println!("{}", "Tool Dependencies:".bold());

    match crate::dependencies::resolve_python_interpreter() {
        Some(name) => println!(
            "  {} Python: {} → code_execution tool registered",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            name
        ),
        None => {
            println!(
                "  {} Python: not found (tried {:?})",
                "✗".truecolor(red_r, red_g, red_b),
                crate::dependencies::PYTHON_CANDIDATES,
            );
            println!("    code_execution tool is NOT advertised to the model on this install.");
            println!("    Install Python 3 and ensure one of those names is on PATH:");
            match std::env::consts::OS {
                "macos" => {
                    println!("      brew install python@3.12   (or download from python.org)")
                }
                "linux" => println!(
                    "      sudo apt install python3    (Debian/Ubuntu) — or your distro's equivalent"
                ),
                "windows" => {
                    println!("      winget install Python.Python.3   (or download from python.org)")
                }
                other => println!("      install Python 3 for {other} from python.org"),
            }
        }
    }

    match crate::dependencies::resolve_node() {
        Some(_) => println!(
            "  {} Node.js: present → js_execution tool registered",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
        ),
        None => {
            println!(
                "  {} Node.js: not found (tried `node`)",
                "✗".truecolor(red_r, red_g, red_b),
            );
            println!("    js_execution tool is NOT advertised to the model on this install.");
            println!("    Install Node 18+ and ensure `node` is on PATH:");
            match std::env::consts::OS {
                "macos" => println!("      brew install node   (or download from nodejs.org)"),
                "linux" => println!(
                    "      sudo apt install nodejs    (Debian/Ubuntu) — or your distro's equivalent"
                ),
                "windows" => {
                    println!("      winget install OpenJS.NodeJS   (or download from nodejs.org)")
                }
                other => println!("      install Node.js for {other} from nodejs.org"),
            }
        }
    }

    match crate::dependencies::resolve_pandoc() {
        Some(_) => println!(
            "  {} pandoc: present → pandoc_convert tool registered",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
        ),
        None => {
            println!("  {} pandoc: not found (optional)", "·".dimmed(),);
            println!(
                "    pandoc_convert tool is NOT advertised to the model. Install pandoc to enable:"
            );
            match std::env::consts::OS {
                "macos" => println!("      brew install pandoc"),
                "linux" => println!(
                    "      sudo apt install pandoc    (Debian/Ubuntu) — or your distro's equivalent"
                ),
                "windows" => {
                    println!("      winget install JohnMacFarlane.Pandoc")
                }
                other => println!("      install pandoc for {other} from pandoc.org"),
            }
        }
    }

    match crate::dependencies::resolve_tesseract() {
        Some(_) => {
            if cfg!(target_os = "macos") {
                println!(
                    "  {} OCR: macOS Vision + tesseract available → image_ocr/read_file screenshot OCR enabled",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
            } else {
                println!(
                    "  {} tesseract: present → image_ocr/read_file screenshot OCR enabled",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
            }
        }
        None => {
            if cfg!(target_os = "macos") {
                println!(
                    "  {} OCR: macOS Vision available → image_ocr/read_file screenshot OCR enabled",
                    "✓".truecolor(aqua_r, aqua_g, aqua_b),
                );
                println!(
                    "    tesseract not found (optional; install only for alternate OCR packs)."
                );
            } else {
                println!("  {} tesseract: not found (optional)", "·".dimmed(),);
                println!(
                    "    image_ocr tool is NOT advertised to the model. Install tesseract to enable:"
                );
                match std::env::consts::OS {
                    "macos" => println!("      brew install tesseract"),
                    "linux" => println!(
                        "      sudo apt install tesseract-ocr    (Debian/Ubuntu) — or your distro's equivalent"
                    ),
                    "windows" => println!("      winget install UB-Mannheim.TesseractOCR"),
                    other => {
                        println!("      install tesseract for {other} from tesseract-ocr.github.io")
                    }
                }
            }
        }
    }

    // PDF text extraction is an optional integration. Codewhale itself stays
    // a single required executable; file and web tools report a typed
    // failed `binary_unavailable` result when Poppler is not installed.
    match crate::dependencies::resolve_pdftotext() {
        Some(_) => println!(
            "  {} pdftotext: available → PDF text extraction enabled",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
        ),
        None => {
            println!(
                "  {} pdftotext: not found (optional; PDF text reads fail as `binary_unavailable`)",
                "·".dimmed(),
            );
            match std::env::consts::OS {
                "macos" => println!("    Install via: brew install poppler"),
                "linux" => {
                    println!("    Install via: sudo apt install poppler-utils   (Debian/Ubuntu)")
                }
                "windows" => println!(
                    "    Install Poppler for Windows from https://blog.alivate.com.au/poppler-windows/"
                ),
                _ => {}
            }
        }
    }

    // Terminal-quirk overrides currently active. Mirrors the env
    // signals checked by `Settings::apply_env_overrides` so users
    // can see at a glance which a11y/compat overrides fired.
    println!();
    println!("{}", "Terminal Quirks:".bold());
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term_program_lc = term_program.to_ascii_lowercase();
    let mut any_quirk = false;
    if matches!(term_program.as_str(), "vscode" | "ghostty") {
        println!(
            "  {} TERM_PROGRAM={} → low_motion + fancy_animations=false (auto)",
            "•".truecolor(sky_r, sky_g, sky_b),
            term_program
        );
        any_quirk = true;
    }
    if term_program == "Termius"
        || std::env::var_os("SSH_CLIENT").is_some_and(|v| !v.is_empty())
        || std::env::var_os("SSH_TTY").is_some_and(|v| !v.is_empty())
    {
        println!(
            "  {} SSH/Termius session → low_motion + fancy_animations=false (auto, #1433)",
            "•".truecolor(sky_r, sky_g, sky_b)
        );
        any_quirk = true;
    }
    if term_program_lc.contains("ptyxis")
        || std::env::var_os("PTYXIS_VERSION").is_some_and(|v| !v.is_empty())
    {
        println!(
            "  {} Ptyxis detected → synchronized_output=off (auto, v0.8.31)",
            "•".truecolor(sky_r, sky_g, sky_b)
        );
        any_quirk = true;
    }
    if crate::settings::detected_legacy_windows_console_host() {
        println!(
            "  {} legacy Windows console host → low_motion + fancy_animations=false + bracketed_paste=false + synchronized_output=off (auto)",
            "•".truecolor(sky_r, sky_g, sky_b)
        );
        any_quirk = true;
    }
    if !any_quirk {
        println!(
            "  {} no env-driven terminal-quirk overrides active",
            "·".dimmed()
        );
    }

    // Platform and sandbox checks
    println!();
    println!("{}", "Platform:".bold());
    println!("  OS: {}", std::env::consts::OS);
    println!("  Arch: {}", std::env::consts::ARCH);

    let sandbox = crate::sandbox::get_platform_sandbox_with_bwrap_preference(
        config.prefer_bwrap.unwrap_or(false),
    );
    if let Some(kind) = sandbox {
        println!(
            "  {} sandbox available: {}",
            "✓".truecolor(aqua_r, aqua_g, aqua_b),
            kind
        );
    } else {
        println!(
            "  {} sandbox not available (commands run best-effort)",
            "!".truecolor(sky_r, sky_g, sky_b)
        );
    }

    println!();
    println!(
        "{}",
        "All checks complete!"
            .truecolor(aqua_r, aqua_g, aqua_b)
            .bold()
    );
}

pub(crate) const DOCTOR_LEGACY_STATE_ITEMS: &[&str] = &[
    "sessions",
    "tasks",
    "skills",
    "slop_ledger",
    "trophies",
    "catalog",
    "review-receipts",
    "config.toml",
    "settings.toml",
    "mcp.json",
];
pub(crate) const DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT: usize = 20;
pub(crate) const DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoctorLegacyStateStatus {
    PrimaryOnly,
    LegacyOnly,
    Both,
    Absent,
}

impl DoctorLegacyStateStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryOnly => "primary_only",
            Self::LegacyOnly => "legacy_only",
            Self::Both => "both",
            Self::Absent => "absent",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DoctorLegacyStateEntry {
    pub(crate) name: &'static str,
    pub(crate) primary_path: PathBuf,
    pub(crate) legacy_path: PathBuf,
    pub(crate) primary_present: bool,
    pub(crate) legacy_present: bool,
    pub(crate) status: DoctorLegacyStateStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoctorSessionRecoveryStatus {
    Isolated,
    NoLegacySessions,
    MigrationPending,
    MigrationIncomplete,
    MigrationComplete,
    ScanFailed,
}

impl DoctorSessionRecoveryStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::NoLegacySessions => "no_legacy_sessions",
            Self::MigrationPending => "migration_pending",
            Self::MigrationIncomplete => "migration_incomplete",
            Self::MigrationComplete => "migration_complete",
            Self::ScanFailed => "scan_failed",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DoctorRecoverableSessionEntry {
    pub(crate) name: PathBuf,
    pub(crate) source_path: PathBuf,
    pub(crate) destination_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct DoctorSessionRecoveryReport {
    pub(crate) status: DoctorSessionRecoveryStatus,
    pub(crate) primary_sessions_path: PathBuf,
    pub(crate) legacy_sessions_path: PathBuf,
    pub(crate) codewhale_home_is_explicit: bool,
    pub(crate) legacy_session_file_count: usize,
    pub(crate) already_present_file_count: usize,
    pub(crate) recoverable_file_count: usize,
    /// Bounded filename/path sample; the total is `recoverable_file_count`.
    pub(crate) recoverable: Vec<DoctorRecoverableSessionEntry>,
    pub(crate) error: Option<String>,
}

impl DoctorSessionRecoveryReport {
    pub(crate) fn needs_attention(&self) -> bool {
        matches!(
            self.status,
            DoctorSessionRecoveryStatus::MigrationPending
                | DoctorSessionRecoveryStatus::MigrationIncomplete
                | DoctorSessionRecoveryStatus::ScanFailed
        )
    }
}

pub(crate) fn doctor_legacy_state_status(
    primary_present: bool,
    legacy_present: bool,
) -> DoctorLegacyStateStatus {
    match (primary_present, legacy_present) {
        (true, false) => DoctorLegacyStateStatus::PrimaryOnly,
        (false, true) => DoctorLegacyStateStatus::LegacyOnly,
        (true, true) => DoctorLegacyStateStatus::Both,
        (false, false) => DoctorLegacyStateStatus::Absent,
    }
}

pub(crate) fn doctor_state_roots() -> (PathBuf, PathBuf) {
    let code_home =
        codewhale_config::codewhale_home().unwrap_or_else(|_| PathBuf::from("~/.codewhale"));
    let legacy_home = if codewhale_config::codewhale_home_is_explicit() {
        code_home.join(codewhale_config::LEGACY_APP_DIR)
    } else {
        codewhale_config::legacy_deepseek_home().unwrap_or_else(|_| PathBuf::from("~/.deepseek"))
    };
    (code_home, legacy_home)
}

pub(crate) fn doctor_legacy_state_report(
    primary_root: &Path,
    legacy_root: &Path,
) -> Vec<DoctorLegacyStateEntry> {
    DOCTOR_LEGACY_STATE_ITEMS
        .iter()
        .copied()
        .map(|name| {
            let primary_path = primary_root.join(name);
            let legacy_path = legacy_root.join(name);
            let primary_present = primary_path.exists();
            let legacy_present = legacy_path.exists();
            let status = doctor_legacy_state_status(primary_present, legacy_present);
            DoctorLegacyStateEntry {
                name,
                primary_path,
                legacy_path,
                primary_present,
                legacy_present,
                status,
            }
        })
        .collect()
}

/// Compare legacy and primary session filenames without opening session files.
///
/// This is deliberately separate from `SessionManager::default_location()`:
/// constructing the manager can trigger the additive legacy migration, while
/// doctor must remain a read-only diagnostic. Session history is stored as
/// top-level JSON files. Directories (including `checkpoints`) and symlinks
/// observed during the scan are ignored, so the diagnostic does not
/// intentionally traverse checkpoint internals or link targets. These checks
/// are best-effort observations, not a race-free no-follow guarantee.
/// A matching filename is only a regular-file counterpart check: doctor does
/// not parse or compare session descriptors.
pub(crate) fn doctor_session_recovery_report(
    primary_root: &Path,
    legacy_root: &Path,
    codewhale_home_is_explicit: bool,
) -> DoctorSessionRecoveryReport {
    let primary_sessions_path = primary_root.join("sessions");
    let legacy_sessions_path = legacy_root.join("sessions");
    let mut report = DoctorSessionRecoveryReport {
        status: DoctorSessionRecoveryStatus::NoLegacySessions,
        primary_sessions_path,
        legacy_sessions_path,
        codewhale_home_is_explicit,
        legacy_session_file_count: 0,
        already_present_file_count: 0,
        recoverable_file_count: 0,
        recoverable: Vec::new(),
        error: None,
    };

    if codewhale_home_is_explicit {
        report.status = DoctorSessionRecoveryStatus::Isolated;
        return report;
    }

    let legacy_root_is_present =
        match doctor_session_directory_is_safe(legacy_root, "legacy state root") {
            Ok(present) => present,
            Err(error) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(error);
                return report;
            }
        };
    if !legacy_root_is_present {
        return report;
    }
    if let Err(error) = doctor_session_directory_is_safe(primary_root, "primary state root") {
        report.status = DoctorSessionRecoveryStatus::ScanFailed;
        report.error = Some(error);
        return report;
    }

    let legacy_sessions_are_present = match doctor_session_directory_is_safe(
        &report.legacy_sessions_path,
        "legacy sessions root",
    ) {
        Ok(present) => present,
        Err(error) => {
            report.status = DoctorSessionRecoveryStatus::ScanFailed;
            report.error = Some(error);
            return report;
        }
    };
    if !legacy_sessions_are_present {
        return report;
    }
    let primary_sessions_are_present = match doctor_session_directory_is_safe(
        &report.primary_sessions_path,
        "primary sessions root",
    ) {
        Ok(present) => present,
        Err(error) => {
            report.status = DoctorSessionRecoveryStatus::ScanFailed;
            report.error = Some(error);
            return report;
        }
    };

    let entries = match std::fs::read_dir(&report.legacy_sessions_path) {
        Ok(entries) => entries,
        Err(err) => {
            report.status = DoctorSessionRecoveryStatus::ScanFailed;
            report.error = Some(format!(
                "could not inspect legacy session filenames at {}: {err}",
                crate::utils::display_path(&report.legacy_sessions_path)
            ));
            return report;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(format!(
                    "could not inspect an entry under {}: {err}",
                    crate::utils::display_path(&report.legacy_sessions_path)
                ));
                return report;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(format!(
                    "could not inspect legacy session entry metadata under {}: {err}",
                    crate::utils::display_path(&report.legacy_sessions_path)
                ));
                return report;
            }
        };
        if !file_type.is_file() || entry.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }

        report.legacy_session_file_count += 1;
        let name = PathBuf::from(entry.file_name());
        let destination_path = report.primary_sessions_path.join(&name);
        match std::fs::symlink_metadata(&destination_path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                report.already_present_file_count += 1;
            }
            Ok(metadata) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                let shape = if metadata.file_type().is_symlink() {
                    "destination session entry is a symlink"
                } else {
                    "destination session entry is not a regular file"
                };
                report.error = Some(format!(
                    "could not inspect destination session metadata at {}: {shape}",
                    crate::utils::display_path(&destination_path)
                ));
                return report;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                report.recoverable_file_count += 1;
                record_doctor_recoverable_session(
                    &mut report.recoverable,
                    DoctorRecoverableSessionEntry {
                        source_path: entry.path(),
                        destination_path,
                        name,
                    },
                );
            }
            Err(err) => {
                report.status = DoctorSessionRecoveryStatus::ScanFailed;
                report.error = Some(format!(
                    "could not inspect destination metadata at {}: {err}",
                    crate::utils::display_path(&destination_path)
                ));
                return report;
            }
        }
    }

    report.status = if report.legacy_session_file_count == 0 {
        DoctorSessionRecoveryStatus::NoLegacySessions
    } else if report.recoverable_file_count == 0 {
        DoctorSessionRecoveryStatus::MigrationComplete
    } else if primary_sessions_are_present {
        DoctorSessionRecoveryStatus::MigrationIncomplete
    } else {
        DoctorSessionRecoveryStatus::MigrationPending
    };
    report
}

/// Validate a session-state directory from observed metadata.
///
/// `doctor` only compares top-level filenames. It rejects a state-root or
/// sessions-root symlink observed during inspection rather than using it for a
/// recovery suggestion. This is a best-effort observation, not a race-free
/// no-follow guarantee. Missing paths are normal on a fresh install and are
/// reported as `false`.
pub(crate) fn doctor_session_directory_is_safe(
    path: &Path,
    label: &str,
) -> std::result::Result<bool, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "could not inspect {label} at {}: {error}",
                crate::utils::display_path(path)
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "could not inspect {label} at {}: path is a symlink",
            crate::utils::display_path(path)
        ));
    }
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "could not inspect {label} at {}: path is not a directory",
            crate::utils::display_path(path)
        ));
    }
    Ok(true)
}

/// Keep the report bounded while preserving a deterministic, lexical sample.
/// `read_dir` order is platform- and filesystem-dependent, so retaining the
/// first entries encountered would make the JSON and human receipts drift.
pub(crate) fn record_doctor_recoverable_session(
    recoverable: &mut Vec<DoctorRecoverableSessionEntry>,
    entry: DoctorRecoverableSessionEntry,
) {
    let insert_at = recoverable
        .binary_search_by(|existing| existing.name.cmp(&entry.name))
        .unwrap_or_else(|index| index);
    if recoverable.len() == DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT
        && insert_at == recoverable.len()
    {
        return;
    }
    recoverable.insert(insert_at, entry);
    if recoverable.len() > DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT {
        recoverable.pop();
    }
}

pub(crate) fn legacy_state_needs_attention(entry: &DoctorLegacyStateEntry) -> bool {
    entry.name != "sessions"
        && matches!(
            entry.status,
            DoctorLegacyStateStatus::LegacyOnly | DoctorLegacyStateStatus::Both
        )
}

pub(crate) fn print_doctor_legacy_state_report(
    report: &[DoctorLegacyStateEntry],
    session_recovery: &DoctorSessionRecoveryReport,
    ok_rgb: (u8, u8, u8),
    warn_rgb: (u8, u8, u8),
) {
    use colored::Colorize;

    let attention: Vec<_> = report
        .iter()
        .filter(|entry| legacy_state_needs_attention(entry))
        .collect();
    if attention.is_empty()
        && !session_recovery.needs_attention()
        && session_recovery.status != DoctorSessionRecoveryStatus::Isolated
    {
        println!(
            "  {} legacy state: no known .deepseek entries need migration",
            "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
        );
    } else if !attention.is_empty() {
        println!(
            "  {} legacy state needs review:",
            "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
        );
        for entry in attention {
            match entry.status {
                DoctorLegacyStateStatus::LegacyOnly => {
                    println!(
                        "    {} {} exists but {} is missing",
                        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
                        crate::utils::display_path(&entry.legacy_path),
                        crate::utils::display_path(&entry.primary_path),
                    );
                }
                DoctorLegacyStateStatus::Both => {
                    println!(
                        "    {} {} exists alongside primary {}; legacy data may still need review",
                        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
                        crate::utils::display_path(&entry.legacy_path),
                        crate::utils::display_path(&entry.primary_path),
                    );
                }
                DoctorLegacyStateStatus::PrimaryOnly | DoctorLegacyStateStatus::Absent => {}
            }
        }
        println!(
            "    Start Codewhale once to trigger safe migration where available, then rerun `codewhale doctor`."
        );
    }

    print_doctor_session_recovery_report(session_recovery, ok_rgb, warn_rgb);
}

pub(crate) fn print_doctor_session_recovery_report(
    report: &DoctorSessionRecoveryReport,
    ok_rgb: (u8, u8, u8),
    warn_rgb: (u8, u8, u8),
) {
    use colored::Colorize;

    match report.status {
        DoctorSessionRecoveryStatus::Isolated => {
            println!(
                "  {} legacy sessions: ambient ~/.deepseek/sessions was not inspected because CODEWHALE_HOME is set",
                "·".dimmed()
            );
            println!(
                "    This preserves the explicit home boundary. To inspect the default home, use a separate shell with CODEWHALE_HOME unset and rerun `codewhale doctor`."
            );
        }
        DoctorSessionRecoveryStatus::NoLegacySessions => {
            println!(
                "  {} legacy sessions: no top-level session JSON files found",
                "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
            );
        }
        DoctorSessionRecoveryStatus::MigrationComplete => {
            println!(
                "  {} legacy sessions: all {} filename(s) have regular-file counterparts under {}; descriptor contents were not compared and legacy originals remain preserved",
                "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2),
                report.legacy_session_file_count,
                crate::utils::display_path(&report.primary_sessions_path),
            );
        }
        DoctorSessionRecoveryStatus::MigrationPending
        | DoctorSessionRecoveryStatus::MigrationIncomplete => {
            let label = if report.status == DoctorSessionRecoveryStatus::MigrationIncomplete {
                "migration is incomplete"
            } else {
                "migration has not completed"
            };
            println!(
                "  {} legacy sessions: {label}; {} recoverable file(s) are absent from {}",
                "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
                report.recoverable_file_count,
                crate::utils::display_path(&report.primary_sessions_path),
            );
            for entry in report
                .recoverable
                .iter()
                .take(DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT)
            {
                println!(
                    "    {} {} -> {}",
                    "·".dimmed(),
                    crate::utils::display_path(&entry.source_path),
                    crate::utils::display_path(&entry.destination_path),
                );
            }
            if report.recoverable_file_count > DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT {
                println!(
                    "    · {} more filename(s); `codewhale doctor --json` includes a bounded metadata-only sample",
                    report.recoverable_file_count - DOCTOR_SESSION_RECOVERY_HUMAN_SAMPLE_LIMIT
                );
            }
            println!("    Safe recovery:");
            println!(
                "      1. Back up {} and {} (if present).",
                crate::utils::display_path(&report.legacy_sessions_path),
                crate::utils::display_path(&report.primary_sessions_path),
            );
            println!(
                "      2. Close other Codewhale processes, then run `codewhale sessions`; migration adds only missing files, never overwrites primary files, and leaves legacy originals in place."
            );
            println!(
                "      3. Rerun `codewhale doctor`. If filenames remain, keep both backups and report only the listed source/destination names."
            );
        }
        DoctorSessionRecoveryStatus::ScanFailed => {
            println!(
                "  {} legacy sessions: recovery diagnostic could not complete",
                "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
            );
            if let Some(error) = report.error.as_deref() {
                println!("    {error}");
            }
            println!(
                "    Keep both session directories unchanged, back them up, fix path permissions or shape, and rerun `codewhale doctor` before attempting migration."
            );
        }
    }
    if report.status != DoctorSessionRecoveryStatus::Isolated {
        println!(
            "    Doctor inspected filenames and filesystem metadata only; it did not read chat contents, traverse checkpoints, or modify session files."
        );
    }
}

pub(crate) fn doctor_session_recovery_json(
    report: &DoctorSessionRecoveryReport,
) -> serde_json::Value {
    use serde_json::json;

    let recoverable: Vec<_> = report
        .recoverable
        .iter()
        .take(DOCTOR_SESSION_RECOVERY_JSON_SAMPLE_LIMIT)
        .map(|entry| {
            json!({
                "name": entry.name.display().to_string(),
                "source_path": entry.source_path.display().to_string(),
                "destination_path": entry.destination_path.display().to_string(),
            })
        })
        .collect();

    json!({
        "status": report.status.as_str(),
        "needs_attention": report.needs_attention(),
        "read_only": true,
        "chat_contents_read": false,
        "checkpoint_internals_scanned": false,
        "session_descriptors_compared": false,
        "counterpart_check": "top_level_filename_and_regular_file_only",
        "codewhale_home_is_explicit": report.codewhale_home_is_explicit,
        "legacy_sessions_path": report.legacy_sessions_path.display().to_string(),
        "primary_sessions_path": report.primary_sessions_path.display().to_string(),
        "legacy_session_file_count": report.legacy_session_file_count,
        "already_present_file_count": report.already_present_file_count,
        "recoverable_file_count": report.recoverable_file_count,
        "recoverable_files": recoverable,
        "recoverable_files_truncated": report.recoverable_file_count > report.recoverable.len(),
        "error": report.error,
        "recovery_command": if report.needs_attention() && report.status != DoctorSessionRecoveryStatus::ScanFailed {
            Some("codewhale sessions")
        } else {
            None
        },
    })
}

pub(crate) fn doctor_legacy_state_json(
    primary_root: &Path,
    legacy_root: &Path,
    report: &[DoctorLegacyStateEntry],
    session_recovery: &DoctorSessionRecoveryReport,
) -> serde_json::Value {
    use serde_json::json;

    let legacy_only = report
        .iter()
        .filter(|entry| entry.status == DoctorLegacyStateStatus::LegacyOnly)
        .count();
    let both = report
        .iter()
        .filter(|entry| entry.status == DoctorLegacyStateStatus::Both)
        .count();
    let entries: Vec<_> = report
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "primary_path": entry.primary_path.display().to_string(),
                "legacy_path": entry.legacy_path.display().to_string(),
                "primary_present": entry.primary_present,
                "legacy_present": entry.legacy_present,
                "status": entry.status.as_str(),
            })
        })
        .collect();

    json!({
        "primary_root": primary_root.display().to_string(),
        "legacy_root": legacy_root.display().to_string(),
        "needs_attention": report.iter().any(legacy_state_needs_attention) || session_recovery.needs_attention(),
        "legacy_only_count": legacy_only,
        "dual_present_count": both,
        "session_recovery": doctor_session_recovery_json(session_recovery),
        "entries": entries,
    })
}

pub(crate) fn doctor_setup_state(
    config: &Config,
    workspace: &Path,
) -> (codewhale_config::SetupState, &'static str) {
    if let Ok(Some(state)) = codewhale_config::SetupState::load() {
        return (state, "persisted");
    }

    (
        codewhale_config::SetupState::derive_inherited(&doctor_inherited_setup_facts(
            config, workspace,
        )),
        "derived",
    )
}

pub(crate) fn doctor_inherited_setup_facts(
    config: &Config,
    workspace: &Path,
) -> codewhale_config::InheritedConfigFacts {
    let user_constitution = codewhale_config::UserConstitution::load().ok();
    let user_constitution_validity = user_constitution.as_ref().map_or(
        codewhale_config::ConstitutionValidity::Unknown,
        codewhale_config::UserConstitutionLoad::validity,
    );
    let has_user_constitution = user_constitution
        .as_ref()
        .is_some_and(|loaded| !matches!(loaded, codewhale_config::UserConstitutionLoad::Missing));
    let has_expert_override = codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join(Path::new(crate::prompts::CONSTITUTION_OVERRIDE_FILE)))
        .is_some_and(|path| path.exists());

    codewhale_config::InheritedConfigFacts {
        language: None,
        has_provider_route: !config.default_model().trim().is_empty(),
        has_credentials_or_local_runtime: doctor_has_credentials_or_local_runtime(config),
        trust_chosen: !crate::tui::onboarding::needs_trust(workspace),
        has_expert_override,
        has_user_constitution,
        user_constitution_validity,
    }
}

pub(crate) fn doctor_has_credentials_or_local_runtime(config: &Config) -> bool {
    resolve_credential_diagnostic(config)
        .availability
        .certifies_ready()
}

pub(crate) fn print_doctor_setup_report(
    config: &Config,
    workspace: &Path,
    state: &codewhale_config::SetupState,
    source: &str,
    ok_rgb: (u8, u8, u8),
    warn_rgb: (u8, u8, u8),
) {
    use colored::Colorize;

    let credential = resolve_credential_diagnostic(config);
    // Setup completion is persisted independently from credential probing.
    // Ordinary doctor deliberately does not read environment values or the
    // durable secret store, so `not_probed` must not erase a completed lane.
    let first_run_ready = state.first_run_ready();
    let update_ready = state.update_ready(crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION);
    let operate_ready = state.operate_ready();
    let first_run_icon = if first_run_ready {
        "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
    } else {
        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
    };
    let update_icon = if update_ready {
        "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
    } else {
        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
    };
    let operate_icon = if operate_ready {
        "✓".truecolor(ok_rgb.0, ok_rgb.1, ok_rgb.2)
    } else {
        "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2)
    };

    println!();
    println!("{}", "Setup State:".bold());
    println!("  · source: {source}");
    println!(
        "  · credential: source={}, availability={}",
        doctor_api_key_source_label(credential.source),
        credential.availability.label()
    );
    println!(
        "  {first_run_icon} first-run: {}",
        doctor_ready_label(first_run_ready)
    );
    println!(
        "  {update_icon} update checkpoint {}: {}",
        crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
        doctor_ready_label(update_ready)
    );
    println!(
        "  {operate_icon} operate/fleet: {}",
        doctor_ready_label(operate_ready)
    );
    println!(
        "  · constitution autonomy: {} (guidance only)",
        doctor_constitution_autonomy_preference_id()
    );
    println!(
        "  · runtime posture: {}",
        doctor_runtime_posture_line(config, workspace)
    );
    println!(
        "  · lifecycle outbox: {}",
        doctor_lifecycle_outbox_posture_line(config)
    );
    println!(
        "  · control socket: {}",
        doctor_control_socket_posture_line(config)
    );
    let consistency = doctor_setup_consistency(state, source);
    if consistency["status"] == "inconsistent" {
        let issues = consistency["issues"]
            .as_array()
            .map(|issues| {
                issues
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        println!(
            "  {} consistency: half-applied setup detected ({issues}) — {}",
            "!".truecolor(warn_rgb.0, warn_rgb.1, warn_rgb.2),
            consistency["repair"].as_str().unwrap_or("/setup"),
        );
    }
    println!(
        "  · next actions: /constitution (standing law), /setup report (readiness), /setup provider or /provider setup <name> (provider credentials), /model (route), /config (runtime posture), /setup fleet (Operate/Fleet readiness), /fleet setup (explicit profile authoring), /setup hotbar (optional shortcuts), /setup tools (Tools/MCP readiness), /setup remote (remote runtime on-ramp), /setup persistence (path review)"
    );
    for step in codewhale_config::SetupStep::ALL {
        let entry = state.steps.get(&step);
        let required = entry.is_some_and(|entry| entry.required);
        let version = entry.and_then(|entry| entry.version.as_deref());
        let result = entry.and_then(|entry| entry.result.as_deref());
        let required_label = if required { "required" } else { "optional" };
        let version_label = version.unwrap_or("unversioned");
        let result_label = result.unwrap_or("no result");
        println!(
            "    · {}: {} ({required_label}, {version_label}, {result_label})",
            setup_step_id(step),
            setup_status_id(state.status(step))
        );
    }
}

/// #5098: print every profile id that exists in more than one roster layer
/// so a personal/config edit that loses to project is visible without
/// opening `/fleet`.
pub(crate) fn print_doctor_fleet_roster_layers(config: &Config, workspace: &Path) {
    use colored::Colorize;

    let roster =
        crate::fleet::identity::load_effective_roster(&config.fleet_config(), workspace, None);
    println!();
    println!("{}", "Fleet roster layers:".bold());
    if let Some(error) = roster.load_error() {
        println!("  ! {error}");
        return;
    }
    let lines = roster.doctor_layer_lines();
    if lines.is_empty() {
        println!("  · no profile id is defined in more than one layer");
        return;
    }
    for line in lines {
        if let Some(layer) = line.strip_prefix("  ") {
            println!("      {layer}");
        } else {
            println!("  · {line}");
        }
    }
}

pub(crate) fn doctor_ready_label(ready: bool) -> &'static str {
    if ready { "ready" } else { "needs action" }
}

/// Detect half-applied setup persistence (#3410).
///
/// The setup transaction writes `constitution.json` and `setup_state.json`
/// together, so a persisted state that points at a user-global constitution
/// which is missing or unusable on disk means a write was interrupted or a
/// file was removed out-of-band. Stale `.tmp*` files in `$CODEWHALE_HOME`
/// are the other fingerprint of an interrupted atomic write.
pub(crate) fn doctor_setup_consistency(
    state: &codewhale_config::SetupState,
    source: &str,
) -> serde_json::Value {
    use serde_json::json;

    let mut issues: Vec<&'static str> = Vec::new();

    if source == "persisted"
        && matches!(
            state.constitution_source,
            codewhale_config::ConstitutionSource::UserGlobal
        )
    {
        match codewhale_config::UserConstitution::load() {
            Ok(codewhale_config::UserConstitutionLoad::Missing) => {
                issues.push("setup_state_points_at_missing_user_constitution");
            }
            Ok(codewhale_config::UserConstitutionLoad::Empty) => {
                issues.push("user_constitution_empty");
            }
            Ok(codewhale_config::UserConstitutionLoad::Invalid(_)) => {
                issues.push("user_constitution_invalid");
            }
            Ok(codewhale_config::UserConstitutionLoad::Unreadable(_)) | Err(_) => {
                issues.push("user_constitution_unreadable");
            }
            Ok(codewhale_config::UserConstitutionLoad::Loaded(_)) => {}
        }
    }

    if doctor_home_has_stale_setup_temp_files() {
        issues.push("stale_setup_temp_files_in_codewhale_home");
    }

    json!({
        "status": if issues.is_empty() { "consistent" } else { "inconsistent" },
        "issues": issues,
        "repair": "/constitution to rebuild standing law, /setup to re-run the checkpoint",
    })
}

pub(crate) fn doctor_home_has_stale_setup_temp_files() -> bool {
    let Ok(home) = codewhale_config::codewhale_home() else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(&home) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_name().to_string_lossy().starts_with(".tmp")
            && entry.file_type().is_ok_and(|kind| kind.is_file())
    })
}

pub(crate) fn doctor_constitution_autonomy_preference() -> codewhale_config::AutonomyPreference {
    codewhale_config::UserConstitution::load()
        .ok()
        .and_then(|load| {
            load.constitution()
                .map(|constitution| constitution.autonomy_preference)
        })
        .unwrap_or(codewhale_config::AutonomyPreference::Unspecified)
}

pub(crate) fn doctor_constitution_autonomy_preference_id() -> &'static str {
    autonomy_preference_id(doctor_constitution_autonomy_preference())
}

pub(crate) fn autonomy_preference_id(
    preference: codewhale_config::AutonomyPreference,
) -> &'static str {
    match preference {
        codewhale_config::AutonomyPreference::Unspecified => "unspecified",
        codewhale_config::AutonomyPreference::Cautious => "cautious",
        codewhale_config::AutonomyPreference::Balanced => "balanced",
        codewhale_config::AutonomyPreference::Autonomous => "autonomous",
    }
}

pub(crate) fn doctor_runtime_default_mode() -> (String, &'static str) {
    match crate::settings::Settings::load_read_only() {
        Ok(settings) => (settings.default_mode, "settings"),
        Err(_) => (crate::settings::Settings::default().default_mode, "default"),
    }
}

/// TUI settings posture used when `config.approval_policy` is unset.
/// Doctor must surface this separately so a saved Full Access baseline is not
/// misreported as the config default `approval_policy=on-request`.
pub(crate) fn doctor_runtime_permission_posture() -> (String, &'static str) {
    match crate::settings::Settings::load_read_only() {
        Ok(settings) => match settings.permission_posture {
            Some(posture) => (posture, "settings"),
            None => ("unset".to_string(), "default"),
        },
        Err(_) => ("unset".to_string(), "default"),
    }
}

pub(crate) fn doctor_runtime_posture_line(config: &Config, workspace: &Path) -> String {
    let (default_mode, default_mode_source) = doctor_runtime_default_mode();
    let (permission_posture, permission_posture_source) = doctor_runtime_permission_posture();
    let approval = config.approval_policy.as_deref().unwrap_or("on-request");
    let approval_source = if config.approval_policy.is_some() {
        "config"
    } else {
        "default"
    };
    let allow_shell = config.interactive_allow_shell();
    let allow_shell_source = if config.allow_shell.is_some() {
        "config"
    } else {
        "interactive default"
    };
    let sandbox = config.sandbox_mode.as_deref().unwrap_or("mode-derived");
    let sandbox_source = if config.sandbox_mode.is_some() {
        "config"
    } else {
        "default"
    };
    let network = config
        .network
        .as_ref()
        .map_or("prompt", |policy| policy.default.as_str());
    let network_source = if config.network.is_some() {
        "config"
    } else {
        "default"
    };
    let trust = if crate::tui::onboarding::needs_trust(workspace) {
        "workspace not elevated"
    } else {
        "workspace trusted"
    };
    let (telemetry_on, telemetry_source) = doctor_runtime_telemetry(config);
    let telemetry = if telemetry_on { "on" } else { "off" };

    format!(
        "default_mode={default_mode} ({default_mode_source}), permission_posture={permission_posture} ({permission_posture_source}), approval_policy={approval} ({approval_source}), allow_shell={allow_shell} ({allow_shell_source}), sandbox={sandbox} ({sandbox_source}), network.default={network} ({network_source}), telemetry={telemetry} ({telemetry_source}), trust={trust}"
    )
}

/// Observability posture row for the lifecycle outbox: the feature is opt-in
/// via `[lifecycle_outbox].path` (unset/empty = off, the default). Truth and
/// resilience theme: report the resolved state and, when enabled, the sink
/// path the writer appends to.
pub(crate) fn doctor_lifecycle_outbox_posture_line(config: &Config) -> String {
    let path = config
        .lifecycle_outbox
        .as_ref()
        .and_then(|outbox| outbox.path.as_deref())
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    if path.trim().is_empty() {
        return "lifecycle_outbox=off (default)".to_string();
    }
    format!("lifecycle_outbox=on (path: {path})")
}

/// Doctor posture for the per-session control socket, enabled via
/// `[control_socket].enabled` (false = off, the default). Report the
/// resolved state and, when enabled, where the socket appears for the
/// running session.
pub(crate) fn doctor_control_socket_posture_line(config: &Config) -> String {
    let enabled = config
        .control_socket
        .as_ref()
        .is_some_and(|socket| socket.enabled);
    if enabled {
        "control_socket=on (sessions/<id>/control.sock per running session)".to_string()
    } else {
        "control_socket=off (default)".to_string()
    }
}

/// Resolved telemetry consent and where it came from (#5441).
///
/// Telemetry ships ON by default, and no posture surface reported that — a
/// user who never opted in saw nothing saying "telemetry: on (default)".
/// Truth change only: the resolution itself is [`codewhale_config`]'s.
pub(crate) fn doctor_runtime_telemetry(config: &Config) -> (bool, &'static str) {
    let (on, source) = codewhale_config::resolved_telemetry_consent(config.telemetry);
    (on, source.as_str())
}

pub(crate) fn doctor_operate_fleet_report_json(
    config: &Config,
    workspace: &Path,
) -> serde_json::Value {
    use serde_json::json;

    let provider = config.api_provider();
    // Doctor reports configured routing posture only. In particular it must
    // never consume an external-file grant merely to label Fleet readiness.
    let credential = resolve_credential_diagnostic(config);
    let has_credentials_or_local = credential.availability.certifies_ready();
    let subagents_enabled = config.subagents_enabled_for_provider(provider);
    let disabled_reason = if subagents_enabled {
        None
    } else {
        Some(
            config
                .subagents_disabled_reason()
                .unwrap_or("disabled for active provider"),
        )
    };
    let max_subagents = config.max_subagents_for_provider(provider);
    let launch_concurrency = config.launch_concurrency_for_provider(provider);
    let max_admitted = config.max_admitted_subagents_for_provider(provider);
    let max_spawn_depth = config.subagent_max_spawn_depth_for_provider(provider);
    let roster =
        crate::fleet::identity::load_effective_roster(&config.fleet_config(), workspace, None);
    let mut built_in_members = 0usize;
    let mut plugin_members = 0usize;
    let mut config_members = 0usize;
    let mut personal_members = 0usize;
    let mut workspace_members = 0usize;
    for member in roster.members() {
        match member.origin {
            crate::fleet::roster::ProfileOrigin::BuiltIn => built_in_members += 1,
            crate::fleet::roster::ProfileOrigin::Plugin => plugin_members += 1,
            crate::fleet::roster::ProfileOrigin::Config => config_members += 1,
            crate::fleet::roster::ProfileOrigin::Personal => personal_members += 1,
            crate::fleet::roster::ProfileOrigin::Workspace => workspace_members += 1,
        }
    }
    let roster_members = roster.members().len();
    let custom_members = plugin_members + config_members + personal_members + workspace_members;
    let roster_ready = roster.load_error().is_none() && roster_members > 0;
    let runtime_ready =
        subagents_enabled && max_subagents > 0 && launch_concurrency > 0 && max_spawn_depth > 0;
    let multi_layer: Vec<serde_json::Value> = roster
        .multi_layer_report()
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "effective": entry.effective.to_string(),
                "effective_path": entry.effective_path.display().to_string(),
                "layers": entry
                    .layers
                    .iter()
                    .map(|layer| {
                        json!({
                            "origin": layer.origin.to_string(),
                            "path": layer.source.display().to_string(),
                            "wins": layer.wins,
                        })
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "ready": has_credentials_or_local && runtime_ready && roster_ready,
        "provider": {
            "id": config.provider_identity_for(provider),
            "auth": {
                "present_or_local": has_credentials_or_local,
                "source": doctor_api_key_source_label(credential.source),
                "availability": credential.availability.label(),
            },
        },
        "worker_runtime": {
            "ready": runtime_ready,
            "enabled": subagents_enabled,
            "disabled_reason": disabled_reason,
            "max_subagents": max_subagents,
            "launch_concurrency": launch_concurrency,
            "max_admitted": max_admitted,
            "max_spawn_depth": max_spawn_depth,
            "host_enforced_workflow_receipts": true,
        },
        "roster": {
            "ready": roster_ready,
            "error": roster.load_error(),
            "total": roster_members,
            "built_in": built_in_members,
            "config": config_members,
            "personal": personal_members,
            "workspace": workspace_members,
            "custom": custom_members,
            "starter_roster_available": built_in_members > 0,
            "readiness_rule": "built-in starter roster or custom roster",
            "multi_layer": multi_layer,
        },
        "concurrency": {
            "launch_concurrency": launch_concurrency,
            "max_subagents": max_subagents,
            "max_admitted": max_admitted,
            "plan_limit_probed": false,
        },
    })
}

pub(crate) fn doctor_provider_model_report_json(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let provider = config.api_provider();
    let credential = resolve_credential_diagnostic(config);
    let auth_present_or_local = credential.availability.certifies_ready();
    let credential_help = provider.credential_help();
    let credential_url = credential_help
        .credential_url
        .map(crate::doctor::structural_url_authority);
    let credential_docs_url = credential_help
        .docs_url
        .map(crate::doctor::structural_url_authority);

    json!({
        "provider": {
            "id": config.provider_identity_for(provider),
            "display": provider.display_name(),
        },
        "model": {
            "resolved": config.default_model(),
        },
        "auth": {
            "present_or_local": auth_present_or_local,
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
            "env_vars": provider.env_vars(),
            "credential_mode": credential_help.acquisition.as_str(),
            "credential_url": credential_url,
            "credential_docs_url": credential_docs_url,
            "credential_guidance": credential_help.guidance,
            "oauth_only": credential_help.acquisition
                == codewhale_config::provider::CredentialAcquisition::OAuth,
        },
        "health": {
            "live_validation": false,
            "next_action": if auth_present_or_local {
                "/model"
            } else {
                "/setup provider or /provider setup <name>"
            },
        },
    })
}

pub(crate) fn doctor_dsh_integration_report(
    config: &Config,
    workspace: &Path,
) -> anyhow::Result<crate::integrations::dsh::DshStatusReport> {
    use crate::integrations::dsh;
    let paths = dsh::DshPaths::from_process()?;
    let detection = dsh::detect::detect(&dsh::DetectEnv::from_process(), &dsh::ProcessRunner);
    let identity = dsh::codewhale_route_identity(config, workspace);
    dsh::compute_status(
        &paths,
        detection,
        identity,
        false,
        dsh::bundle_availability_now(),
    )
}

pub(crate) fn doctor_dsh_integration_lines(config: &Config, workspace: &Path) -> Vec<String> {
    match doctor_dsh_integration_report(config, workspace) {
        Ok(report) => {
            let mut lines = vec![
                format!("state: {}", report.state.label()),
                crate::integrations::dsh::status_line(&report),
                format!(
                    "owned files: {} (overlay {})",
                    crate::utils::display_path(&report.paths_root),
                    if report.overlay_present {
                        "present"
                    } else {
                        "absent"
                    }
                ),
            ];
            if !report.shadowing_namespaces.is_empty() {
                lines.push(format!(
                    "dsh settings.yaml sections that can shadow the overlay: {}",
                    report.shadowing_namespaces.join(", ")
                ));
            }
            lines
        }
        Err(error) => vec![format!("unavailable: {error}")],
    }
}

pub(crate) fn doctor_dsh_integration_json(config: &Config, workspace: &Path) -> serde_json::Value {
    match doctor_dsh_integration_report(config, workspace) {
        Ok(report) => serde_json::json!({
            "state": report.state.label(),
            "summary": crate::integrations::dsh::status_line(&report),
            "dsh_version": report.detection.version,
            "compatibility": report.detection.compatibility.label(),
            "overlay_present": report.overlay_present,
            "shadowing_namespaces": report.shadowing_namespaces,
        }),
        Err(error) => serde_json::json!({ "state": "unavailable", "error": error.to_string() }),
    }
}

pub(crate) fn doctor_external_credential_consent_statuses(
    config: &Config,
) -> Vec<codewhale_config::ExternalCredentialConsentStatus> {
    [
        crate::config::ApiProvider::OpenaiCodex,
        crate::config::ApiProvider::Xai,
        crate::config::ApiProvider::Deepseek,
    ]
    .into_iter()
    .filter_map(|provider| config.external_credential_consent_status(provider))
    .collect()
}

pub(crate) fn doctor_external_credential_consent_lines(config: &Config) -> Vec<String> {
    doctor_external_credential_consent_statuses(config)
        .into_iter()
        .flat_map(|status| {
            let mut lines = vec![
                format!(
                    "{}: access={}, provider={}, source={}, owner={}, path={}, version={}, state={}, ambient_path_changed={}",
                    status.provider,
                    status.access.as_str(),
                    status.provider,
                    status.source.as_str(),
                    status.owner,
                    codewhale_config::quote_os_path(&status.path),
                    status.consent_version,
                    status.route_state,
                    status.ambient_path_changed,
                ),
                format!("  semantics: {}", status.semantics),
                format!("  revoke: {}", status.revoke_command),
            ];
            if let Some(warning) = status.ambient_path_warning() {
                lines.push(format!("  {warning}"));
            }
            lines
        })
        .collect()
}

pub(crate) fn doctor_external_credential_consent_json(config: &Config) -> serde_json::Value {
    serde_json::Value::Array(
        doctor_external_credential_consent_statuses(config)
            .into_iter()
            .map(|status| {
                serde_json::json!({
                    "provider": status.provider,
                    "access": status.access.as_str(),
                    "source": status.source.as_str(),
                    "owner": status.owner,
                    "path": codewhale_config::quote_os_path(&status.path),
                    "consent_version": status.consent_version,
                    "scope_valid": status.scope_valid,
                    "ambient_path_changed": status.ambient_path_changed,
                    "ambient_path_warning": status.ambient_path_warning(),
                    "route_state": status.route_state,
                    "semantics": status.semantics,
                    "revoke_command": status.revoke_command,
                })
            })
            .collect(),
    )
}

pub(crate) fn doctor_setup_report_json(config: &Config, workspace: &Path) -> serde_json::Value {
    use serde_json::json;

    let (state, source) = doctor_setup_state(config, workspace);
    let (default_mode, default_mode_source) = doctor_runtime_default_mode();
    let (permission_posture, permission_posture_source) = doctor_runtime_permission_posture();
    let approval_policy = config.approval_policy.as_deref().unwrap_or("on-request");
    let approval_policy_source = if config.approval_policy.is_some() {
        "config"
    } else {
        "default"
    };
    let allow_shell = config.interactive_allow_shell();
    let allow_shell_source = if config.allow_shell.is_some() {
        "config"
    } else {
        "interactive_default"
    };
    let sandbox_mode = config.sandbox_mode.as_deref().unwrap_or("mode-derived");
    let sandbox_mode_source = if config.sandbox_mode.is_some() {
        "config"
    } else {
        "default"
    };
    let network_default = config
        .network
        .as_ref()
        .map_or("prompt", |policy| policy.default.as_str());
    let network_source = if config.network.is_some() {
        "config"
    } else {
        "default"
    };
    let (telemetry_value, telemetry_source) = doctor_runtime_telemetry(config);
    let workspace_trusted = !crate::tui::onboarding::needs_trust(workspace);
    let credential = resolve_credential_diagnostic(config);
    let credential_ready = credential.availability.certifies_ready();
    let steps: Vec<_> = codewhale_config::SetupStep::ALL
        .into_iter()
        .map(|step| {
            let entry = state.steps.get(&step);
            json!({
                "step": setup_step_id(step),
                "status": setup_status_id(state.status(step)),
                "required": entry.is_some_and(|entry| entry.required),
                "version": entry.and_then(|entry| entry.version.clone()),
                "result": entry.and_then(|entry| entry.result.clone()),
            })
        })
        .collect();

    json!({
        "source": source,
        "schema_version": state.schema_version,
        "inherited": state.inherited,
        "checkpoint_version": crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
        "first_run_ready": state.first_run_ready(),
        "update_ready": state.update_ready(crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION),
        "operate_ready": state.operate_ready(),
        "credential": {
            "ready": credential_ready,
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
        },
        "constitution": {
            "choice": constitution_choice_id(state.constitution_choice),
            "source": constitution_source_id(state.constitution_source),
            "validity": constitution_validity_id(state.constitution_validity),
            "checkpoint_completed_for": state.constitution_checkpoint_completed_for.clone(),
            "language": state.constitution_language.clone(),
            "preview_hash_present": state.constitution_preview_hash.is_some(),
            "preview_version": state.constitution_preview_version,
            "autonomy_preference": doctor_constitution_autonomy_preference_id(),
        },
        "runtime_posture_source": runtime_posture_source_id(state.runtime_posture_source),
        "runtime_posture": {
            "source": runtime_posture_source_id(state.runtime_posture_source),
            "default_mode": {
                "value": default_mode,
                "source": default_mode_source,
            },
            "permission_posture": {
                "value": permission_posture,
                "source": permission_posture_source,
            },
            "approval_policy": {
                "value": approval_policy,
                "source": approval_policy_source,
            },
            "allow_shell": {
                "value": allow_shell,
                "source": allow_shell_source,
            },
            "sandbox_mode": {
                "value": sandbox_mode,
                "source": sandbox_mode_source,
            },
            "network_default": {
                "value": network_default,
                "source": network_source,
            },
            "telemetry": {
                "value": telemetry_value,
                "source": telemetry_source,
            },
            "workspace_trust": {
                "trusted": workspace_trusted,
                "source": "workspace",
            },
        },
        "provider_model": doctor_provider_model_report_json(config),
        "operate_fleet": doctor_operate_fleet_report_json(config, workspace),
        "consistency": doctor_setup_consistency(&state, source),
        "next_actions": {
            "constitution": "/constitution",
            "setup_report": "/setup report",
            "provider_model": "/setup provider, /provider setup <name>, or /model",
            "runtime_posture": "/config",
            "operate_fleet": "/setup fleet (readiness), /fleet setup (explicit profile authoring)",
            "hotbar": "/setup hotbar",
            "tools_mcp": "/setup tools",
            "remote_runtime": "/setup remote",
            "persistence": "/setup persistence",
        },
        "steps": steps,
    })
}

pub(crate) fn setup_step_id(step: codewhale_config::SetupStep) -> &'static str {
    match step {
        codewhale_config::SetupStep::Language => "language",
        codewhale_config::SetupStep::ProviderModel => "provider_model",
        codewhale_config::SetupStep::TrustSandbox => "trust_sandbox",
        codewhale_config::SetupStep::ToolsMcp => "tools_mcp",
        codewhale_config::SetupStep::Hotbar => "hotbar",
        codewhale_config::SetupStep::RemoteRuntime => "remote_runtime",
        codewhale_config::SetupStep::Persistence => "persistence",
        codewhale_config::SetupStep::Constitution => "constitution",
        codewhale_config::SetupStep::OperateFleet => "operate_fleet",
        codewhale_config::SetupStep::Verification => "verification",
    }
}

pub(crate) fn setup_status_id(status: codewhale_config::StepStatus) -> &'static str {
    match status {
        codewhale_config::StepStatus::NotStarted => "not_started",
        codewhale_config::StepStatus::Recommended => "recommended",
        codewhale_config::StepStatus::Optional => "optional",
        codewhale_config::StepStatus::Deferred => "deferred",
        codewhale_config::StepStatus::InProgress => "in_progress",
        codewhale_config::StepStatus::Verified => "verified",
        codewhale_config::StepStatus::NeedsAction => "needs_action",
        codewhale_config::StepStatus::Failed => "failed",
        codewhale_config::StepStatus::Skipped => "skipped",
    }
}

pub(crate) fn constitution_choice_id(choice: codewhale_config::ConstitutionChoice) -> &'static str {
    match choice {
        codewhale_config::ConstitutionChoice::Unset => "unset",
        codewhale_config::ConstitutionChoice::Bundled => "bundled",
        codewhale_config::ConstitutionChoice::GuidedCustom => "guided_custom",
        codewhale_config::ConstitutionChoice::ExpertOverride => "expert_override",
        codewhale_config::ConstitutionChoice::Deferred => "deferred",
    }
}

pub(crate) fn constitution_source_id(source: codewhale_config::ConstitutionSource) -> &'static str {
    match source {
        codewhale_config::ConstitutionSource::Bundled => "bundled",
        codewhale_config::ConstitutionSource::UserGlobal => "user_global",
        codewhale_config::ConstitutionSource::ExpertOverride => "expert_override",
    }
}

pub(crate) fn constitution_validity_id(
    validity: codewhale_config::ConstitutionValidity,
) -> &'static str {
    match validity {
        codewhale_config::ConstitutionValidity::Unknown => "unknown",
        codewhale_config::ConstitutionValidity::Valid => "valid",
        codewhale_config::ConstitutionValidity::Invalid => "invalid",
        codewhale_config::ConstitutionValidity::Empty => "empty",
        codewhale_config::ConstitutionValidity::Unreadable => "unreadable",
    }
}

pub(crate) fn runtime_posture_source_id(
    source: codewhale_config::RuntimePostureSource,
) -> &'static str {
    match source {
        codewhale_config::RuntimePostureSource::Unset => "unset",
        codewhale_config::RuntimePostureSource::Inherited => "inherited",
        codewhale_config::RuntimePostureSource::Confirmed => "confirmed",
    }
}

/// Emit a bounded, secret-redacted JSON failure when configuration cannot be
/// loaded or validated. Invalid configuration must not be forced through the
/// normal doctor report because its route/capability facts would be misleading.
pub(crate) fn run_doctor_json_config_error(error: &anyhow::Error) -> Result<()> {
    let safe_message = error
        .downcast_ref::<crate::config::SafeConfigDiagnostic>()
        .map(ToString::to_string);
    let report = serde_json::json!({
        "status": "error",
        "error": {
            "kind": "config_validation",
            "message": safe_message.as_deref().unwrap_or("configuration validation failed; details omitted because configuration errors may contain credential material"),
        },
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    // Keep stderr generic: the actionable, redacted error is already on
    // stdout, and Rust's Result termination must never redisclose a secret.
    bail!("doctor configuration validation failed; see JSON output")
}

/// Machine-readable counterpart to `run_doctor`. This report is always
/// structural and offline; live probe flags conflict with `--json`.
pub(crate) fn run_doctor_json(
    config: &Config,
    workspace: &Path,
    config_path_override: Option<&Path>,
    plugins: &crate::plugins::PluginRegistry,
) -> Result<()> {
    use serde_json::json;

    let doctor_paths = crate::doctor::DoctorPathReport::resolve(config_path_override)?;
    let config_path = &doctor_paths.config;
    let secret_backend = codewhale_secrets::diagnose_secret_backend();

    let credential = resolve_credential_diagnostic(config);

    let mcp_config_path = config.mcp_config_path();
    let project_mcp_config_path = crate::mcp::workspace_mcp_config_path(workspace);
    let mcp_present = mcp_config_path.exists();
    let project_mcp_present = project_mcp_config_path.exists();
    let mcp_summary = match crate::mcp::load_config_with_workspace_and_plugins(
        &mcp_config_path,
        workspace,
        plugins,
    ) {
        Ok(cfg) => {
            let servers: Vec<serde_json::Value> = cfg
                .servers
                .iter()
                .map(|(name, server)| doctor_mcp_server_json(name, server))
                .collect();
            json!({
                "config_path": mcp_config_path.display().to_string(),
                "present": mcp_present,
                "project_config_path": project_mcp_config_path.display().to_string(),
                "project_present": project_mcp_present,
                "probe_scope": "configuration",
                "live_health_checked": false,
                "servers": servers,
            })
        }
        Err(_) => json!({
            "config_path": mcp_config_path.display().to_string(),
            "present": mcp_present,
            "project_config_path": project_mcp_config_path.display().to_string(),
            "project_present": project_mcp_present,
            "probe_scope": "configuration",
            "live_health_checked": false,
            "servers": [],
            "error": "configuration_unavailable_details_omitted",
        }),
    };

    let global_skills_dir = config.skills_dir();
    let agents_skills_dir = workspace.join(".agents").join("skills");
    let local_skills_dir = workspace.join("skills");
    let agents_global_skills_dir = crate::skills::agents_global_skills_dir();
    // #432: cross-tool skill discovery dirs surface in the JSON
    // report so external dashboards can see whether any
    // `.opencode/skills/`, `.claude/skills/`, `.cursor/skills/`, or
    // global agentskills.io content is contributing to the merged catalogue.
    let opencode_skills_dir = workspace.join(".opencode").join("skills");
    let claude_skills_dir = workspace.join(".claude").join("skills");
    let selected_skills_dir = if agents_skills_dir.exists() {
        agents_skills_dir.clone()
    } else if local_skills_dir.exists() {
        local_skills_dir.clone()
    } else if config.skills_dir.is_none()
        && let Some(global_agents) = agents_global_skills_dir.as_ref()
        && global_agents.exists()
    {
        global_agents.clone()
    } else {
        global_skills_dir.clone()
    };
    let agents_global_summary = agents_global_skills_dir
        .as_ref()
        .map(|path| {
            json!({
                "path": path.display().to_string(),
                "present": path.exists(),
                "count": skills_count_for(path),
            })
        })
        .unwrap_or_else(|| {
            json!({
                "path": null,
                "present": false,
                "count": 0,
            })
        });

    let tools_dir = default_tools_dir();
    let plugins_dir = default_plugins_dir();

    // Memory feature state (#489). Operators ask "is memory on?" and
    // "where does it live?" — surface both here so the question can be
    // answered without booting the TUI. Both inputs are checked: the
    // config flag and the env-var override that the runtime would
    // honour. (The dedicated `Config::memory_enabled()` accessor lives
    // on the memory-MVP branch (#518); this duplicates the same logic
    // until the two PRs land and it can be replaced with a single
    // method call.)
    let memory_path = config.memory_path();
    let memory_enabled_env = std::env::var("CODEWHALE_MEMORY")
        .or_else(|_| std::env::var("DEEPSEEK_MEMORY"))
        .ok()
        .map(|raw| {
            matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "on" | "true" | "yes" | "y" | "enabled"
            )
        })
        .unwrap_or(false);
    let memory_summary = json!({
        // The MVP feature is opt-in by default; this defaults to false
        // on branches without the [memory] section in `Config`.
        "enabled": memory_enabled_env,
        "path": memory_path.display().to_string(),
        "file_present": memory_path.exists(),
    });
    let api_target = doctor_api_target(config);
    let strict_tool_mode = doctor_strict_tool_mode_status(config);
    let tls_status = doctor_tls_status(config);
    let (code_home, legacy_home) = doctor_state_roots();
    let legacy_state_report = doctor_legacy_state_report(&code_home, &legacy_home);
    let session_recovery = doctor_session_recovery_report(
        &code_home,
        &legacy_home,
        codewhale_config::codewhale_home_is_explicit(),
    );

    let stash = crate::composer_stash::diagnostic_stash_report();
    let report = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "config_path": config_path.display().to_string(),
        "config_present": config_path.exists(),
        "paths": doctor_paths,
        "secret_backend": secret_backend,
        "workspace": workspace.display().to_string(),
        "legacy_state": doctor_legacy_state_json(
            &code_home,
            &legacy_home,
            &legacy_state_report,
            &session_recovery,
        ),
        "setup": doctor_setup_report_json(config, workspace),
        "api_key": {
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
        },
        "external_credentials": doctor_external_credential_consent_json(config),
        "dsh_integration": doctor_dsh_integration_json(config, workspace),
        "base_url": crate::doctor::structural_url_authority(&api_target.base_url),
        "default_text_model": api_target.model,
        // DGF-01: this report describes the route a session launched now
        // would resolve; a running session keeps its launch-time route.
        "route_scope": "configured_at_launch",
        "model_resolution": match api_target.resolution {
            DoctorModelResolution::Resolved => "resolved",
            DoctorModelResolution::ConfiguredOnly => "configured_unresolved",
        },
        "route": doctor_route_report(config),
        "strict_tool_mode": doctor_strict_tool_mode_report_json(&strict_tool_mode),
        "tls": {
            "certificate_verification": tls_status.certificate_verification,
            "insecure_skip_tls_verify": tls_status.insecure_skip_tls_verify,
            "provider": tls_status.provider,
            "message": tls_status.message,
        },
        "search_provider": doctor_search_provider_json(config),
        "memory": memory_summary,
        "mcp": mcp_summary,
        "skills": {
            "selected": selected_skills_dir.display().to_string(),
            "global": {
                "path": global_skills_dir.display().to_string(),
                "present": global_skills_dir.exists(),
                "count": skills_count_for(&global_skills_dir),
            },
            "agents": {
                "path": agents_skills_dir.display().to_string(),
                "present": agents_skills_dir.exists(),
                "count": skills_count_for(&agents_skills_dir),
            },
            "agents_global": agents_global_summary,
            "local": {
                "path": local_skills_dir.display().to_string(),
                "present": local_skills_dir.exists(),
                "count": skills_count_for(&local_skills_dir),
            },
            "opencode": {
                "path": opencode_skills_dir.display().to_string(),
                "present": opencode_skills_dir.exists(),
                "count": skills_count_for(&opencode_skills_dir),
            },
            "claude": {
                "path": claude_skills_dir.display().to_string(),
                "present": claude_skills_dir.exists(),
                "count": skills_count_for(&claude_skills_dir),
            },
        },
        "tools": {
            "path": tools_dir.display().to_string(),
            "present": tools_dir.exists(),
            "count": if tools_dir.exists() { count_dir_entries(&tools_dir) } else { 0 },
        },
        "plugins": {
            "path": plugins_dir.display().to_string(),
            "present": plugins_dir.exists(),
            "count": if plugins_dir.exists() { count_dir_entries(&plugins_dir) } else { 0 },
        },
        "storage": {
            "spillover": {
                "path": crate::tools::truncate::spillover_root()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                "present": crate::tools::truncate::spillover_root()
                    .is_some_and(|p| p.is_dir()),
                "count": crate::tools::truncate::spillover_root()
                    .filter(|p| p.is_dir())
                    .map(|p| count_dir_entries(&p))
                    .unwrap_or(0),
            },
            "stash": {
                "path": stash
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                "present": stash.present,
                "count": stash.count,
                "error": stash.error,
            },
        },
        "sandbox": match crate::sandbox::get_platform_sandbox_with_bwrap_preference(
            config.prefer_bwrap.unwrap_or(false),
        ) {
            Some(kind) => json!({"available": true, "kind": kind.to_string()}),
            None => json!({"available": false, "kind": null}),
        },
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "api_connectivity": {
            "checked": false,
            "status": "not_probed",
            "note": "JSON doctor is offline; use `codewhale doctor --probe-api` or `--probe-local` for an explicit live check.",
        },
        "capability": provider_capability_report(config),
    });

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

pub(crate) fn run_doctor_context_json(config: &Config, workspace: &Path) -> Result<()> {
    let report = crate::context_report::build_headless_context_report(config, workspace);
    println!("{}", crate::context_report::context_report_json(&report));
    Ok(())
}

/// Build the `capability` section for the machine-readable doctor report.
///
/// Returns a JSON value with the resolved provider, resolved model, context
/// window, max output, thinking support, cache telemetry support, and request
/// payload mode.
pub(crate) fn provider_capability_report(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let provider = config.api_provider();
    let configured_model = config.default_model();
    let route_result =
        crate::route_runtime::resolve_runtime_route(config, provider, Some(&configured_model));
    let route_error = route_result
        .is_err()
        .then_some("route_resolution_failed_details_omitted");
    let route = route_result.ok();
    let resolved_model = route
        .as_ref()
        .map_or(configured_model.as_str(), |route| route.model.as_str());
    let cap = crate::config::provider_capability(provider, resolved_model);
    let route_profile = route.as_ref().map(|route| {
        crate::model_profile::resolved_capability_profile_for_route(
            provider,
            resolved_model,
            route.candidate.capabilities(),
            route.candidate.limits(),
        )
    });
    let context_window = route
        .as_ref()
        .map_or(cap.context_window, |route| route.context_window.tokens);
    let context_window_source = route.as_ref().map_or(
        crate::route_runtime::ContextWindowSource::Fallback.label(),
        |route| route.context_window.source.label(),
    );
    // `null` when neither the resolved route nor the compatibility matrix
    // publishes an output ceiling — doctor must not invent one.
    let max_output = route_profile
        .as_ref()
        .and_then(|profile| profile.max_output)
        .or(cap.max_output);
    let is_exact_kimi_code_k3 = route.as_ref().is_some_and(|route| {
        crate::config::is_exact_kimi_code_k3_route(
            provider,
            &route.candidate.endpoint().base_url,
            route.candidate.wire_model_id().as_str(),
        )
    });
    let thinking_supported = is_exact_kimi_code_k3
        || route_profile
            .as_ref()
            .map_or(cap.thinking_supported, |profile| {
                profile.supports_reasoning()
            });
    let cache_telemetry_supported = route_profile
        .as_ref()
        .map_or(cap.cache_telemetry_supported, |profile| {
            profile.prompt_caching.is_supported()
        });
    let request_payload_mode = route_profile
        .as_ref()
        .map_or(cap.request_payload_mode, |profile| {
            profile.request_payload_mode
        });
    let alias_deprecation = config.active_deepseek_alias_deprecation();

    json!({
        "resolved_provider": config.provider_identity_for(provider),
        "resolved_model": resolved_model,
        "context_window": context_window,
        "context_window_source": context_window_source,
        "max_output": max_output,
        "thinking_supported": thinking_supported,
        "cache_telemetry_supported": cache_telemetry_supported,
        "request_payload_mode": serde_json::to_value(request_payload_mode).unwrap_or_default(),
        "route_error": route_error,
        "alias_deprecation": alias_deprecation,
    })
}

pub(crate) fn doctor_route_report(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let target = doctor_api_target(config);
    let provider = config.api_provider();
    let redacted_base_url = crate::doctor::structural_url_authority(&target.base_url);
    let route_result =
        crate::route_runtime::resolve_runtime_route(config, provider, Some(&target.model));
    let route_error = route_result
        .is_err()
        .then_some("route_resolution_failed_details_omitted");
    let context_window = route_result
        .ok()
        .map(|route| {
        json!({
            "tokens": route.context_window.tokens,
            "source": route.context_window.source.label(),
        })
    })
    .unwrap_or_else(|| {
        json!({
            "tokens": crate::config::provider_capability(provider, &target.model).context_window,
            "source": crate::route_runtime::ContextWindowSource::Fallback.label(),
        })
    });

    let route_identity =
        crate::config::moonshot_k3_route_display_name(&target.base_url, &target.model);
    let credential = resolve_credential_diagnostic(config);

    json!({
        "provider": target.provider,
        "provider_source": doctor_provider_source(config),
        "provider_config_table": doctor_provider_config_table(config, provider),
        "model": target.model,
        "route_identity": route_identity,
        "wire_protocol": doctor_wire_protocol(provider),
        "base_url": {
            "redacted": redacted_base_url,
            "class": doctor_base_url_class(provider, &target.base_url),
            "fingerprint": crate::utils::redacted_identifier_for_log(&target.base_url),
        },
        "auth": {
            "scheme": doctor_auth_scheme(config),
            "source": doctor_api_key_source_label(credential.source),
            "availability": credential.availability.label(),
        },
        "context_window": context_window,
        "route_error": route_error,
    })
}

pub(crate) fn doctor_provider_config_table(
    config: &Config,
    provider: crate::config::ApiProvider,
) -> String {
    if provider != crate::config::ApiProvider::Custom {
        return provider_config_table_key(provider).to_string();
    }
    if config.uses_legacy_literal_custom_route() {
        "root (legacy literal custom)".to_string()
    } else {
        format!("providers.{}", config.provider_identity_for(provider))
    }
}

pub(crate) fn doctor_provider_source(config: &Config) -> &'static str {
    if config
        .provider
        .as_ref()
        .is_some_and(|provider| !provider.trim().is_empty())
    {
        "config"
    } else {
        "default"
    }
}

pub(crate) fn doctor_wire_protocol(provider: crate::config::ApiProvider) -> &'static str {
    let policy = provider
        .metadata()
        .map(|metadata| metadata.wire_policy())
        .unwrap_or(codewhale_config::provider::WirePolicy::Fixed(
            codewhale_config::provider::WireFormat::ChatCompletions,
        ));
    match policy.fixed() {
        Some(codewhale_config::provider::WireFormat::ChatCompletions) => "chat_completions",
        Some(codewhale_config::provider::WireFormat::Responses) => "responses",
        Some(codewhale_config::provider::WireFormat::AnthropicMessages) => "anthropic_messages",
        None => "model_aware",
    }
}

pub(crate) fn doctor_base_url_class(
    provider: crate::config::ApiProvider,
    base_url: &str,
) -> &'static str {
    let normalized = base_url.trim_end_matches('/').to_ascii_lowercase();
    if normalized.starts_with("http://localhost")
        || normalized.starts_with("http://127.0.0.1")
        || normalized.starts_with("http://[::1]")
    {
        return "local";
    }
    if normalized
        == provider
            .default_base_url()
            .trim_end_matches('/')
            .to_ascii_lowercase()
    {
        "default"
    } else {
        "custom"
    }
}

pub(crate) fn doctor_auth_scheme(config: &Config) -> &'static str {
    let provider = config.api_provider();
    if crate::config::auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref())
    {
        "none"
    } else if provider == crate::config::ApiProvider::Anthropic {
        "x-api-key"
    } else if provider == crate::config::ApiProvider::XiaomiMimo
        && doctor_xiaomi_mimo_base_url_uses_token_plan(&config.deepseek_base_url())
    {
        "api-key"
    } else if provider == crate::config::ApiProvider::XiaomiMimo {
        // The alternate MiMo scheme depends on a credential prefix. Ordinary
        // doctor does not read credentials merely to make this label precise.
        "unknown"
    } else if matches!(
        provider,
        crate::config::ApiProvider::Sglang
            | crate::config::ApiProvider::Vllm
            | crate::config::ApiProvider::Ollama
    ) {
        "optional_bearer"
    } else {
        "bearer"
    }
}

pub(crate) fn doctor_xiaomi_mimo_base_url_uses_token_plan(base_url: &str) -> bool {
    let normalized = base_url.trim_end_matches('/');
    [
        crate::config::XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL,
        crate::config::XIAOMI_MIMO_TOKEN_PLAN_SGP_BASE_URL,
        crate::config::XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL,
    ]
    .iter()
    .any(|candidate| normalized.eq_ignore_ascii_case(candidate.trim_end_matches('/')))
}

pub(crate) fn doctor_api_key_source_label(source: ApiKeySource) -> &'static str {
    match source {
        ApiKeySource::ConfigDeclared => "config_declared",
        ApiKeySource::EnvDeclared => "env_declared",
        ApiKeySource::ExternalAuthDeclared => "external_auth_declared",
        ApiKeySource::SecretStoreUnprobed => "secret_store_unprobed",
        ApiKeySource::SecretStoreUnavailable => "secret_store_unavailable",
        ApiKeySource::OAuth => "oauth_unprobed",
        ApiKeySource::ExternalConsent => "external_consent",
        ApiKeySource::NoAuth => "none",
        ApiKeySource::LocalRuntime => "local_runtime",
        ApiKeySource::Unknown => "unknown",
    }
}

pub(crate) fn doctor_search_provider_line(config: &Config) -> String {
    let search_provider = config.search_provider_resolution();
    let switch_hint = if matches!(
        (search_provider.provider, search_provider.source),
        (
            crate::config::SearchProvider::Firecrawl,
            crate::config::SearchProviderSource::Default
        )
    ) {
        "; set [search] provider = \"baidu\" | \"metaso\" | \"volcengine\" for China"
    } else {
        ""
    };

    format!(
        "search_provider: {} (source: {}{})",
        search_provider.provider.as_str(),
        search_provider.source.as_str(),
        switch_hint
    )
}

pub(crate) fn doctor_search_provider_json(config: &Config) -> serde_json::Value {
    use serde_json::json;

    let search_provider = config.search_provider_resolution();
    json!({
        "provider": search_provider.provider.as_str(),
        "source": search_provider.source.as_str(),
        "reachability": "not_checked",
        "reachability_reason": "offline_json",
    })
}

/// Whether the model in a [`DoctorApiTarget`] is the wire id the engine
/// resolver produced, or only the raw configured value because resolution
/// failed. Doctor never prints resolution error details — the JSON route
/// report already redacts them for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoctorModelResolution {
    Resolved,
    ConfiguredOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorApiTarget {
    pub(crate) provider: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) resolution: DoctorModelResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorStrictToolModeStatus {
    pub(crate) enabled: bool,
    pub(crate) status: &'static str,
    pub(crate) function_strict_sent: bool,
    pub(crate) message: String,
    pub(crate) recommended_base_url: Option<String>,
}

pub(crate) fn doctor_api_target(config: &Config) -> DoctorApiTarget {
    let provider = config.api_provider();
    // Report the model through the same resolver the live client uses at
    // session launch (`client.rs` → `resolve_runtime_route`), so doctor's
    // answer matches what a session started now would actually serve —
    // saved provider models, alias normalization, and roster preference
    // included — instead of re-deriving a config default that can diverge
    // from the engine (DGF-01, dogfood 2026-08-02).
    let (model, resolution) =
        match crate::route_runtime::resolve_runtime_route(config, provider, None) {
            Ok(route) => (route.model.clone(), DoctorModelResolution::Resolved),
            Err(_) => (
                config.default_model(),
                DoctorModelResolution::ConfiguredOnly,
            ),
        };
    DoctorApiTarget {
        provider: config.provider_identity_for(provider),
        base_url: config.deepseek_base_url(),
        model,
        resolution,
    }
}

pub(crate) fn doctor_strict_tool_mode_status(config: &Config) -> DoctorStrictToolModeStatus {
    if !config.strict_tool_mode.unwrap_or(false) {
        return DoctorStrictToolModeStatus {
            enabled: false,
            status: "disabled",
            function_strict_sent: false,
            message: "disabled".to_string(),
            recommended_base_url: None,
        };
    }

    let target = doctor_api_target(config);
    match known_deepseek_base_url_kind(&target.base_url) {
        Some(DeepSeekBaseUrlKind::Beta) => DoctorStrictToolModeStatus {
            enabled: true,
            status: "ready",
            function_strict_sent: true,
            message: "enabled; DeepSeek strict schemas use the beta endpoint".to_string(),
            recommended_base_url: None,
        },
        Some(DeepSeekBaseUrlKind::NonBeta) => {
            let recommended = recommended_strict_base_url(config, &target.base_url);
            DoctorStrictToolModeStatus {
                enabled: true,
                status: "fallback_non_beta",
                function_strict_sent: false,
                message:
                    "enabled, but function.strict is stripped for this non-beta DeepSeek endpoint"
                        .to_string(),
                recommended_base_url: Some(recommended.to_string()),
            }
        }
        None => DoctorStrictToolModeStatus {
            enabled: true,
            status: "custom_endpoint",
            function_strict_sent: true,
            message: "enabled; function.strict will be sent to this custom endpoint".to_string(),
            recommended_base_url: None,
        },
    }
}

pub(crate) fn doctor_strict_tool_mode_report_json(
    status: &DoctorStrictToolModeStatus,
) -> serde_json::Value {
    serde_json::json!({
        "enabled": status.enabled,
        "status": status.status,
        "function_strict_sent": status.function_strict_sent,
        "message": status.message,
        "recommended_base_url": status
            .recommended_base_url
            .as_deref()
            .map(crate::doctor::structural_url_authority),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoctorTlsStatus {
    pub(crate) certificate_verification: bool,
    pub(crate) insecure_skip_tls_verify: bool,
    pub(crate) provider: String,
    pub(crate) message: String,
}

pub(crate) fn doctor_tls_status(config: &Config) -> DoctorTlsStatus {
    let provider = config.provider_identity_for(config.api_provider());
    let insecure_skip_tls_verify = config.insecure_skip_tls_verify();
    let message = if insecure_skip_tls_verify {
        format!(
            "TLS certificate verification cannot be disabled for provider {provider}; use SSL_CERT_FILE with a trusted custom CA bundle"
        )
    } else {
        "TLS certificate verification enabled".to_string()
    };
    DoctorTlsStatus {
        certificate_verification: true,
        insecure_skip_tls_verify,
        provider,
        message,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeepSeekBaseUrlKind {
    Beta,
    NonBeta,
}

pub(crate) fn known_deepseek_base_url_kind(base_url: &str) -> Option<DeepSeekBaseUrlKind> {
    let normalized = base_url.trim_end_matches('/');
    if normalized.eq_ignore_ascii_case("https://api.deepseek.com/beta")
        || normalized.eq_ignore_ascii_case("https://api.deepseeki.com/beta")
    {
        Some(DeepSeekBaseUrlKind::Beta)
    } else if normalized.eq_ignore_ascii_case("https://api.deepseek.com")
        || normalized.eq_ignore_ascii_case("https://api.deepseek.com/v1")
        || normalized.eq_ignore_ascii_case("https://api.deepseeki.com")
        || normalized.eq_ignore_ascii_case("https://api.deepseeki.com/v1")
    {
        Some(DeepSeekBaseUrlKind::NonBeta)
    } else {
        None
    }
}

pub(crate) fn recommended_strict_base_url(_config: &Config, _base_url: &str) -> &'static str {
    crate::config::DEFAULT_DEEPSEEK_BASE_URL
}

pub(crate) fn doctor_timeout_recovery_lines(config: &Config) -> Vec<String> {
    let target = doctor_api_target(config);
    let mut lines = vec![format!(
        "Connection timed out while reaching {}.",
        crate::doctor::structural_url_authority(&target.base_url)
    )];

    match config.api_provider() {
        crate::config::ApiProvider::Deepseek
            if target.base_url.contains("api.deepseek.com")
                && !target.base_url.contains("api.deepseeki.com") =>
        {
            lines.push(
                "If this is a custom DeepSeek-compatible endpoint, set its HTTPS base URL in ~/.codewhale/config.toml and rerun `codewhale doctor`."
                    .to_string(),
            );
        }
        crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN => {
            lines.push(
                "If this is a custom DeepSeek-compatible endpoint, confirm it serves `/v1/models` and `/v1/chat/completions` over HTTPS."
                    .to_string(),
            );
        }
        _ => {
            lines.push(
                "Confirm the configured provider endpoint is reachable and OpenAI-compatible for `/v1/models` and `/v1/chat/completions`."
                    .to_string(),
            );
        }
    }

    lines.push(
        "Run `codewhale doctor --json` and include `base_url`, `default_text_model`, and `api_connectivity` when filing an issue."
            .to_string(),
    );
    lines
}

pub(crate) fn run_features_command(config: &Config, command: FeaturesCli) -> Result<()> {
    match command.command {
        FeaturesSubcommand::List => {
            print!("{}", render_feature_table(&config.features()));
            Ok(())
        }
    }
}

pub(crate) async fn run_models(config: &Config, args: ModelsArgs) -> Result<()> {
    use crate::client::DeepSeekClient;

    let client = DeepSeekClient::new(config)?;
    let mut models = client.list_models().await?;
    models.sort_by(|a, b| a.id.cmp(&b.id));

    if args.json {
        println!("{}", serde_json::to_string_pretty(&models)?);
        return Ok(());
    }

    if models.is_empty() {
        println!("No models returned by the API.");
        return Ok(());
    }

    let default_model = config.default_model();

    println!("Available models (default: {default_model})");
    for model in models {
        let marker = if model.id == default_model { "*" } else { " " };
        if let Some(owner) = model.owned_by {
            println!("{marker} {} ({owner})", model.id);
        } else {
            println!("{marker} {}", model.id);
        }
    }

    Ok(())
}

pub(crate) async fn run_speech(config: &Config, args: SpeechArgs) -> Result<()> {
    use crate::client::{DeepSeekClient, SpeechSynthesisRequest};
    use crate::config::ApiProvider;
    use crate::tools::speech::{
        DEFAULT_VOICE, SPEECH_MODEL_EXAMPLES, combine_speech_instructions,
        default_speech_output_name, describe_speech_voice, encode_voice_clone_sample_data_uri,
        infer_speech_model, normalize_speech_format,
    };

    let SpeechArgs {
        text,
        output,
        output_dir,
        model,
        voice,
        instruction,
        voice_prompt,
        clone_voice,
        format,
        json: json_output,
    } = args;

    if config.api_provider() != ApiProvider::XiaomiMimo {
        bail!(
            "`speech` requires provider = \"xiaomi-mimo\" (current: {}). Run with `--provider xiaomi-mimo` or set it in config.",
            config.api_provider().as_str()
        );
    }

    if text.trim().is_empty() {
        bail!("Speech text cannot be empty");
    }
    let voice_is_data_uri = voice
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| value.starts_with("data:audio/"));
    if clone_voice.is_some() && voice.is_some() {
        bail!("Use either --clone-voice or --voice for cloned voice data, not both");
    }
    let model = infer_speech_model(
        model.as_deref(),
        clone_voice.is_some() || voice_is_data_uri,
        voice_prompt.is_some(),
    );
    let model_lower = model.to_ascii_lowercase();
    if !model_lower.contains("tts") {
        bail!(
            "speech requires a TTS model (examples: {}); got {model}",
            SPEECH_MODEL_EXAMPLES.join(", ")
        );
    }
    let is_voice_design = model_lower.contains("voicedesign");
    let is_voice_clone = model_lower.contains("voiceclone");

    let instruction = combine_speech_instructions(instruction, voice_prompt);
    if is_voice_design
        && instruction
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        bail!(
            "mimo-v2.5-tts-voicedesign requires --voice-prompt or --instruction to describe the voice"
        );
    }

    let voice = if let Some(clone_path) = clone_voice {
        Some(encode_voice_clone_sample_data_uri(&clone_path)?)
    } else if is_voice_design {
        None
    } else if let Some(value) = voice.filter(|value| !value.trim().is_empty()) {
        Some(value)
    } else if is_voice_clone {
        bail!("mimo-v2.5-tts-voiceclone requires --clone-voice <mp3|wav> or --voice <data-uri>");
    } else {
        Some(DEFAULT_VOICE.to_string())
    };
    let format = normalize_speech_format(&format).with_context(|| {
        format!("Unsupported speech format '{format}' (allowed: wav, mp3, pcm16)")
    })?;
    let output = output.unwrap_or_else(|| {
        output_dir
            .or_else(|| config.speech_output_dir())
            .unwrap_or_default()
            .join(default_speech_output_name(&format))
    });

    let client = DeepSeekClient::new(config)?;
    let response = client
        .synthesize_speech(SpeechSynthesisRequest {
            model: model.clone(),
            text,
            instruction,
            audio_format: format.clone(),
            voice,
        })
        .await?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create output directory {}", parent.display()))?;
    }
    std::fs::write(&output, &response.audio_bytes)
        .with_context(|| format!("Failed to write audio file {}", output.display()))?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "speech",
                "success": true,
                "model": response.model,
                "format": response.audio_format,
                "output": output.display().to_string(),
                "bytes": response.audio_bytes.len(),
                "voice": response.voice.as_deref().map(describe_speech_voice),
                "transcript": response.transcript,
            }))?
        );
    } else {
        println!(
            "Generated speech: {} ({} bytes, model: {}, format: {})",
            output.display(),
            response.audio_bytes.len(),
            response.model,
            response.audio_format
        );
    }

    Ok(())
}
