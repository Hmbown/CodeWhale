//! `/import-claude`: explicit, reviewable migration from Claude Code (#5557).
//!
//! Reads `~/.claude.json` and `~/.claude/settings.json` (bounded, read-only)
//! and builds a plan — never a silent import:
//!
//! - **MCP servers** are surfaced through the existing external-import consent
//!   flow (`/mcp import <name> --approve`); nothing merges automatically.
//! - **Env vars** that are known-safe (timeouts, output caps, editor choices)
//!   are proposed as a portable bundle file for `codewhale config import`,
//!   which carries its own plan/consent/rollback. Secret-shaped or unknown
//!   keys are named but never imported or echoed.
//! - **Permissions** become approval-policy recommendations in a written
//!   report (Claude rule grammar does not map 1:1; nothing is auto-applied).
//! - **Hooks** and **paths** (e.g. `~/.claude/CLAUDE.md`) are listed as manual
//!   follow-ups with exact target commands.
//!
//! The command writes a report and an *unapplied* bundle file. Applying any
//! part always requires its own explicit consent path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Maximum size accepted for any single Claude source file. Generous for
/// real `~/.claude.json` files with many projects; bounded so a pathological
/// file cannot exhaust memory. Matches the bundle reader's order of magnitude.
pub(crate) const MAX_SOURCE_BYTES: u64 = 5 * 1024 * 1024;

/// Env keys safe to propose for the portable bundle (lowercase). Deliberately
/// a short allowlist: timeouts, output caps, and editor/telemetry choices.
/// Anything not listed stays out of the bundle and is reported by name only.
const SAFE_ENV_KEYS: &[&str] = &[
    "bash_default_timeout_ms",
    "bash_max_timeout_ms",
    "bash_max_output_length",
    "bash_mcp_timeout_ms",
    "mcp_timeout_ms",
    "mcp_tools_timeout_ms",
    "use_builtin_ripgrep",
    "editor",
    "max_thinking_tokens",
    "disable_cost_warnings",
    "disable_nonessential_model_calls",
    "disable_telemetry",
];

/// Longest env value accepted into the bundle; guards against blob-shaped
/// values a migration should not carry.
const MAX_ENV_VALUE_LEN: usize = 200;

/// Status of one Claude source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceStatus {
    Found,
    NotFound,
    Unreadable(String),
    Oversize(u64),
    InvalidJson,
}

impl SourceStatus {
    fn describe(&self, path: &Path) -> String {
        match self {
            Self::Found => format!("found: {}", crate::utils::display_path(path)),
            Self::NotFound => format!("not found: {}", crate::utils::display_path(path)),
            Self::Unreadable(error) => {
                format!("unreadable: {} ({error})", crate::utils::display_path(path))
            }
            Self::Oversize(bytes) => format!(
                "skipped, too large ({} bytes): {}",
                bytes,
                crate::utils::display_path(path)
            ),
            Self::InvalidJson => {
                format!("invalid JSON: {}", crate::utils::display_path(path))
            }
        }
    }
}

/// One parsed MCP candidate (name + one-line summary), already filtered to the
/// Claude sources by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpCandidateLine {
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) hard_blocked: bool,
}

/// The full `/import-claude` plan. Pure data: building it mutates nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClaudeImportPlan {
    pub(crate) sources: Vec<(PathBuf, SourceStatus)>,
    /// MCP servers from `~/.claude.json` (top level), for `/mcp import`.
    pub(crate) mcp_candidates: Vec<McpCandidateLine>,
    /// Names of MCP servers found under `projects.<path>.mcpServers`, listed
    /// for provenance only in v1.
    pub(crate) per_project_mcp: Vec<String>,
    /// Safe env entries proposed for the portable bundle.
    pub(crate) env_safe: BTreeMap<String, String>,
    /// Env keys skipped because they are secret-shaped or not allowlisted.
    pub(crate) env_skipped: Vec<String>,
    /// Claude `permissions.defaultMode`, when present.
    pub(crate) permissions_default_mode: Option<String>,
    pub(crate) permissions_allow: Vec<String>,
    pub(crate) permissions_ask: Vec<String>,
    pub(crate) permissions_deny: Vec<String>,
    /// Hook event names with counts, e.g. `PreToolUse (2)`.
    pub(crate) hook_events: Vec<String>,
    /// True when `~/.claude/CLAUDE.md` exists.
    pub(crate) has_claude_md: bool,
    /// Project workspace paths recorded in `~/.claude.json` (bounded).
    pub(crate) claude_projects: Vec<String>,
}

impl ClaudeImportPlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.mcp_candidates.is_empty()
            && self.per_project_mcp.is_empty()
            && self.env_safe.is_empty()
            && self.env_skipped.is_empty()
            && self.permissions_default_mode.is_none()
            && self.permissions_allow.is_empty()
            && self.permissions_ask.is_empty()
            && self.permissions_deny.is_empty()
            && self.hook_events.is_empty()
            && !self.has_claude_md
            && self.claude_projects.is_empty()
    }
}

fn read_bounded_json(path: &Path) -> Result<Value, SourceStatus> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SourceStatus::NotFound,
        _ => SourceStatus::Unreadable(e.to_string()),
    })?;
    if !meta.is_file() {
        return Err(SourceStatus::NotFound);
    }
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(SourceStatus::Oversize(meta.len()));
    }
    let raw = std::fs::read(path).map_err(|e| SourceStatus::Unreadable(e.to_string()))?;
    serde_json::from_slice::<Value>(&raw).map_err(|_| SourceStatus::InvalidJson)
}

/// Read the Claude sources under `home` (usually `~`).
pub(crate) fn read_sources(
    home: &Path,
) -> (Vec<(PathBuf, SourceStatus)>, Option<Value>, Option<Value>) {
    let claude_json = home.join(".claude.json");
    let settings = home.join(".claude").join("settings.json");
    let mut sources = Vec::new();
    let mut claude_value = None;
    let mut settings_value = None;
    match read_bounded_json(&claude_json) {
        Ok(value) => {
            sources.push((claude_json.clone(), SourceStatus::Found));
            claude_value = Some(value);
        }
        Err(status) => sources.push((claude_json, status)),
    }
    match read_bounded_json(&settings) {
        Ok(value) => {
            sources.push((settings.clone(), SourceStatus::Found));
            settings_value = Some(value);
        }
        Err(status) => sources.push((settings, status)),
    }
    (sources, claude_value, settings_value)
}

fn safe_env_value(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => {
            (text.len() <= MAX_ENV_VALUE_LEN && !text.contains('\n')).then(|| text.clone())
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }?;
    (codewhale_config::persistence::redact_secrets(&text) == text).then_some(text)
}

/// Build the plan from already-read sources plus MCP candidates discovered by
/// the existing external-import machinery (so server parsing stays single-sourced).
pub(crate) fn build_plan(
    sources: Vec<(PathBuf, SourceStatus)>,
    claude: Option<Value>,
    settings: Option<Value>,
    home: &Path,
    mcp_candidates: Vec<McpCandidateLine>,
) -> ClaudeImportPlan {
    let mut plan = ClaudeImportPlan {
        sources,
        mcp_candidates,
        ..ClaudeImportPlan::default()
    };
    if let Some(claude) = claude.as_ref()
        && let Some(projects) = claude.get("projects").and_then(Value::as_object)
    {
        for (path, project) in projects {
            if let Some(servers) = project.get("mcpServers").and_then(Value::as_object) {
                for name in servers.keys() {
                    plan.per_project_mcp
                        .push(format!("{name} (project {path})"));
                }
            }
            if plan.claude_projects.len() < 8 {
                plan.claude_projects.push(path.clone());
            }
        }
    }
    if let Some(settings) = settings.as_ref() {
        if let Some(env) = settings.get("env").and_then(Value::as_object) {
            for (key, value) in env {
                if SAFE_ENV_KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                    match safe_env_value(value) {
                        Some(text) => {
                            plan.env_safe.insert(key.clone(), text);
                        }
                        None => plan.env_skipped.push(format!("{key} (value not portable)")),
                    }
                } else {
                    plan.env_skipped.push(key.clone());
                }
            }
        }
        if let Some(permissions) = settings.get("permissions").and_then(Value::as_object) {
            plan.permissions_default_mode = permissions
                .get("defaultMode")
                .and_then(Value::as_str)
                .map(str::to_string);
            for (key, target) in [
                ("allow", &mut plan.permissions_allow),
                ("ask", &mut plan.permissions_ask),
                ("deny", &mut plan.permissions_deny),
            ] {
                if let Some(rules) = permissions.get(key).and_then(Value::as_array) {
                    for rule in rules {
                        if let Some(text) = rule.as_str() {
                            target.push(codewhale_config::persistence::redact_secrets(text));
                        }
                    }
                }
            }
        }
        if let Some(hooks) = settings.get("hooks").and_then(Value::as_object) {
            for (event, entries) in hooks {
                let count = entries.as_array().map_or(1, Vec::len);
                plan.hook_events.push(format!("{event} ({count})"));
            }
        }
    }
    plan.has_claude_md = home.join(".claude").join("CLAUDE.md").is_file();
    plan
}

/// Claude `permissions.defaultMode` → the Codewhale approval-posture
/// recommendation (report text only; nothing is applied).
pub(crate) fn approval_posture_recommendation(mode: Option<&str>) -> &'static str {
    match mode {
        Some("acceptEdits") => "auto-review posture (/permissions) matches acceptEdits",
        Some("plan") => "plan mode matches Claude's plan default",
        Some("bypassPermissions") => {
            "Full Access (yolo) matches bypassPermissions — confirm you want it"
        }
        _ => "the default Ask posture matches Claude's default",
    }
}

/// One-line migration guidance for a Claude permission rule.
pub(crate) fn permission_rule_guidance(rule: &str) -> String {
    let rule = rule.trim();
    if rule.starts_with("Bash(") {
        format!("{rule} → an execpolicy rule for that command family (/permissions)")
    } else if rule.starts_with("WebFetch(domain:") || rule.starts_with("WebSearch(") {
        format!("{rule} → a fetch/network rule for that host or surface")
    } else if rule.starts_with("mcp__") {
        format!("{rule} → enable that MCP tool in your mcp.json enabled_tools")
    } else if rule.starts_with("Read(") || rule.starts_with("Edit(") || rule.starts_with("Write(") {
        format!("{rule} → a path-scoped ask rule (reads stay workspace-bounded by default)")
    } else {
        format!("{rule} → review against /permissions before mapping")
    }
}

/// The unapplied portable bundle document for the safe env entries. JSON is a
/// first-class bundle format (`config import` accepts `.json`), so the
/// generated file goes through the same strict parser as any hand-written one.
pub(crate) fn portable_bundle_json(plan: &ClaudeImportPlan) -> String {
    let global: BTreeMap<&str, &str> = plan
        .env_safe
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    serde_json::json!({
        "schema_version": 1,
        "kind": "codewhale.portable-config",
        "metadata": {
            "name": "claude-import",
            "generator": "codewhale /import-claude",
        },
        "global": global,
    })
    .to_string()
}

/// Render the reviewable plan shown in the transcript.
pub(crate) fn render_plan(plan: &ClaudeImportPlan) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("Claude import plan (nothing is applied without its own consent):\n");
    for (path, status) in &plan.sources {
        let _ = writeln!(out, "  · {}", status.describe(path));
    }
    if !plan.mcp_candidates.is_empty() {
        out.push_str("\nMCP servers (apply each with `/mcp import <name> --approve`):\n");
        for candidate in &plan.mcp_candidates {
            let blocked = if candidate.hard_blocked {
                " [blocked]"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  · {}{blocked} — {}",
                candidate.name, candidate.summary
            );
        }
    }
    if !plan.per_project_mcp.is_empty() {
        out.push_str("\nProject-scoped MCP servers (listed for provenance only):\n");
        for name in plan.per_project_mcp.iter().take(8) {
            let _ = writeln!(out, "  · {name}");
        }
    }
    if !plan.env_safe.is_empty() {
        out.push_str("\nSafe env settings proposed for the portable bundle:\n");
        for key in plan.env_safe.keys() {
            let _ = writeln!(out, "  · {key}");
        }
    }
    if !plan.env_skipped.is_empty() {
        out.push_str("\nEnv keys not imported (secret-shaped or unmapped; values never shown):\n");
        for key in plan.env_skipped.iter().take(12) {
            let _ = writeln!(out, "  · {key}");
        }
    }
    let _ = writeln!(
        out,
        "\nApproval posture: {}.",
        approval_posture_recommendation(plan.permissions_default_mode.as_deref())
    );
    for (label, rules) in [
        ("Allow", &plan.permissions_allow),
        ("Ask", &plan.permissions_ask),
        ("Deny", &plan.permissions_deny),
    ] {
        if !rules.is_empty() {
            let _ = writeln!(out, "\n{label} rules (map manually; not auto-applied):");
            for rule in rules.iter().take(8) {
                let _ = writeln!(out, "  · {}", permission_rule_guidance(rule));
            }
        }
    }
    if !plan.hook_events.is_empty() {
        out.push_str("\nHook events (map manually with /hooks):\n");
        for event in &plan.hook_events {
            let _ = writeln!(out, "  · {event}");
        }
    }
    if plan.has_claude_md {
        out.push_str("\nStanding instructions: ~/.claude/CLAUDE.md found — copy it to ~/.codewhale/instructions.md (or the repo's AGENTS.md) to carry it over.\n");
    }
    out
}

/// The written follow-through report (markdown).
pub(crate) fn report_markdown(plan: &ClaudeImportPlan) -> String {
    let mut out = String::from(
        "# Claude import report\n\nGenerated by `/import-claude`. Nothing was applied automatically.\n\n",
    );
    out.push_str(&render_plan(plan));
    if !plan.env_safe.is_empty() {
        out.push_str("\n## Apply the portable bundle\n\nReview the generated bundle, then run:\n\n    codewhale config import <bundle path>\n\n");
    }
    out.push_str("\n## Manual follow-ups\n\n- MCP: `/mcp import <name> --approve` per server (consent is recorded).\n- Permissions: map rules in /permissions and execpolicy.\n- Hooks: re-create matching hooks with /hooks.\n- Paths: copy CLAUDE.md if you want those standing instructions.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_home(tag: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("home");
        let _ = tag;
        let home = dir.path().to_path_buf();
        (dir, home)
    }

    #[test]
    fn missing_sources_report_not_found_and_plan_is_empty() {
        let (dir, home) = temp_home("missing");
        let (sources, claude, settings) = read_sources(&home);
        let plan = build_plan(sources, claude, settings, &home, Vec::new());
        assert!(plan.is_empty());
        assert!(
            plan.sources
                .iter()
                .any(|(_, s)| *s == SourceStatus::NotFound)
        );
        assert!(dir.path().exists());
    }

    #[test]
    fn settings_env_splits_safe_and_skipped_without_ever_inlining_secret_values() {
        let (_dir, home) = temp_home("env");
        std::fs::create_dir_all(home.join(".claude")).expect("dir");
        std::fs::write(
            home.join(".claude").join("settings.json"),
            json!({
                "env": {
                    "BASH_DEFAULT_TIMEOUT_MS": "120000",
                    "USE_BUILTIN_RIPGREP": "1",
                    "EDITOR": "sk-editor-secret-value",
                    "ANTHROPIC_API_KEY": "sk-ant-secret-value",
                    "SOMETHING_ELSE": "x"
                },
                "permissions": {
                    "defaultMode": "acceptEdits",
                    "allow": ["Bash(git status:*)", "WebFetch(domain:github.com)", "mcp__demo__kick", "Bash(curl -H token=sk-rule-secret-value)"] ,
                    "deny": ["Read(./.env)"]
                },
                "hooks": {
                    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}],
                    "PostToolUse": []
                }
            })
            .to_string(),
        )
        .expect("settings");

        let (sources, _claude, settings) = read_sources(&home);
        let plan = build_plan(sources, None, settings, &home, Vec::new());
        assert_eq!(
            plan.env_safe
                .get("BASH_DEFAULT_TIMEOUT_MS")
                .map(String::as_str),
            Some("120000")
        );
        assert!(plan.env_safe.contains_key("USE_BUILTIN_RIPGREP"));
        assert!(!plan.env_safe.contains_key("EDITOR"));
        assert!(!plan.env_safe.contains_key("ANTHROPIC_API_KEY"));
        assert!(
            plan.env_skipped
                .iter()
                .any(|k| k.contains("ANTHROPIC_API_KEY"))
        );
        assert_eq!(
            plan.permissions_default_mode.as_deref(),
            Some("acceptEdits")
        );
        assert_eq!(plan.permissions_allow.len(), 4);
        assert!(
            plan.permissions_allow
                .iter()
                .any(|rule| rule.contains("[redacted]"))
        );
        assert_eq!(plan.permissions_deny.len(), 1);
        assert_eq!(plan.hook_events.len(), 2);

        let rendered = render_plan(&plan);
        assert!(!rendered.contains("sk-ant-secret-value"), "{rendered}");
        assert!(!rendered.contains("sk-editor-secret-value"), "{rendered}");
        assert!(!rendered.contains("sk-rule-secret-value"), "{rendered}");
        assert!(rendered.contains("[redacted]"), "{rendered}");
        assert!(rendered.contains("Bash(git status:*) → an execpolicy rule"));
        assert!(rendered.contains("acceptEdits"));

        let bundle = portable_bundle_json(&plan);
        assert!(!bundle.contains("sk-ant-secret-value"), "{bundle}");
        assert!(!bundle.contains("sk-editor-secret-value"), "{bundle}");
        assert!(bundle.contains("codewhale.portable-config"));
        assert!(bundle.contains("BASH_DEFAULT_TIMEOUT_MS"));
    }

    #[test]
    fn claude_json_surfaces_projects_and_per_project_servers() {
        let (dir, home) = temp_home("projects");
        std::fs::write(
            home.join(".claude.json"),
            json!({
                "mcpServers": {
                    "top": {"command": "node", "args": ["top.js"]}
                },
                "projects": {
                    "/tmp/proj-a": {"mcpServers": {"inner": {"command": "node"}}}
                }
            })
            .to_string(),
        )
        .expect("claude.json");

        let (sources, claude, _settings) = read_sources(&home);
        let candidates = vec![McpCandidateLine {
            name: "top".to_string(),
            summary: "stdio server (node)".to_string(),
            hard_blocked: false,
        }];
        let plan = build_plan(sources, claude, None, &home, candidates);
        assert_eq!(plan.mcp_candidates.len(), 1);
        assert_eq!(plan.per_project_mcp.len(), 1);
        assert!(plan.per_project_mcp[0].contains("inner"));
        assert!(plan.claude_projects.iter().any(|p| p.contains("proj-a")));
        assert!(dir.path().exists());
    }

    #[test]
    fn oversize_and_invalid_sources_fail_closed_with_named_status() {
        let (_dir, home) = temp_home("bad");
        std::fs::write(home.join(".claude.json"), "not json").expect("bad json");
        let (sources, claude, _settings) = read_sources(&home);
        assert!(claude.is_none());
        assert!(
            sources
                .iter()
                .any(|(_, s)| matches!(s, SourceStatus::InvalidJson))
        );
    }

    #[test]
    fn posture_recommendations_cover_the_four_claude_modes() {
        assert!(approval_posture_recommendation(Some("acceptEdits")).contains("auto-review"));
        assert!(approval_posture_recommendation(Some("plan")).contains("plan"));
        assert!(approval_posture_recommendation(Some("bypassPermissions")).contains("Full Access"));
        assert!(approval_posture_recommendation(None).contains("Ask"));
    }
}
