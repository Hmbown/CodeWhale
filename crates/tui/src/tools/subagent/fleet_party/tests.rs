//! Fleet-party spawn-surface tests: roster advertising, profile application
//! precedence, ranked-model degradation, and unknown-id errors.

use super::*;
use crate::tools::registry::AgentToolSurfaceOptions;
use crate::tools::spec::{ToolContext, ToolSpec};
use crate::tools::subagent::{
    AgentTool, DEFAULT_MAX_SPAWN_DEPTH, SharedSubAgentManager, SubAgentManager, SubAgentRuntime,
    SubAgentType, parse_spawn_request,
};
use crate::worker_profile::{ShellPolicy, WorkerRuntimeProfile};

use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

fn write_profile(workspace: &std::path::Path, file: &str, body: &str) {
    let dir = workspace.join(".codewhale/agents");
    std::fs::create_dir_all(&dir).expect("create party dir");
    std::fs::write(dir.join(file), body).expect("write profile");
}

fn reviewer_profile_toml() -> &'static str {
    r#"
id = "reviewer"
base_role = "reviewer"
model_class_hint = "heavy"
model = "deepseek-v4-pro"

[instructions]
text = "Focus on security-sensitive diffs first."
"#
}

fn scout_profile_toml() -> &'static str {
    r#"
id = "scout"
base_role = "scout"
model_class_hint = "fast"
"#
}

fn party_of(workspace: &std::path::Path) -> Vec<AgentProfile> {
    load_fleet_party(workspace)
}

fn stub_runtime(workspace: &std::path::Path) -> SubAgentRuntime {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = crate::config::Config {
        api_key: Some("test-key".to_string()),
        ..crate::config::Config::default()
    };
    let client = crate::client::DeepSeekClient::new(&config).expect("stub client");
    let manager: SharedSubAgentManager = Arc::new(RwLock::new(SubAgentManager::new(
        workspace.to_path_buf(),
        4,
    )));
    SubAgentRuntime {
        client,
        model: "deepseek-v4-flash".to_string(),
        auto_model: false,
        reasoning_effort: None,
        reasoning_effort_auto: false,
        role_models: std::collections::HashMap::new(),
        context: ToolContext::new(workspace.to_path_buf()),
        allow_shell: true,
        agent_tool_surface_options: AgentToolSurfaceOptions::new(ShellPolicy::Full),
        worker_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
        event_tx: None,
        manager,
        spawn_depth: 0,
        parent_agent_id: None,
        max_spawn_depth: DEFAULT_MAX_SPAWN_DEPTH,
        cancel_token: CancellationToken::new(),
        mailbox: None,
        parent_completion_tx: None,
        fork_context: None,
        mcp_pool: None,
        step_api_timeout: Duration::from_secs(30),
        tool_timeout: Duration::from_secs(30),
        speech_output_dir: None,
        todos: crate::tools::todo::new_shared_todo_list(),
        approval_broker: None,
    }
}

#[test]
fn subagent_fleet_roster_advertised_only_when_party_exists() {
    // Party present: the agent tool description carries the compact roster
    // and the schema exposes the `profile` field.
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    write_profile(tmp.path(), "scout.toml", scout_profile_toml());
    let runtime = stub_runtime(tmp.path());
    let tool = AgentTool::new(runtime.manager.clone(), runtime);
    let description = tool.description();
    assert!(
        description.contains("Fleet party (use profile:\"<id>\")"),
        "roster header missing: {description}"
    );
    assert!(
        description.contains("reviewer — role reviewer, class heavy, model deepseek-v4-pro"),
        "reviewer roster entry missing: {description}"
    );
    assert!(
        description.contains("scout — role scout, class fast"),
        "scout roster entry missing: {description}"
    );
    assert!(
        tool.input_schema()["properties"].get("profile").is_some(),
        "schema must advertise the profile field"
    );

    // Zero profiles: behavior unchanged — byte-identical base description.
    let empty = tempdir().expect("tempdir");
    let runtime = stub_runtime(empty.path());
    let tool = AgentTool::new(runtime.manager.clone(), runtime);
    assert!(
        !tool.description().contains("Fleet party"),
        "empty party must not advertise a roster"
    );
    assert_eq!(
        tool.description(),
        super::super::AGENT_TOOL_BASE_DESCRIPTION
    );
}

/// Build a long-lived registry the way `run_subagent` does (one registry for
/// the child's whole lifetime) so the tests below can drive the roster's
/// staleness/refresh cycle at its real seam.
fn agent_registry(workspace: &std::path::Path) -> crate::tools::ToolRegistry {
    let runtime = stub_runtime(workspace);
    crate::tools::registry::ToolRegistryBuilder::new()
        .with_subagent_tools(runtime.manager.clone(), runtime)
        .build(ToolContext::new(workspace.to_path_buf()))
}

fn agent_description(registry: &crate::tools::ToolRegistry) -> String {
    registry
        .to_api_tools()
        .into_iter()
        .find(|tool| tool.name == "agent")
        .expect("agent tool in catalog")
        .description
}

/// Live repro: an agent created `.codewhale/agents/*.toml` mid-session and
/// the `agent` tool description never showed the roster, because the roster
/// is composed once at `AgentTool::new` and both the tool instance and the
/// registry's serialized-catalog memo outlive that snapshot in a long-lived
/// registry (the sub-agent step loop). `refresh_model_surface` is the
/// fingerprint-gated fix; the parent path refreshes naturally because its
/// registry is rebuilt every user turn.
#[test]
fn subagent_fleet_roster_appears_after_midsession_profile_creation() {
    let tmp = tempdir().expect("tempdir");
    let mut registry = agent_registry(tmp.path());
    assert_eq!(
        agent_description(&registry),
        super::super::AGENT_TOOL_BASE_DESCRIPTION,
        "no party at construction ⇒ base description"
    );

    // Mid-session party creation. Without a refresh the long-lived registry
    // keeps serving the stale birth snapshot — the original defect.
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    assert_eq!(
        agent_description(&registry),
        super::super::AGENT_TOOL_BASE_DESCRIPTION,
        "stale without refresh: description pinned at construction"
    );

    assert!(
        registry.refresh_model_surface(),
        "party changed ⇒ refresh reports a catalog change"
    );
    let description = agent_description(&registry);
    assert!(
        description.contains("Fleet party (use profile:\"<id>\")")
            && description.contains("reviewer — role reviewer, class heavy, model deepseek-v4-pro"),
        "refreshed roster advertises the new profile: {description}"
    );

    // Turn boundary (parent path): a freshly constructed AgentTool — what
    // `handle_send_message` builds each user turn — sees the party too.
    let runtime = stub_runtime(tmp.path());
    let tool = AgentTool::new(runtime.manager.clone(), runtime);
    assert_eq!(tool.description(), description);
}

#[test]
fn subagent_fleet_roster_updates_after_profile_edit_and_removal() {
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    let mut registry = agent_registry(tmp.path());
    assert!(agent_description(&registry).contains("model deepseek-v4-pro"));

    // Edit: the profile's pinned model changes ⇒ one refresh, new roster.
    write_profile(
        tmp.path(),
        "reviewer.toml",
        r#"
id = "reviewer"
base_role = "reviewer"
model_class_hint = "heavy"
model = "deepseek-v4-flash"
"#,
    );
    assert!(registry.refresh_model_surface(), "edit must be detected");
    let description = agent_description(&registry);
    assert!(
        description.contains("model deepseek-v4-flash") && !description.contains("deepseek-v4-pro"),
        "edited pin replaces the old roster entry: {description}"
    );

    // Removal: back to the byte-identical no-party base description.
    std::fs::remove_file(tmp.path().join(".codewhale/agents/reviewer.toml"))
        .expect("remove profile");
    assert!(registry.refresh_model_surface(), "removal must be detected");
    assert_eq!(
        agent_description(&registry),
        super::super::AGENT_TOOL_BASE_DESCRIPTION,
        "empty party degrades to the exact legacy description"
    );
}

#[test]
fn subagent_fleet_roster_unchanged_party_keeps_catalog_bytes_stable() {
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    write_profile(tmp.path(), "scout.toml", scout_profile_toml());
    let mut registry = agent_registry(tmp.path());

    // Unchanged party: refresh is a no-op and the serialized catalog stays
    // byte-identical across reads (prefix-cache stability).
    let before = serde_json::to_value(registry.to_api_tools()).expect("serialize catalog");
    assert!(
        !registry.refresh_model_surface(),
        "unchanged party must not report a catalog change"
    );
    let after = serde_json::to_value(registry.to_api_tools()).expect("serialize catalog");
    assert_eq!(before, after, "no-op refresh keeps catalog bytes pinned");

    // Turn-boundary rebuild (parent path) with an unchanged party is also
    // byte-identical: the roster is deterministic over the sorted party.
    let runtime = stub_runtime(tmp.path());
    let first = AgentTool::new(runtime.manager.clone(), runtime);
    let runtime = stub_runtime(tmp.path());
    let second = AgentTool::new(runtime.manager.clone(), runtime);
    assert_eq!(first.description(), second.description());
    assert_eq!(first.description(), agent_description(&registry));
}

#[test]
fn subagent_no_party_description_stays_byte_identical_across_rebuilds() {
    let tmp = tempdir().expect("tempdir");
    let mut registry = agent_registry(tmp.path());
    assert!(
        !registry.refresh_model_surface(),
        "no party ⇒ nothing to refresh"
    );
    assert_eq!(
        agent_description(&registry),
        super::super::AGENT_TOOL_BASE_DESCRIPTION
    );
    let runtime = stub_runtime(tmp.path());
    let rebuilt = AgentTool::new(runtime.manager.clone(), runtime);
    assert_eq!(
        rebuilt.description(),
        super::super::AGENT_TOOL_BASE_DESCRIPTION,
        "no-party rebuild stays byte-identical to the legacy description"
    );
}

#[test]
fn subagent_fleet_roster_is_bounded_with_more_marker() {
    let tmp = tempdir().expect("tempdir");
    for index in 0..10 {
        write_profile(
            tmp.path(),
            &format!("member{index:02}.toml"),
            &format!("id = \"member{index:02}\"\nbase_role = \"scout\"\n"),
        );
    }
    let roster = fleet_party_roster(&party_of(tmp.path())).expect("roster");
    assert!(roster.contains("member00"));
    assert!(roster.contains("member07"));
    assert!(
        !roster.contains("member08"),
        "ninth member must be truncated: {roster}"
    );
    assert!(roster.ends_with("; +2 more"), "bounded marker: {roster}");
}

#[test]
fn subagent_fleet_roster_shows_all_members_at_exactly_the_bound() {
    // Exactly 8 members (the roster ceiling): every member is listed and no
    // "+N more" marker appears. At 9, exactly one member overflows.
    let tmp = tempdir().expect("tempdir");
    for index in 0..8 {
        write_profile(
            tmp.path(),
            &format!("member{index:02}.toml"),
            &format!("id = \"member{index:02}\"\nbase_role = \"scout\"\n"),
        );
    }
    let roster = fleet_party_roster(&party_of(tmp.path())).expect("roster");
    assert!(
        roster.contains("member00") && roster.contains("member07"),
        "{roster}"
    );
    assert!(!roster.contains("more"), "no marker at the bound: {roster}");

    write_profile(
        tmp.path(),
        "member08.toml",
        "id = \"member08\"\nbase_role = \"scout\"\n",
    );
    let roster = fleet_party_roster(&party_of(tmp.path())).expect("roster");
    assert!(!roster.contains("member08"), "{roster}");
    assert!(roster.ends_with("; +1 more"), "{roster}");
}

#[test]
fn subagent_fast_class_profile_without_pins_routes_faster() {
    // A `fast` profile with no pinned models must still route to the faster
    // sibling — the same route the fleet worker runtime derives from
    // `FleetLoadout::Fast` — instead of silently inheriting the parent model.
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "scout.toml", scout_profile_toml());
    let party = party_of(tmp.path());

    let mut request =
        parse_spawn_request(&json!({ "prompt": "map the crate", "profile": "scout" }))
            .expect("parse");
    let application = apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies")
    .expect("application present");

    assert!(application.notice.is_none(), "no pins, no degradation");
    assert_eq!(request.model, None, "class route decides, not a pin");
    assert_eq!(request.model_strength, SubAgentModelStrength::Faster);

    // An explicit model_strength on the call still wins over the class.
    let mut request = parse_spawn_request(
        &json!({ "prompt": "map", "profile": "scout", "model_strength": "same" }),
    )
    .expect("parse");
    apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies");
    assert_eq!(request.model_strength, SubAgentModelStrength::Same);
}

#[test]
fn subagent_heavy_class_profile_without_pins_inherits_route() {
    // Heavy/omni classes have no faster-sibling analogue on the spawn
    // surface: with no pins the request stays on the inherited route.
    let tmp = tempdir().expect("tempdir");
    write_profile(
        tmp.path(),
        "architect.toml",
        r#"
id = "architect"
base_role = "planner"
model_class_hint = "heavy"
"#,
    );
    let party = party_of(tmp.path());

    let mut request =
        parse_spawn_request(&json!({ "prompt": "plan the refactor", "profile": "architect" }))
            .expect("parse");
    apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies");

    assert_eq!(request.model, None);
    assert_eq!(request.model_strength, SubAgentModelStrength::Same);
    assert_eq!(request.agent_type, SubAgentType::Plan, "planner → plan");
}

#[test]
fn subagent_custom_class_token_normalizing_to_omni_is_a_real_class() {
    // A wizard custom token like "multi-modal" normalizes to the Omni
    // loadout: the roster advertises the real class, not a custom token.
    let tmp = tempdir().expect("tempdir");
    write_profile(
        tmp.path(),
        "vision.toml",
        r#"
id = "vision"
base_role = "reviewer"
model_class_hint = "multi-modal"
"#,
    );
    let party = party_of(tmp.path());
    assert_eq!(
        party[0].profile.loadout,
        codewhale_config::FleetLoadout::Omni
    );
    let roster = fleet_party_roster(&party).expect("roster");
    assert!(
        roster.contains("vision — role reviewer, class omni"),
        "{roster}"
    );
}

/// JOURNEY (ratify path): a wizard model-draft — the exact JSON shape the
/// drafting gate accepts — renders to TOML, loads through the profile
/// loader, is advertised on the roster, resolves via `profile:"<id>"`, and
/// routes its ranked pin on a DeepSeek provider.
#[test]
fn subagent_journey_ratified_wizard_draft_spawns_and_routes_on_deepseek() {
    use crate::fleet::profile::{FleetProfileDraft, UntrustedProfileParse};

    let UntrustedProfileParse::Drafted(draft) = FleetProfileDraft::from_untrusted_json(
        r#"{"id":"release-captain","display_name":"Release Captain","description":"Owns release verification.","role_hint":"verifier","model_class_hint":"heavy","model":"deepseek-v4-pro","instructions":"Verify receipts. Report. Stop."}"#,
    ) else {
        panic!("draft should parse");
    };

    // (i) The ratify keypress persists exactly draft.render_toml() at
    // .codewhale/agents/<id>.toml — mirror that write.
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), &draft.file_name(), &draft.render_toml());
    let party = party_of(tmp.path());
    assert_eq!(party.len(), 1, "rendered TOML must load through the loader");
    assert_eq!(party[0].id, "release-captain");

    // (ii) The roster advertises the member with role/class/model.
    let roster = fleet_party_roster(&party).expect("roster");
    assert!(
        roster.contains("release-captain — role verifier, class heavy, model deepseek-v4-pro"),
        "{roster}"
    );

    // (iii) profile:"<id>" resolves on spawn; (iv) the pin routes on DeepSeek.
    let mut request = parse_spawn_request(
        &json!({ "prompt": "verify the release", "profile": "release-captain" }),
    )
    .expect("parse");
    let application = apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies")
    .expect("application present");
    assert_eq!(application.profile_id, "release-captain");
    assert!(application.notice.is_none(), "DeepSeek serves the pin");
    assert_eq!(request.agent_type, SubAgentType::Verifier, "verifier role");
    assert_eq!(request.assignment.role.as_deref(), Some("verifier"));
    assert_eq!(request.model.as_deref(), Some("deepseek-v4-pro"));
    assert!(request.prompt.contains("Verify receipts. Report. Stop."));
}

/// Live repro (ChatGPT Codex OAuth): a reviewer profile pinning
/// `models = ["deepseek-v4-pro", "glm-5.2"]` used to wire `deepseek-v4-pro`
/// straight to the Codex Responses backend, which answered
/// `Invalid request (400): The 'deepseek-v4-pro' model is not supported when
/// using Codex with a ChatGPT account.` Both pins are foreign to the codex
/// route, so the profile must degrade to the class route (model unset →
/// child inherits the parent codex model) with a surfaced notice — never a
/// known-bad wire call.
#[test]
fn subagent_profile_with_foreign_pins_degrades_on_codex_route() {
    let tmp = tempdir().expect("tempdir");
    write_profile(
        tmp.path(),
        "reviewer.toml",
        r#"
id = "reviewer"
base_role = "reviewer"
models = ["deepseek-v4-pro", "glm-5.2"]
"#,
    );
    let party = party_of(tmp.path());

    let mut request =
        parse_spawn_request(&json!({ "prompt": "review the diff", "profile": "reviewer" }))
            .expect("parse");
    let application =
        apply_fleet_profile_to_spawn(&mut request, &party, "gpt-5.5", ApiProvider::OpenaiCodex)
            .expect("profile applies")
            .expect("application present");

    assert_eq!(
        request.model, None,
        "no foreign pin may reach the codex wire; the class route decides"
    );
    let notice = application.notice.expect("degradation must be surfaced");
    assert!(notice.contains("deepseek-v4-pro"), "{notice}");
    assert!(notice.contains("glm-5.2"), "{notice}");

    // A codex-family pin on the same route still wins normally.
    write_profile(
        tmp.path(),
        "captain.toml",
        r#"
id = "captain"
base_role = "reviewer"
models = ["deepseek-v4-pro", "gpt-5.5"]
"#,
    );
    let party = party_of(tmp.path());
    let mut request =
        parse_spawn_request(&json!({ "prompt": "review", "profile": "captain" })).expect("parse");
    let application =
        apply_fleet_profile_to_spawn(&mut request, &party, "gpt-5.5", ApiProvider::OpenaiCodex)
            .expect("profile applies")
            .expect("application present");
    assert_eq!(request.model.as_deref(), Some("gpt-5.5"));
    assert!(application.notice.is_some(), "skipped pin is surfaced");
}

/// JOURNEY (manual path): a profile written to the authoring prompt's exact
/// schema (name/display_name/description/role_hint/model_class_hint/models/
/// [instructions].text/[tools].posture) loads, is advertised, and resolves
/// its ranked `models` list against the active provider on spawn.
#[test]
fn subagent_journey_manual_authoring_schema_profile_ranks_models_on_deepseek() {
    let tmp = tempdir().expect("tempdir");
    write_profile(
        tmp.path(),
        "builder.toml",
        r#"
name = "builder"
display_name = "Bounded Builder"
description = "Implements bounded changes."
role_hint = "builder"
model_class_hint = "heavy"
models = ["glm-5.2", "deepseek-v4-pro"]

[instructions]
text = "Implement only the assigned slice."

[tools]
posture = "read-only"
"#,
    );
    let party = party_of(tmp.path());
    assert_eq!(party.len(), 1);
    let roster = fleet_party_roster(&party).expect("roster");
    assert!(
        roster.contains("builder — role builder, class heavy, model glm-5.2"),
        "roster shows the top preference: {roster}"
    );

    let mut request =
        parse_spawn_request(&json!({ "prompt": "implement the fix", "profile": "builder" }))
            .expect("parse");
    let application = apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies")
    .expect("application present");

    // glm-5.2 is foreign to the official DeepSeek API: ranked fallback wins
    // and the degradation is surfaced, never a known-bad wire call.
    assert_eq!(request.model.as_deref(), Some("deepseek-v4-pro"));
    let notice = application.notice.expect("skipped pin carries a notice");
    assert!(notice.contains("glm-5.2"), "{notice}");
    assert_eq!(request.agent_type, SubAgentType::Implementer);
    assert!(
        request
            .prompt
            .contains("Implement only the assigned slice.")
    );
}

#[test]
fn subagent_fleet_profile_applies_role_model_and_instructions() {
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    write_profile(tmp.path(), "scout.toml", scout_profile_toml());
    let party = party_of(tmp.path());

    let mut request =
        parse_spawn_request(&json!({ "prompt": "review the diff", "profile": "reviewer" }))
            .expect("parse");
    let application = apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies")
    .expect("profile application present");

    assert_eq!(application.profile_id, "reviewer");
    assert!(application.notice.is_none(), "usable pin has no notice");
    assert_eq!(request.agent_type, SubAgentType::Review);
    assert_eq!(request.assignment.role.as_deref(), Some("reviewer"));
    assert_eq!(request.model.as_deref(), Some("deepseek-v4-pro"));
    assert!(
        request.prompt.contains("Fleet profile: reviewer"),
        "overlay names the profile: {}",
        request.prompt
    );
    assert!(
        request
            .prompt
            .contains("Focus on security-sensitive diffs first."),
        "instructions overlay applied: {}",
        request.prompt
    );
    assert!(
        request.prompt.starts_with("review the diff"),
        "objective stays first: {}",
        request.prompt
    );
}

#[test]
fn subagent_fleet_profile_defers_to_explicit_call_fields() {
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    let party = party_of(tmp.path());

    let mut request = parse_spawn_request(&json!({
        "prompt": "scout the module",
        "profile": "reviewer",
        "type": "explore",
        "model": "deepseek-v4-flash",
    }))
    .expect("parse");
    apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies");

    assert_eq!(
        request.agent_type,
        SubAgentType::Explore,
        "explicit type wins over the profile role"
    );
    assert_eq!(
        request.model.as_deref(),
        Some("deepseek-v4-flash"),
        "explicit model wins over the profile pin"
    );
    assert!(
        request.prompt.contains("Fleet profile: reviewer"),
        "instruction overlay still applies"
    );
}

#[test]
fn subagent_unknown_fleet_profile_errors_listing_available_ids() {
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    write_profile(tmp.path(), "scout.toml", scout_profile_toml());
    let party = party_of(tmp.path());

    let mut request =
        parse_spawn_request(&json!({ "prompt": "x", "profile": "ghost" })).expect("parse");
    let err = apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect_err("unknown id must error");
    let message = err.to_string();
    assert!(
        message.contains("Unknown fleet profile 'ghost'")
            && message.contains("reviewer")
            && message.contains("scout"),
        "error lists available ids: {message}"
    );

    let mut request =
        parse_spawn_request(&json!({ "prompt": "x", "profile": "ghost" })).expect("parse");
    let err = apply_fleet_profile_to_spawn(
        &mut request,
        &[],
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect_err("empty party must error");
    assert!(
        err.to_string().contains("/fleet setup"),
        "empty party points at /fleet setup: {err}"
    );
}

#[test]
fn subagent_degraded_fleet_pin_falls_back_to_class_route_with_notice() {
    let tmp = tempdir().expect("tempdir");
    write_profile(
        tmp.path(),
        "scout.toml",
        r#"
id = "scout"
base_role = "scout"
model_class_hint = "fast"
models = ["claude-opus-9000"]
"#,
    );
    let party = party_of(tmp.path());

    let mut request =
        parse_spawn_request(&json!({ "prompt": "map the crate", "profile": "scout" }))
            .expect("parse");
    let application = apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies")
    .expect("application present");

    let notice = application.notice.expect("degraded pin carries a notice");
    assert!(
        notice.contains("claude-opus-9000") && notice.contains("class route"),
        "notice explains the fallback: {notice}"
    );
    assert_eq!(
        request.model, None,
        "no known-bad id may reach the wire; the class route decides"
    );
    assert_eq!(
        request.model_strength,
        crate::tools::subagent::SubAgentModelStrength::Faster,
        "fast class degrades to the faster route"
    );
    assert_eq!(request.agent_type, SubAgentType::Explore, "scout → explore");
}

#[test]
fn subagent_fleet_profile_does_not_touch_permissions_or_tools() {
    // The fleet floor stays: applying a profile changes role/model/prompt
    // only — never the allowlist or an approval-related field.
    let tmp = tempdir().expect("tempdir");
    write_profile(tmp.path(), "reviewer.toml", reviewer_profile_toml());
    let party = party_of(tmp.path());

    let mut request = parse_spawn_request(&json!({
        "prompt": "review",
        "profile": "reviewer",
        "allowed_tools": ["read_file"],
    }))
    .expect("parse");
    apply_fleet_profile_to_spawn(
        &mut request,
        &party,
        "deepseek-v4-flash",
        ApiProvider::Deepseek,
    )
    .expect("profile applies");
    assert_eq!(
        request.allowed_tools.as_deref(),
        Some(&["read_file".to_string()][..]),
        "explicit tool narrowing survives profile application"
    );
}
