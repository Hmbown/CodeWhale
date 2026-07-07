//! Fleet profile vocabulary, local profile discovery, and config-facing aliases.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;

#[allow(unused_imports)]
pub use codewhale_config::{
    FleetDelegationHints, FleetLoadout, FleetProfile, FleetProfilePermissions, FleetRole, FleetSlot,
};

pub use super::roster::ProfileOrigin;

pub const WORKSPACE_AGENT_PROFILE_DIR: &str = ".codewhale/agents";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub profile: FleetProfile,
    pub source: PathBuf,
    /// Roster layer this profile came from (#fleet-roster cutover (v0.8.67)).
    /// File-based loading in this module always yields `Workspace`; the
    /// roster stamps `BuiltIn` / `Config` for the other layers.
    pub origin: ProfileOrigin,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileToml {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    role_hint: Option<String>,
    #[serde(default)]
    base_role: Option<String>,
    #[serde(default)]
    persona: Option<String>,
    #[serde(default)]
    loadout: Option<String>,
    #[serde(default, alias = "model_hint", alias = "model_id")]
    model: Option<String>,
    #[serde(default)]
    instructions: Option<AgentProfileInstructions>,
    #[serde(default)]
    tools: Option<AgentProfileTools>,
    #[serde(default)]
    permissions: Option<AgentProfilePermissionsToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileInstructions {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileTools {
    #[serde(default)]
    posture: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfilePermissionsToml {
    #[serde(default)]
    allow_shell: Option<bool>,
    #[serde(default)]
    trust: Option<bool>,
    #[serde(default)]
    approval_required: Option<bool>,
}

pub fn load_workspace_agent_profiles(workspace: impl AsRef<Path>) -> Result<Vec<AgentProfile>> {
    load_agent_profiles_from_dir(workspace.as_ref().join(WORKSPACE_AGENT_PROFILE_DIR))
}

pub fn load_agent_profiles_from_dir(dir: impl AsRef<Path>) -> Result<Vec<AgentProfile>> {
    let dir = dir.as_ref();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    if !dir.is_dir() {
        bail!("agent profile path {} is not a directory", dir.display());
    }

    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading agent profile dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("reading agent profile entries in {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());

    let mut profiles = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let profile = load_agent_profile_file(&path)?;
        if !seen.insert(profile.id.clone()) {
            bail!("duplicate agent profile id {}", profile.id);
        }
        profiles.push(profile);
    }
    Ok(profiles)
}

fn load_agent_profile_file(path: &Path) -> Result<AgentProfile> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading agent profile {}", path.display()))?;
    let parsed: AgentProfileToml = toml::from_str(&raw)
        .map_err(|err| anyhow!("parsing agent profile {}: {err}", path.display()))?;
    agent_profile_from_toml(path, parsed)
}

fn agent_profile_from_toml(path: &Path, parsed: AgentProfileToml) -> Result<AgentProfile> {
    reject_permission_expansion(path, parsed.tools.as_ref(), parsed.permissions.as_ref())?;

    let fallback_id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("profile");
    let id = first_present([parsed.id.as_deref(), parsed.name.as_deref()])
        .unwrap_or(fallback_id)
        .to_string();
    validate_agent_profile_token(path, "id/name", &id)?;

    let role_name = first_present([
        parsed.base_role.as_deref(),
        parsed.role_hint.as_deref(),
        parsed.name.as_deref(),
    ])
    .unwrap_or(&id)
    .to_string();
    validate_agent_profile_token(path, "base_role/role_hint", &role_name)?;

    let loadout = first_present([parsed.loadout.as_deref()])
        .map(FleetLoadout::from_name)
        .unwrap_or_default();
    let model = non_empty_trimmed(parsed.model.as_deref()).map(str::to_string);
    validate_agent_profile_model_hint(path, model.as_deref())?;

    let instructions = parsed
        .instructions
        .as_ref()
        .and_then(|instructions| non_empty_trimmed(instructions.text.as_deref()))
        .or_else(|| non_empty_trimmed(parsed.persona.as_deref()))
        .map(str::to_string);

    let description = non_empty_trimmed(parsed.description.as_deref()).map(str::to_string);
    let profile = FleetProfile {
        slot: FleetSlot::from_name(&role_name),
        role: FleetRole {
            name: role_name,
            description: description.clone(),
            instructions,
        },
        loadout,
        model,
        permissions: FleetProfilePermissions::default(),
        delegation: FleetDelegationHints::default(),
    };

    Ok(AgentProfile {
        id,
        display_name: non_empty_trimmed(parsed.display_name.as_deref()).map(str::to_string),
        description,
        profile,
        source: path.to_path_buf(),
        origin: ProfileOrigin::Workspace,
    })
}

fn reject_permission_expansion(
    path: &Path,
    tools: Option<&AgentProfileTools>,
    permissions: Option<&AgentProfilePermissionsToml>,
) -> Result<()> {
    if let Some(posture) = tools
        .and_then(|tools| tools.posture.as_deref())
        .and_then(trimmed_non_empty)
    {
        match posture {
            "read-only" | "readonly" | "read_only" => {}
            other => bail!(
                "agent profile {} tools.posture={other:?} would widen permissions; use FleetProfile policy for grants",
                path.display()
            ),
        }
    }

    if let Some(permissions) = permissions {
        if permissions.allow_shell.unwrap_or(false) {
            bail!(
                "agent profile {} may not request allow_shell=true",
                path.display()
            );
        }
        if permissions.trust.unwrap_or(false) {
            bail!(
                "agent profile {} may not request trust=true",
                path.display()
            );
        }
        if permissions.approval_required == Some(false) {
            bail!(
                "agent profile {} may not disable approval_required",
                path.display()
            );
        }
    }
    Ok(())
}

fn validate_agent_profile_token(path: &Path, field: &str, value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("agent profile {} {field} cannot be empty", path.display());
    }
    if trimmed != value || !trimmed.chars().all(is_agent_profile_token_char) {
        bail!(
            "agent profile {} {field} must be a simple token",
            path.display()
        );
    }
    Ok(())
}

fn validate_agent_profile_model_hint(path: &Path, value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if !is_model_hint(value) {
        bail!(
            "agent profile {} model must be a visible model id without whitespace or secrets",
            path.display()
        );
    }
    Ok(())
}

fn is_agent_profile_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

fn is_model_hint(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed == value
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_graphic() && !matches!(ch, '=' | '\'' | '"'))
}

fn first_present<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    values.into_iter().flatten().find_map(trimmed_non_empty)
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.and_then(trimmed_non_empty)
}

fn trimmed_non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Outcome of parsing untrusted model output into a fleet profile draft.
/// Mirrors `UntrustedDraftParse` from the constitution pipeline: the reply is
/// data, never trusted, and any failure is a reason string for the status
/// line — drafting failures degrade to the manual authoring flow.
#[derive(Debug)]
pub enum UntrustedProfileParse {
    Drafted(Box<FleetProfileDraft>),
    Empty,
    Invalid(String),
}

/// A model-drafted fleet agent profile that has passed the untrusted gate:
/// balanced-JSON extraction, serde parse with `deny_unknown_fields` (so
/// provider/base_url/api_key/permissions/tools cannot ride along), the same
/// escalation rejections the profile loader applies, token and model-hint
/// validation, prose bounds, and control-character stripping. The persisted
/// TOML is rendered deterministically from this struct — model bytes are
/// never written to disk verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetProfileDraft {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub role_hint: String,
    pub model_class_hint: Option<String>,
    pub model: Option<String>,
    pub instructions: Option<String>,
}

/// Bounds for model-drafted profile prose. Same philosophy as the
/// constitution bounds: roomy enough for a real profile, hard enough that a
/// misbehaving provider cannot bloat the store.
pub const MAX_PROFILE_DESCRIPTION_LEN: usize = 1000;
pub const MAX_PROFILE_INSTRUCTIONS_LEN: usize = 4000;
const MAX_PROFILE_DISPLAY_NAME_LEN: usize = 80;
const MAX_PROFILE_TOKEN_LEN: usize = 64;

/// The JSON shape the drafting prompt asks for. `deny_unknown_fields` is the
/// first escalation gate: a draft that tries to smuggle `permissions`,
/// `tools`, `provider`, `base_url`, or `api_key` fails the parse outright
/// instead of being silently stripped.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FleetProfileDraftJson {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    role_hint: Option<String>,
    #[serde(default)]
    model_class_hint: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
}

impl FleetProfileDraft {
    /// Parse untrusted model output. Any structural problem is `Invalid`
    /// with a short reason; a parse that carries no usable content is
    /// `Empty`.
    #[must_use]
    pub fn from_untrusted_json(raw: &str) -> UntrustedProfileParse {
        let Some(json) = extract_first_json_object(raw) else {
            return UntrustedProfileParse::Invalid("no JSON object found".to_string());
        };
        let parsed: FleetProfileDraftJson = match serde_json::from_str(json) {
            Ok(parsed) => parsed,
            Err(err) => return UntrustedProfileParse::Invalid(err.to_string()),
        };

        let role_hint = match parsed
            .role_hint
            .as_deref()
            .and_then(trimmed_non_empty)
            .map(sanitize_profile_token)
        {
            Some(token) if !token.is_empty() => token,
            _ => return UntrustedProfileParse::Invalid("role_hint missing".to_string()),
        };
        let id = parsed
            .id
            .as_deref()
            .and_then(trimmed_non_empty)
            .map(sanitize_profile_token)
            .filter(|token| !token.is_empty())
            .unwrap_or_else(|| role_hint.clone());
        let model_class_hint = parsed
            .model_class_hint
            .as_deref()
            .and_then(trimmed_non_empty)
            .map(sanitize_profile_token)
            .filter(|token| !token.is_empty());
        let model = parsed
            .model
            .as_deref()
            .and_then(trimmed_non_empty)
            .map(str::to_string);
        if let Some(ref model) = model
            && !is_model_hint(model)
        {
            return UntrustedProfileParse::Invalid(
                "model must be a visible model id without whitespace or secrets".to_string(),
            );
        }
        let display_name = parsed
            .display_name
            .as_deref()
            .map(|text| sanitize_profile_prose(text, MAX_PROFILE_DISPLAY_NAME_LEN))
            .and_then(|text| trimmed_non_empty(&text).map(str::to_string));
        let description = parsed
            .description
            .as_deref()
            .map(|text| sanitize_profile_prose(text, MAX_PROFILE_DESCRIPTION_LEN))
            .and_then(|text| trimmed_non_empty(&text).map(str::to_string));
        let instructions = parsed
            .instructions
            .as_deref()
            .map(|text| sanitize_profile_prose(text, MAX_PROFILE_INSTRUCTIONS_LEN))
            .and_then(|text| trimmed_non_empty(&text).map(str::to_string));

        let draft = FleetProfileDraft {
            id,
            display_name,
            description,
            role_hint,
            model_class_hint,
            model,
            instructions,
        };
        if draft.description.is_none() && draft.instructions.is_none() {
            return UntrustedProfileParse::Empty;
        }
        UntrustedProfileParse::Drafted(Box::new(draft))
    }

    /// Deterministic TOML rendering — the exact bytes the ratify keypress
    /// would persist. Loading this back through the profile loader must
    /// succeed with the default (floor) permissions.
    #[must_use]
    pub fn render_toml(&self) -> String {
        let mut root = toml::value::Table::new();
        root.insert("id".to_string(), toml::Value::String(self.id.clone()));
        if let Some(ref display_name) = self.display_name {
            root.insert(
                "display_name".to_string(),
                toml::Value::String(display_name.clone()),
            );
        }
        if let Some(ref description) = self.description {
            root.insert(
                "description".to_string(),
                toml::Value::String(description.clone()),
            );
        }
        root.insert(
            "role_hint".to_string(),
            toml::Value::String(self.role_hint.clone()),
        );
        if let Some(ref hint) = self.model_class_hint {
            root.insert("loadout".to_string(), toml::Value::String(hint.clone()));
        }
        if let Some(ref model) = self.model {
            root.insert("model".to_string(), toml::Value::String(model.clone()));
        }
        if let Some(ref instructions) = self.instructions {
            let mut table = toml::value::Table::new();
            table.insert(
                "text".to_string(),
                toml::Value::String(instructions.clone()),
            );
            root.insert("instructions".to_string(), toml::Value::Table(table));
        }
        toml::to_string_pretty(&toml::Value::Table(root))
            .unwrap_or_else(|_| String::from("# failed to render profile"))
    }

    /// File name (stem + `.toml`) for this draft, always derived from the
    /// sanitized id — never a model-chosen free-form path.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.toml", self.id)
    }
}

/// Keep only the loader's token alphabet, lowercased, bounded.
fn sanitize_profile_token(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| ch.to_ascii_lowercase())
        .filter(|ch| is_agent_profile_token_char(*ch))
        .take(MAX_PROFILE_TOKEN_LEN)
        .collect()
}

/// Strip control characters (newline/tab survive) and bound length by chars.
fn sanitize_profile_prose(text: &str, max_len: usize) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .take(max_len)
        .collect()
}

/// Extract the first balanced `{...}` object from untrusted output, so fenced
/// or prose-wrapped JSON still parses. Mirrors the constitution pipeline's
/// extractor (which is private to codewhale-config).
fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in raw[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=start + offset]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn draft_gate_rejects_unknown_and_escalation_fields() {
        for raw in [
            r#"{"id":"x","role_hint":"reviewer","description":"d","permissions":{"allow_shell":true}}"#,
            r#"{"id":"x","role_hint":"reviewer","description":"d","tools":{"posture":"full"}}"#,
            r#"{"id":"x","role_hint":"reviewer","description":"d","provider":"openai"}"#,
            r#"{"id":"x","role_hint":"reviewer","description":"d","api_key":"sk-nope"}"#,
        ] {
            assert!(
                matches!(
                    FleetProfileDraft::from_untrusted_json(raw),
                    UntrustedProfileParse::Invalid(_)
                ),
                "{raw} must be rejected, not stripped"
            );
        }
    }

    #[test]
    fn draft_gate_bounds_and_sanitizes() {
        let huge = "x".repeat(MAX_PROFILE_INSTRUCTIONS_LEN + 500);
        // \u0007 (BEL) inside the description must be stripped by the
        // prose sanitizer; the oversized instructions must be bounded.
        let raw = format!(
            "{{\"id\":\"  Weird ID!!  \",\"role_hint\":\"Code Reviewer\",\"description\":\"has\\u0007control\",\"instructions\":\"{huge}\"}}"
        );
        let UntrustedProfileParse::Drafted(draft) = FleetProfileDraft::from_untrusted_json(&raw)
        else {
            panic!("draft should parse");
        };
        assert_eq!(draft.id, "weirdid");
        assert_eq!(draft.role_hint, "codereviewer");
        assert_eq!(draft.description.as_deref(), Some("hascontrol"));
        assert_eq!(
            draft.instructions.as_deref().unwrap().chars().count(),
            MAX_PROFILE_INSTRUCTIONS_LEN
        );
    }

    #[test]
    fn draft_gate_rejects_secret_shaped_model_and_missing_role() {
        assert!(matches!(
            FleetProfileDraft::from_untrusted_json(
                r#"{"id":"x","role_hint":"reviewer","description":"d","model":"has secret ="}"#
            ),
            UntrustedProfileParse::Invalid(_)
        ));
        assert!(matches!(
            FleetProfileDraft::from_untrusted_json(r#"{"id":"x","description":"d"}"#),
            UntrustedProfileParse::Invalid(_)
        ));
        assert!(matches!(
            FleetProfileDraft::from_untrusted_json(r#"{"id":"x","role_hint":"reviewer"}"#),
            UntrustedProfileParse::Empty
        ));
    }

    #[test]
    fn draft_gate_accepts_fenced_output() {
        let raw = "Here you go:\n```json\n{\"id\":\"reviewer\",\"role_hint\":\"reviewer\",\"description\":\"Reviews diffs.\"}\n```";
        assert!(matches!(
            FleetProfileDraft::from_untrusted_json(raw),
            UntrustedProfileParse::Drafted(_)
        ));
    }

    #[test]
    fn rendered_draft_round_trips_through_the_loader_with_floor_permissions() {
        let UntrustedProfileParse::Drafted(draft) = FleetProfileDraft::from_untrusted_json(
            r#"{"id":"reviewer","display_name":"Reviewer","description":"Reviews diffs for correctness.","role_hint":"reviewer","model_class_hint":"cheap","model":"glm-5.2","instructions":"Read the diff.\nReport findings, then stop."}"#,
        ) else {
            panic!("draft should parse");
        };

        let dir = TempDir::new().unwrap();
        let path = write_profile(dir.path(), &draft.file_name(), &draft.render_toml());
        let profiles = load_agent_profiles_from_dir(dir.path()).expect("rendered TOML loads");
        assert_eq!(profiles.len(), 1);
        let loaded = &profiles[0];
        assert_eq!(loaded.id, "reviewer");
        assert_eq!(loaded.display_name.as_deref(), Some("Reviewer"));
        assert_eq!(loaded.profile.model.as_deref(), Some("glm-5.2"));
        assert_eq!(
            loaded.profile.role.instructions.as_deref(),
            Some("Read the diff.\nReport findings, then stop.")
        );
        // The loader always installs the permission floor, no matter what.
        assert_eq!(
            loaded.profile.permissions,
            FleetProfilePermissions::default()
        );
        assert_eq!(path, loaded.source);
    }

    fn write_profile(dir: &Path, filename: &str, contents: &str) -> PathBuf {
        let path = dir.join(filename);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn fleet_profile_round_trips_through_serde_with_safe_defaults() {
        let profile = FleetProfile::default();

        let serialized = toml::to_string(&profile).expect("profile serializes");
        let round_tripped: FleetProfile =
            toml::from_str(&serialized).expect("profile deserializes");

        assert_eq!(round_tripped, profile);
        assert_eq!(round_tripped.role.name, "general");
        assert_eq!(round_tripped.loadout, FleetLoadout::Inherit);
        assert!(!round_tripped.permissions.allow_shell);
        assert!(!round_tripped.permissions.trust);
        assert!(round_tripped.permissions.approval_required);
        assert_eq!(round_tripped.delegation.max_spawn_depth, None);
        assert_eq!(round_tripped.delegation.max_concurrency, None);
    }

    #[test]
    fn fleet_profile_explicit_toml_parses_role_loadout_permissions() {
        let profile: FleetProfile = toml::from_str(
            r#"
slot = "reviewer"
loadout = "deep-reasoning"

[role]
name = "verifier"
instructions = "Review the patch and produce verification evidence."

[permissions]
allow_shell = true
trust = true
approval_required = false

[delegation]
max_spawn_depth = 1
concurrency = 2
"#,
        )
        .expect("explicit fleet profile parses");

        assert_eq!(profile.slot, FleetSlot::Reviewer);
        assert_eq!(profile.role.name, "verifier");
        assert_eq!(
            profile.role.instructions.as_deref(),
            Some("Review the patch and produce verification evidence.")
        );
        assert_eq!(
            profile.loadout,
            FleetLoadout::Custom("deep-reasoning".to_string())
        );
        assert!(profile.permissions.allow_shell);
        assert!(profile.permissions.trust);
        assert!(!profile.permissions.approval_required);
        assert_eq!(profile.delegation.max_spawn_depth, Some(1));
        assert_eq!(profile.delegation.max_concurrency, Some(2));
    }

    #[test]
    fn fleet_profile_accepts_compact_role_string() {
        let profile: FleetProfile = toml::from_str(
            r#"
role = "scout"
loadout = "fast"
model = "deepseek-v4-flash"
"#,
        )
        .expect("compact fleet profile parses");

        assert_eq!(profile.role.name, "scout");
        assert_eq!(profile.loadout, FleetLoadout::Fast);
        assert_eq!(profile.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(profile.permissions, FleetProfilePermissions::default());
    }

    #[test]
    fn agent_profile_loader_returns_empty_for_missing_workspace_dir() {
        let tmp = TempDir::new().unwrap();

        let profiles = load_workspace_agent_profiles(tmp.path()).unwrap();

        assert!(profiles.is_empty());
    }

    #[test]
    fn agent_profile_loader_normalizes_project_agent_toml() {
        let tmp = TempDir::new().unwrap();
        let agents_dir = tmp.path().join(WORKSPACE_AGENT_PROFILE_DIR);
        std::fs::create_dir_all(&agents_dir).unwrap();
        let source = write_profile(
            &agents_dir,
            "reviewer.toml",
            r#"
name = "adversarial_reviewer"
display_name = "Adversarial Reviewer"
description = "Skeptical read-only review posture"
role_hint = "reviewer"
loadout = "balanced"
model = "deepseek-v4-pro"

[instructions]
text = "Focus on regressions, missing tests, and fragile assumptions."

[tools]
posture = "read-only"
"#,
        );

        let profiles = load_workspace_agent_profiles(tmp.path()).unwrap();

        assert_eq!(profiles.len(), 1);
        let profile = &profiles[0];
        assert_eq!(profile.id, "adversarial_reviewer");
        assert_eq!(
            profile.display_name.as_deref(),
            Some("Adversarial Reviewer")
        );
        assert_eq!(
            profile.description.as_deref(),
            Some("Skeptical read-only review posture")
        );
        assert_eq!(profile.profile.slot, FleetSlot::Reviewer);
        assert_eq!(profile.profile.role.name, "reviewer");
        assert_eq!(
            profile.profile.role.instructions.as_deref(),
            Some("Focus on regressions, missing tests, and fragile assumptions.")
        );
        assert_eq!(
            profile.profile.loadout,
            FleetLoadout::Custom("balanced".to_string())
        );
        assert_eq!(profile.profile.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(
            profile.profile.permissions,
            FleetProfilePermissions::default()
        );
        assert_eq!(profile.source, source);
    }

    #[test]
    fn agent_profile_loader_rejects_retired_model_policy_aliases() {
        for (field, value) in [("model_class_hint", "balanced"), ("route_tier", "fast")] {
            let tmp = TempDir::new().unwrap();
            write_profile(
                tmp.path(),
                "reviewer.toml",
                &format!(
                    r#"
name = "reviewer"
role_hint = "reviewer"
{field} = "{value}"
"#
                ),
            );

            let err = load_agent_profiles_from_dir(tmp.path())
                .unwrap_err()
                .to_string();

            assert!(
                err.contains(field) || err.contains("unknown field"),
                "unexpected error for {field}: {err}"
            );
        }
    }

    #[test]
    fn agent_profile_loader_rejects_hidden_provider_policy_fields() {
        let tmp = TempDir::new().unwrap();
        write_profile(
            tmp.path(),
            "reviewer.toml",
            r#"
name = "reviewer"
provider = "openrouter"
model = "deepseek/deepseek-v4-pro"
"#,
        );

        let err = load_agent_profiles_from_dir(tmp.path())
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("provider") || err.contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn agent_profile_loader_rejects_permission_expansion() {
        let tmp = TempDir::new().unwrap();
        write_profile(
            tmp.path(),
            "builder.toml",
            r#"
name = "builder"

[tools]
posture = "read-write"
"#,
        );

        let err = load_agent_profiles_from_dir(tmp.path())
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("would widen permissions"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn agent_profile_loader_rejects_secret_like_model_hint() {
        let tmp = TempDir::new().unwrap();
        write_profile(
            tmp.path(),
            "reviewer.toml",
            r#"
name = "reviewer"
model = "deepseek-v4-pro api_key=secret"
"#,
        );

        let err = load_agent_profiles_from_dir(tmp.path())
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("model must be a visible model id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn agent_profile_loader_rejects_duplicate_ids() {
        let tmp = TempDir::new().unwrap();
        write_profile(tmp.path(), "a.toml", "name = \"reviewer\"\n");
        write_profile(tmp.path(), "b.toml", "id = \"reviewer\"\n");

        let err = load_agent_profiles_from_dir(tmp.path())
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("duplicate agent profile id reviewer"),
            "unexpected error: {err}"
        );
    }
}
