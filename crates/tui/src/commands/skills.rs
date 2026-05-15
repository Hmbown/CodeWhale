//! Skills commands: skills, skill

use std::fmt::Write;

use crate::network_policy::NetworkPolicy;
use crate::skills::SkillRegistry;
use crate::skills::install::{
    self, DEFAULT_MAX_SIZE_BYTES, DEFAULT_REGISTRY_URL, InstallOutcome, InstallSource,
    RegistryFetchResult, SkillSyncOutcome, SyncResult, UpdateResult,
};
use crate::tui::app::App;
use crate::tui::history::HistoryCell;

use super::CommandResult;

fn discover_visible_skills(app: &App) -> SkillRegistry {
    crate::skills::discover_for_workspace_and_dir(&app.workspace, &app.skills_dir)
}

fn render_skill_warnings(registry: &SkillRegistry) -> String {
    if registry.warnings().is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let _ = writeln!(out, "\nWarnings ({}):", registry.warnings().len());
    for warning in registry.warnings() {
        let _ = writeln!(out, "  - {warning}");
    }
    out
}

/// List all available skills. Pass `--remote` (or `remote`) to fetch the
/// curated registry instead of scanning the local skills directory.
/// Pass `sync` to pull the registry index and download all skills to the
/// local cache (`~/.deepseek/cache/skills/`).
pub fn list_skills(app: &mut App, arg: Option<&str>) -> CommandResult {
    let mut prefix: Option<String> = None;
    if let Some(arg) = arg {
        let trimmed = arg.trim();
        if trimmed == "--remote" || trimmed == "remote" {
            return list_remote_skills(app);
        }
        if trimmed == "sync" || trimmed == "--sync" {
            return sync_skills(app);
        }
        if !trimmed.is_empty() {
            // Anything else is treated as a name-prefix filter (#1318).
            // Reject obviously malformed args (whitespace inside the
            // prefix, leading dash) so future flag additions don't
            // collide with skill names. Skill names that start with
            // `-` aren't allowed by the loader so this is safe.
            if trimmed.starts_with('-') || trimmed.split_whitespace().count() > 1 {
                return CommandResult::error("Usage: /skills [--remote|sync|<name-prefix>]");
            }
            prefix = Some(trimmed.to_ascii_lowercase());
        }
    }
    let skills_dir = app.skills_dir.clone();
    let registry = discover_visible_skills(app);
    let warnings = render_skill_warnings(&registry);

    if registry.is_empty() {
        let msg = format!(
            "No skills found.\n\n\
             Skills location: {}\n\n\
             To add skills, create directories with SKILL.md files:\n  \
             {}/my-skill/SKILL.md\n\n\
             Format:\n  \
             ---\n  \
             name: my-skill\n  \
             description: What this skill does\n  \
             allowed-tools: read_file, list_dir\n  \
             ---\n\n  \
             <instructions here>{warnings}",
            skills_dir.display(),
            skills_dir.display()
        );
        return CommandResult::message(msg);
    }

    let filtered: Vec<&crate::skills::Skill> = if let Some(p) = prefix.as_deref() {
        registry
            .list()
            .iter()
            .filter(|s| s.name.to_ascii_lowercase().starts_with(p))
            .collect()
    } else {
        registry.list().iter().collect()
    };

    if filtered.is_empty() {
        // The user typed a prefix that matched nothing. Surface what
        // they typed plus the full count so they can decide whether
        // to adjust the prefix or run `/skills` for the whole list.
        let p = prefix.as_deref().unwrap_or("");
        return CommandResult::message(format!(
            "No skills match prefix `{p}` (out of {} available).\n\nRun /skills to see them all.{warnings}",
            registry.len()
        ));
    }

    let mut output = if let Some(p) = prefix.as_deref() {
        format!(
            "Available skills matching `{p}` ({} of {}):\n",
            filtered.len(),
            registry.len()
        )
    } else {
        format!("Available skills ({}):\n", registry.len())
    };
    output.push_str("─────────────────────────────\n");

    if prefix.is_some() {
        // Filtered view: keep the flat list — the user already narrowed.
        for (idx, skill) in filtered.iter().enumerate() {
            if idx > 0 {
                output.push('\n');
            }
            let _ = writeln!(output, "  /{} - {}", skill.name, skill.description);
        }
    } else {
        // Unfiltered view: partition into user-created and built-in so a
        // workspace skill at the top of the list isn't pushed off-screen
        // by 10+ bundled descriptions. User skills always render with
        // their full description; bundled skills render compactly when
        // numerous so the whole menu fits in a typical terminal viewport.
        let (user_skills, bundled_skills): (
            Vec<&&crate::skills::Skill>,
            Vec<&&crate::skills::Skill>,
        ) = filtered
            .iter()
            .partition(|s| !crate::skills::is_bundled_skill_name(&s.name));

        if !user_skills.is_empty() {
            let _ = writeln!(output, "Your skills ({}):", user_skills.len());
            for skill in &user_skills {
                let _ = writeln!(output, "  /{} - {}", skill.name, skill.description);
            }
            if !bundled_skills.is_empty() {
                output.push('\n');
            }
        }

        if !bundled_skills.is_empty() {
            let _ = writeln!(output, "Built-in skills ({}):", bundled_skills.len());
            // When there are user skills to surface, keep built-ins compact
            // (single-line names list) so they never crowd the viewport.
            // When there are no user skills, render full descriptions —
            // there is nothing else competing for space and the user is
            // likely getting their first look at the catalog.
            if user_skills.is_empty() {
                for skill in &bundled_skills {
                    let _ = writeln!(output, "  /{} - {}", skill.name, skill.description);
                }
            } else {
                let names: Vec<String> = bundled_skills
                    .iter()
                    .map(|s| format!("/{}", s.name))
                    .collect();
                output.push_str("  ");
                output.push_str(&names.join(", "));
                output.push('\n');
                output.push_str("  (run /skills <name> for details on a built-in)\n");
            }
        }
    }

    let _ = write!(
        output,
        "\nUse /skill <name> to run a skill\nSkills location: {}{}",
        skills_dir.display(),
        warnings
    );

    CommandResult::message(output)
}

/// Run a specific skill — activates skill for next user message, or
/// dispatches a sub-command (`install`, `update`, `uninstall`, `trust`).
/// Try to run a skill by exact name (used for unified slash-command namespace, #435).
/// Returns None when no skill with that name exists, so the caller can try other sources.
pub fn run_skill_by_name(app: &mut App, name: &str, _arg: Option<&str>) -> Option<CommandResult> {
    let registry = discover_visible_skills(app);
    if registry.get(name).is_some() {
        Some(activate_skill(app, name))
    } else {
        None
    }
}

pub fn run_skill(app: &mut App, name: Option<&str>) -> CommandResult {
    let raw = match name {
        Some(n) => n.trim(),
        None => {
            return CommandResult::error(
                "Usage: /skill <name>\n\nSubcommands:\n  /skill install <github:owner/repo|https://…|<registry-name>>\n  /skill update <name>\n  /skill uninstall <name>\n  /skill trust <name>",
            );
        }
    };

    // Sub-command dispatch happens before the activation path so users can't
    // accidentally activate a skill literally named "install".
    let mut iter = raw.splitn(2, char::is_whitespace);
    let head = iter.next().unwrap_or("").trim();
    let rest = iter.next().unwrap_or("").trim();
    match head {
        "install" => return install_skill(app, rest),
        "update" => return update_skill(app, rest),
        "uninstall" => return uninstall_skill(app, rest),
        "trust" => return trust_skill(app, rest),
        _ => {}
    }

    activate_skill(app, raw)
}

fn activate_skill(app: &mut App, name: &str) -> CommandResult {
    // `/skill new` is a friendly alias for `/skill skill-creator`.
    let name = if name == "new" { "skill-creator" } else { name };

    let registry = discover_visible_skills(app);

    if let Some(skill) = registry.get(name) {
        let instruction = format!(
            "You are now using a skill. Follow these instructions:\n\n# Skill: {}\n\n{}\n\n---\n\nNow respond to the user's request following the above skill instructions.",
            skill.name, skill.body
        );

        app.add_message(HistoryCell::System {
            content: format!("Activated skill: {}\n\n{}", skill.name, skill.description),
        });

        app.active_skill = Some(instruction);

        CommandResult::message(format!(
            "Skill '{}' activated.\n\nDescription: {}\n\nType your request and the skill instructions will be applied.",
            skill.name, skill.description
        ))
    } else {
        let available: Vec<String> = registry.list().iter().map(|s| s.name.clone()).collect();
        let warnings = render_skill_warnings(&registry);

        if available.is_empty() {
            CommandResult::error(format!(
                "Skill '{name}' not found. No skills installed.\n\nUse /skills to see how to add skills.{warnings}"
            ))
        } else {
            CommandResult::error(format!(
                "Skill '{}' not found.\n\nAvailable skills: {}{}",
                name,
                available.join(", "),
                warnings
            ))
        }
    }
}

// ─── /skill install ────────────────────────────────────────────────────────

fn install_skill(app: &mut App, spec: &str) -> CommandResult {
    if spec.is_empty() {
        return CommandResult::error(
            "Usage: /skill install <github:owner/repo|https://…|<registry-name>>",
        );
    }
    let source = match InstallSource::parse(spec) {
        Ok(s) => s,
        Err(err) => return CommandResult::error(format!("Invalid install source: {err}")),
    };
    let skills_dir = app.skills_dir.clone();
    let (network, max_size, registry_url) = installer_settings(app);

    let outcome = run_async(async move {
        install::install_with_registry(
            source,
            &skills_dir,
            max_size,
            &network,
            false,
            &registry_url,
        )
        .await
    });

    match outcome {
        Ok(InstallOutcome::Installed(installed)) => {
            app.refresh_skill_cache();
            let path_str = path_or_default(&installed.path);
            CommandResult::message(format!(
                "Installed skill '{}' from {}.\nLocation: {}\n\nRun /skills to see it in the list.",
                installed.name, spec, path_str
            ))
        }
        Ok(InstallOutcome::NeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(InstallOutcome::NetworkDenied(host)) => {
            CommandResult::error(network_denied_message(&host))
        }
        Err(err) => CommandResult::error(format!("Install failed: {err:#}")),
    }
}

// ─── /skill update ─────────────────────────────────────────────────────────

fn update_skill(app: &mut App, name: &str) -> CommandResult {
    if name.is_empty() {
        return CommandResult::error("Usage: /skill update <name>");
    }
    let skills_dir = app.skills_dir.clone();
    let (network, max_size, registry_url) = installer_settings(app);
    let owned_name = name.to_string();
    let outcome = run_async(async move {
        install::update_with_registry(&owned_name, &skills_dir, max_size, &network, &registry_url)
            .await
    });

    match outcome {
        Ok(UpdateResult::NoChange) => {
            CommandResult::message(format!("Skill '{name}': no upstream change."))
        }
        Ok(UpdateResult::Updated(installed)) => CommandResult::message(format!(
            "Skill '{}' updated. Location: {}",
            installed.name,
            path_or_default(&installed.path)
        )),
        Ok(UpdateResult::NeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(UpdateResult::NetworkDenied(host)) => {
            CommandResult::error(network_denied_message(&host))
        }
        Err(err) => CommandResult::error(format!("Update failed: {err:#}")),
    }
}

// ─── /skill uninstall ──────────────────────────────────────────────────────

fn uninstall_skill(app: &mut App, name: &str) -> CommandResult {
    if name.is_empty() {
        return CommandResult::error("Usage: /skill uninstall <name>");
    }
    match install::uninstall(name, &app.skills_dir) {
        Ok(()) => {
            app.refresh_skill_cache();
            CommandResult::message(format!("Removed skill '{name}'."))
        }
        Err(err) => CommandResult::error(format!("Uninstall failed: {err:#}")),
    }
}

// ─── /skill trust ──────────────────────────────────────────────────────────

fn trust_skill(app: &mut App, name: &str) -> CommandResult {
    if name.is_empty() {
        return CommandResult::error("Usage: /skill trust <name>");
    }
    match install::trust(name, &app.skills_dir) {
        Ok(()) => CommandResult::message(format!(
            "Marked skill '{name}' as trusted. Tools that consult the .trusted marker may now invoke its scripts/."
        )),
        Err(err) => CommandResult::error(format!("Trust failed: {err:#}")),
    }
}

// ─── /skills --remote ──────────────────────────────────────────────────────

/// List skills available in the configured curated registry.
pub fn list_remote_skills(app: &mut App) -> CommandResult {
    let (network, _max_size, registry_url) = installer_settings(app);
    let registry = run_async(async move { install::fetch_registry(&network, &registry_url).await });
    match registry {
        Ok(RegistryFetchResult::Loaded(doc)) => {
            if doc.skills.is_empty() {
                return CommandResult::message("Registry is empty.");
            }
            let mut out = format!("Available remote skills ({}):\n", doc.skills.len());
            out.push_str("─────────────────────────────\n");
            for (name, entry) in &doc.skills {
                let _ = writeln!(
                    out,
                    "  {name} — {} (source: {})",
                    entry.description.clone().unwrap_or_default(),
                    entry.source
                );
            }
            let _ = write!(out, "\nInstall with: /skill install <name>");
            CommandResult::message(out)
        }
        Ok(RegistryFetchResult::NeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(RegistryFetchResult::Denied(host)) => {
            CommandResult::error(network_denied_message(&host))
        }
        Err(err) => CommandResult::error(format_registry_error("Failed to fetch registry", &err)),
    }
}

// ─── /skills sync ──────────────────────────────────────────────────────────

/// Fetch the remote registry index and download every listed skill into the
/// local cache (`~/.deepseek/cache/skills/<name>/`).
///
/// For each skill the sync checks the cached ETag / SHA-256 before
/// downloading so unchanged skills are skipped in O(1) network round-trips.
fn sync_skills(app: &mut App) -> CommandResult {
    let (network, max_size, registry_url) = installer_settings(app);
    let cache_dir = install::default_cache_skills_dir();

    let result = run_async(async move {
        install::sync_registry(&network, &registry_url, &cache_dir, max_size).await
    });

    match result {
        Ok(SyncResult::RegistryDenied(host)) => CommandResult::error(network_denied_message(&host)),
        Ok(SyncResult::RegistryNeedsApproval(host)) => {
            CommandResult::error(needs_approval_message(&host))
        }
        Ok(SyncResult::Done { outcomes }) => {
            let total = outcomes.len();
            let mut downloaded = 0usize;
            let mut fresh = 0usize;
            let mut failed = 0usize;
            let mut out = String::from("Registry sync complete.\n\n");

            for outcome in &outcomes {
                match outcome {
                    SkillSyncOutcome::Downloaded { name, path } => {
                        downloaded += 1;
                        let _ = writeln!(out, "  [+] {name} — downloaded to {}", path.display());
                    }
                    SkillSyncOutcome::Fresh { name } => {
                        fresh += 1;
                        let _ = writeln!(out, "  [=] {name} — already up to date");
                    }
                    SkillSyncOutcome::Failed { name, reason } => {
                        failed += 1;
                        let _ = writeln!(out, "  [!] {name} — failed: {reason}");
                    }
                    SkillSyncOutcome::Denied { name, host } => {
                        failed += 1;
                        let _ = writeln!(out, "  [x] {name} — network denied ({host})");
                    }
                    SkillSyncOutcome::NeedsApproval { name, host } => {
                        failed += 1;
                        let _ = writeln!(
                            out,
                            "  [?] {name} — needs approval for {host} (run `/network allow {host}` then retry)"
                        );
                    }
                }
            }

            let _ = write!(
                out,
                "\n{total} skill(s) processed: {downloaded} downloaded, {fresh} up-to-date, {failed} failed."
            );

            CommandResult::message(out)
        }
        Err(err) => CommandResult::error(format_registry_error("Sync failed", &err)),
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

/// Read the active config knobs for the installer.
///
/// We load `Config::load` on demand because [`App`] does not carry a `Config`
/// field — and loading is cheap (small TOML file) compared to the network
/// round-trip the install/update operation will incur next. If the config
/// fails to parse, we fall back to defaults so the user still gets a
/// network-gated install rather than a silent crash.
fn installer_settings(_app: &App) -> (NetworkPolicy, u64, String) {
    let cfg = crate::config::Config::load(None, None).unwrap_or_default();
    let network = cfg
        .network
        .clone()
        .map(|policy| policy.into_runtime())
        .unwrap_or_default();
    let skills_cfg = cfg.skills.as_ref();
    let max_size = skills_cfg
        .and_then(|s| s.max_install_size_bytes)
        .unwrap_or(DEFAULT_MAX_SIZE_BYTES);
    let registry_url = skills_cfg
        .and_then(|s| s.registry_url.clone())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());
    (network, max_size, registry_url)
}

fn run_async<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // We're on the TUI's thread, which is part of the multi-threaded runtime.
    // `block_in_place` + `Handle::current().block_on` is the pattern used by
    // `commands/cycle.rs` to bridge sync slash-command handlers back into the
    // async ecosystem.
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

fn path_or_default(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| {
            // Display with parent so the user sees the full skill location.
            // We intentionally use `display()` here because it's just for
            // user-facing output, not for path comparisons.
            let parent = path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            if parent.is_empty() {
                n.to_string_lossy().to_string()
            } else {
                format!("{parent}/{}", n.to_string_lossy())
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}

fn needs_approval_message(host: &str) -> String {
    format!(
        "Network policy requires approval for {host}.\n\
         Add it to your allow list with `/network allow {host}` (or set [network].default = \"allow\" in ~/.deepseek/config.toml), then retry."
    )
}

fn network_denied_message(host: &str) -> String {
    format!(
        "Network policy denied access to {host}.\n\
         Remove the deny entry from ~/.deepseek/config.toml under [network] or contact your administrator."
    )
}

/// Inspect an anyhow chain and surface a one-line hint pointing at the most
/// common cause of a registry fetch failure (DNS, refused, TLS, HTTP status,
/// timeout). The chain itself is still rendered with `{err:#}`; this hint is
/// appended below it so users on `/skills --remote` and `/skills sync` get an
/// actionable next step instead of an opaque reqwest error.
fn registry_fetch_error_hint(err: &anyhow::Error) -> Option<&'static str> {
    let msg = format!("{err:#}").to_lowercase();
    if msg.contains("dns")
        || msg.contains("name resolution")
        || msg.contains("getaddrinfo")
        || msg.contains("nodename nor servname")
    {
        Some(
            "Hint: DNS lookup failed. Check internet/DNS connectivity, or override the registry URL in [skills] of ~/.deepseek/config.toml.",
        )
    } else if msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("connection aborted")
    {
        Some(
            "Hint: connection refused/reset. The registry host may be unreachable from this network (corporate proxy, firewall, offline).",
        )
    } else if msg.contains("tls")
        || msg.contains("certificate")
        || msg.contains("ssl")
        || msg.contains("handshake")
    {
        Some(
            "Hint: TLS handshake failed. The system trust store may be missing the registry's CA, or a TLS-intercepting proxy is rewriting the certificate.",
        )
    } else if msg.contains(" 404") || msg.contains("not found") {
        Some(
            "Hint: registry URL returned 404. Verify the registry URL in [skills] of ~/.deepseek/config.toml.",
        )
    } else if msg.contains(" 401") || msg.contains(" 403") || msg.contains("forbidden") {
        Some(
            "Hint: registry returned an auth error. The registry may require credentials or have been moved.",
        )
    } else if msg.contains(" 429") || msg.contains("rate limit") || msg.contains("too many") {
        Some("Hint: rate-limited by the registry. Try again in a moment.")
    } else if msg.contains("timed out") || msg.contains("timeout") {
        Some("Hint: request timed out. Network may be slow or the registry host may be down.")
    } else {
        None
    }
}

fn format_registry_error(prefix: &str, err: &anyhow::Error) -> String {
    let mut out = format!("{prefix}: {err:#}");
    if let Some(hint) = registry_fetch_error_hint(err) {
        out.push_str("\n\n");
        out.push_str(hint);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::ffi::OsString;
    use tempfile::TempDir;

    struct IsolatedHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        home_prev: Option<OsString>,
        userprofile_prev: Option<OsString>,
    }

    impl IsolatedHome {
        fn new(tmpdir: &TempDir) -> Self {
            let lock = crate::test_support::lock_test_env();
            let home = tmpdir.path().join("home");
            std::fs::create_dir_all(&home).unwrap();
            let home_prev = std::env::var_os("HOME");
            let userprofile_prev = std::env::var_os("USERPROFILE");
            // SAFETY: tests that mutate process env hold the shared test env
            // mutex for the full lifetime of this guard.
            unsafe {
                std::env::set_var("HOME", &home);
                std::env::set_var("USERPROFILE", &home);
            }
            Self {
                _lock: lock,
                home_prev,
                userprofile_prev,
            }
        }

        unsafe fn restore_var(key: &str, value: Option<OsString>) {
            if let Some(value) = value {
                unsafe { std::env::set_var(key, value) };
            } else {
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    impl Drop for IsolatedHome {
        fn drop(&mut self) {
            // SAFETY: the shared test env mutex is still held while Drop runs.
            unsafe {
                Self::restore_var("HOME", self.home_prev.take());
                Self::restore_var("USERPROFILE", self.userprofile_prev.take());
            }
        }
    }

    fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.skills_dir = tmpdir.path().join("skills");
        app
    }

    fn create_skill_dir(tmpdir: &TempDir, skill_name: &str, skill_content: &str) {
        let skill_dir = tmpdir.path().join("skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), skill_content).unwrap();
    }

    #[test]
    fn registry_fetch_error_hint_recognises_dns_failures() {
        let err = anyhow::Error::msg("error sending request: dns error: failed to lookup")
            .context("failed to fetch registry https://example.com/registry.json");
        let hint = registry_fetch_error_hint(&err).expect("dns hint");
        assert!(hint.contains("DNS"), "got: {hint}");
    }

    #[test]
    fn registry_fetch_error_hint_recognises_connection_refused() {
        let err = anyhow::Error::msg("error sending request: tcp connect: connection refused");
        let hint = registry_fetch_error_hint(&err).expect("refused hint");
        assert!(hint.contains("refused"), "got: {hint}");
    }

    #[test]
    fn registry_fetch_error_hint_recognises_tls_failures() {
        let err = anyhow::Error::msg("invalid peer certificate: UnknownIssuer (TLS handshake)");
        let hint = registry_fetch_error_hint(&err).expect("tls hint");
        assert!(hint.contains("TLS"), "got: {hint}");
    }

    #[test]
    fn registry_fetch_error_hint_recognises_http_status_codes() {
        let err_404 = anyhow::Error::msg("registry returned an error status: 404 Not Found");
        assert!(
            registry_fetch_error_hint(&err_404)
                .map(|h| h.contains("404"))
                .unwrap_or(false)
        );
        let err_429 =
            anyhow::Error::msg("registry returned an error status: 429 Too Many Requests");
        assert!(
            registry_fetch_error_hint(&err_429)
                .map(|h| h.contains("rate"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn registry_fetch_error_hint_returns_none_for_unrecognised_errors() {
        let err = anyhow::Error::msg("a totally novel error nobody anticipated");
        assert!(registry_fetch_error_hint(&err).is_none());
    }

    #[test]
    fn format_registry_error_appends_hint_when_pattern_matches() {
        let err = anyhow::Error::msg("dns error: nodename nor servname provided");
        let formatted = format_registry_error("Failed to fetch registry", &err);
        assert!(formatted.starts_with("Failed to fetch registry: "));
        assert!(
            formatted.contains("Hint: DNS"),
            "expected hint, got: {formatted}"
        );
    }

    #[test]
    fn format_registry_error_omits_hint_when_no_pattern_matches() {
        let err = anyhow::Error::msg("inscrutable opaque failure");
        let formatted = format_registry_error("Sync failed", &err);
        assert_eq!(formatted, "Sync failed: inscrutable opaque failure");
    }

    #[test]
    fn test_list_skills_empty_directory() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, None);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("No skills found"));
        assert!(msg.contains("Skills location:"));
    }

    #[test]
    fn test_list_skills_with_skills() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "test-skill",
            "---\nname: test-skill\ndescription: A test skill\n---\nDo something",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, None);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Available skills"));
        assert!(msg.contains("/test-skill"));
    }

    #[test]
    fn test_list_skills_filters_by_name_prefix() {
        // #1318: a `/skills <prefix>` argument should narrow the list to
        // skills whose names start with the prefix. The header reflects
        // both the matched count and the registry total so the user
        // knows what they're looking at.
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First\n---\nbody",
        );
        create_skill_dir(
            &tmpdir,
            "alphabet-helper",
            "---\nname: alphabet-helper\ndescription: Helper\n---\nbody",
        );
        create_skill_dir(
            &tmpdir,
            "beta-skill",
            "---\nname: beta-skill\ndescription: Second\n---\nbody",
        );

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, Some("alph"));
        let msg = result.message.expect("filter result has message");

        assert!(msg.contains("/alpha-skill"));
        assert!(msg.contains("/alphabet-helper"));
        assert!(
            !msg.contains("/beta-skill"),
            "beta-skill must be filtered out"
        );
        assert!(
            msg.contains("matching `alph`") && msg.contains("2 of 3"),
            "header should show count + total, got: {msg}"
        );
    }

    #[test]
    fn test_list_skills_filter_is_case_insensitive() {
        // Prefix matching is case-insensitive — typing `Alph` finds
        // `alpha-skill` the same as `alph` does.
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First\n---\nbody",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, Some("ALPH"));
        let msg = result.message.expect("case-insensitive filter has message");
        assert!(msg.contains("/alpha-skill"));
    }

    #[test]
    fn test_list_skills_filter_with_zero_matches_says_so() {
        // When the prefix matches nothing, the message must say so
        // explicitly (rather than printing an empty list) and point
        // the user back at the unfiltered command.
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First\n---\nbody",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, Some("nonexistent"));
        let msg = result.message.expect("zero-match filter still has message");
        assert!(msg.contains("No skills match prefix `nonexistent`"));
        assert!(msg.contains("Run /skills"));
    }

    #[test]
    fn test_list_skills_rejects_flag_like_prefix() {
        // `--remote` and `sync` stay reserved as subcommands; any other
        // dash-prefixed argument is rejected so we don't silently turn
        // a future flag into a no-match filter.
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, Some("--bogus"));
        assert!(
            result.is_error,
            "expected usage error for --bogus, got: {result:?}"
        );
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("name-prefix")),
            "expected --bogus error message to mention name-prefix, got: {result:?}"
        );
    }

    #[test]
    fn test_list_skills_renders_user_skills_under_your_skills_section() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: First skill\n---\nDo alpha work",
        );
        create_skill_dir(
            &tmpdir,
            "beta-skill",
            "---\nname: beta-skill\ndescription: Second skill\n---\nDo beta work",
        );

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, None);
        let msg = result.message.unwrap();

        // User-created skills must appear in their own section so they
        // stay visible even when many bundled skills are installed.
        let section = msg
            .find("Your skills")
            .expect("user skills section header missing");
        let alpha = msg.find("/alpha-skill").expect("alpha skill should render");
        let beta = msg.find("/beta-skill").expect("beta skill should render");
        assert!(
            alpha > section,
            "alpha-skill should follow the header: {msg}"
        );
        assert!(beta > section, "beta-skill should follow the header: {msg}");
        // Each entry on its own line with the description inline.
        assert!(msg.contains("/alpha-skill - First skill"), "got: {msg}");
        assert!(msg.contains("/beta-skill - Second skill"), "got: {msg}");
    }

    #[test]
    fn test_list_skills_merges_workspace_and_configured_dirs() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let workspace_skill_dir = tmpdir
            .path()
            .join(".agents")
            .join("skills")
            .join("workspace-skill");
        std::fs::create_dir_all(&workspace_skill_dir).unwrap();
        std::fs::write(
            workspace_skill_dir.join("SKILL.md"),
            "---\nname: workspace-skill\ndescription: Workspace skill\n---\nDo workspace work",
        )
        .unwrap();
        create_skill_dir(
            &tmpdir,
            "configured-skill",
            "---\nname: configured-skill\ndescription: Configured skill\n---\nDo configured work",
        );

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = list_skills(&mut app, None);
        let msg = result.message.unwrap();

        assert!(msg.contains("/workspace-skill"), "got: {msg}");
        assert!(msg.contains("/configured-skill"), "got: {msg}");
    }

    #[test]
    fn test_skill_subcommand_dispatch_install_usage() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        // Empty install spec → usage hint, not invalid-source error.
        let result = run_skill(&mut app, Some("install"));
        let msg = result.message.unwrap();
        assert!(msg.contains("/skill install"), "got: {msg}");
    }

    #[test]
    fn test_skill_subcommand_dispatch_uninstall_missing() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = run_skill(&mut app, Some("uninstall absent-skill"));
        let msg = result.message.unwrap();
        assert!(msg.contains("not installed"), "got: {msg}");
    }

    #[test]
    fn test_run_skill_without_name() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = run_skill(&mut app, None);
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Usage: /skill"));
    }

    #[test]
    fn test_run_skill_not_found() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = run_skill(&mut app, Some("nonexistent"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn test_run_skill_activates() {
        let tmpdir = TempDir::new().unwrap();
        let _home = IsolatedHome::new(&tmpdir);
        create_skill_dir(
            &tmpdir,
            "test-skill",
            "---\nname: test-skill\ndescription: A test skill\n---\nDo something special",
        );
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = run_skill(&mut app, Some("test-skill"));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Skill 'test-skill' activated"));
        assert!(msg.contains("A test skill"));
        assert!(app.active_skill.is_some());
        assert!(!app.history.is_empty());
    }
}
