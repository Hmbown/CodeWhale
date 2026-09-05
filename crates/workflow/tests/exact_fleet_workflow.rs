//! End-to-end vertical for an exact named Fleet at the crate boundary:
//! parse → attach a reusable Reasoning Router → freeze routes → router decision
//! → reasoning resolution → immutable Workflow snapshot.
//!
//! No live provider calls: the router's response is a fixture string.

use codewhale_workflow::{
    CapturedReasoningRouter, CredentialReadiness, EffectiveReasoning, EffectiveReasoningSource,
    EndpointIdentity, FleetDocument, FleetSearchRoot, FleetSnapshot, NamedFleetError,
    PreflightedRoute, ProviderEffectiveReasoning, QualifiedFleetId, REASONING_ROUTER_DIR,
    REASONING_ROUTER_SERVICE_KIND, ReasoningCapability, ReasoningRouterError,
    ReasoningRouterProfile, ReasoningTier, RequestedReasoning, RouterAvailability,
    RouterCallReasoning, RouterIdentity, bounded_routing_payload, parse_router_decision,
    resolve_exact_member_reasoning, router_call_plan, router_system_prompt, router_user_message,
};

const GLM_FLEET: &str = r#"
name = "glm-pair"
description = "GLM workers with a shared GPT-5.6 Luna reasoning router"
schema = "exact"
schema_revision = 1
reasoning_router = "luna-low"

[[members]]
id = "implementer"
role = "builder"
provider = "zai"
model = "glm-5"
reasoning = "auto"
permissions = "read_write"

[[members]]
id = "auditor"
role = "reviewer"
provider = "zai"
model = "glm-5"
reasoning = "high"
permissions = "read_only"
"#;

/// The user's example: GPT-5.6 Luna, called at `low`.
const LUNA: &str = r#"
name = "luna-low"
schema = "reasoning_router"
schema_revision = 1
provider = "openai"
model = "gpt-5.6-luna"
call_reasoning = "low"
"#;

fn fleet_id(name: &str) -> QualifiedFleetId {
    QualifiedFleetId {
        name: name.to_string(),
        origin: "workspace".to_string(),
    }
}

fn luna() -> CapturedReasoningRouter {
    let profile = ReasoningRouterProfile::parse(LUNA).expect("router profile");
    CapturedReasoningRouter::from_profile(&profile, "workspace")
}

fn route(member: &str, provider: &str, model: &str) -> PreflightedRoute {
    PreflightedRoute {
        member_id: member.to_string(),
        provider_id: provider.to_string(),
        provider_config_id: None,
        provider_kind: provider.to_string(),
        declared_model: model.to_string(),
        wire_model: model.to_string(),
        endpoint: EndpointIdentity::from_base_url("https://api.example.test/v1"),
        credential: CredentialReadiness::Configured,
        capability: ReasoningCapability::tiered(),
    }
}

/// A workspace holding one Fleet and one Router profile, plus its search roots.
fn workspace_with(fleet: &str, router: Option<&str>) -> (tempfile::TempDir, Vec<FleetSearchRoot>) {
    let tmp = tempfile::tempdir().expect("tmp");
    std::fs::create_dir_all(tmp.path().join("fleets")).expect("fleets dir");
    std::fs::write(tmp.path().join("fleets/glm-pair.toml"), fleet).expect("fleet");
    if let Some(router) = router {
        std::fs::create_dir_all(tmp.path().join(REASONING_ROUTER_DIR)).expect("routers dir");
        std::fs::write(
            tmp.path().join(REASONING_ROUTER_DIR).join("luna-low.toml"),
            router,
        )
        .expect("router");
    }
    let roots = vec![FleetSearchRoot::new("workspace", tmp.path())];
    (tmp, roots)
}

#[test]
fn a_fleet_and_its_referenced_router_resolve_reasoning_without_moving_the_route() {
    let (_tmp, roots) = workspace_with(GLM_FLEET, Some(LUNA));
    let (document, id) = FleetDocument::load_by_name("glm-pair", &roots).expect("fleet loads");
    let exact = document.exact().expect("exact");

    // The Router is a *reference* to a separately saved service.
    assert_eq!(exact.reasoning_router.as_deref(), Some("luna-low"));
    assert!(exact.legacy_inline_router().is_none());

    let (profile, router_id) =
        ReasoningRouterProfile::load_by_name("luna-low", &roots).expect("router loads");
    assert_eq!(router_id.qualified(), "workspace/luna-low");
    let captured = CapturedReasoningRouter::from_profile(&profile, router_id.origin);

    let snapshot = FleetSnapshot::capture(id, &document, "2026-07-26T00:00:00Z", Some(captured))
        .expect("capture");

    // Routes are frozen from the snapshot, before any reasoning resolution.
    let member = snapshot.member("implementer").expect("member");
    let frozen = member.route.clone();
    assert_eq!(frozen.provider, "zai");
    assert_eq!(frozen.model, "glm-5");

    // The Router is a non-dispatchable service with no authority.
    let router = snapshot.router().expect("router service");
    assert_eq!(router.service_kind, REASONING_ROUTER_SERVICE_KIND);
    assert_eq!(router.route.model, "gpt-5.6-luna");
    assert_eq!(router.requested_call_reasoning, RouterCallReasoning::Low);
    assert!(!router.dispatchable);
    assert!(!router.permissions.tools);

    let decision = parse_router_decision(
        r#"```json
{"reasoning":"max"}
```"#,
    )
    .expect("router decision parses");

    let identity = RouterIdentity::from_captured(
        router,
        Some(&route("router", "openai", "gpt-5.6-luna")),
        Some(
            router_call_plan(
                router.requested_call_reasoning,
                &ReasoningCapability::tiered(),
            )
            .disclosure,
        ),
    );

    let resolved = resolve_exact_member_reasoning(
        &member.id,
        &frozen,
        member.requested_reasoning,
        &ReasoningCapability::tiered(),
        &RouterAvailability::Ready,
        Some(&decision),
        Some(&identity),
    )
    .expect("ready router resolves auto");

    assert_eq!(resolved.requested(), RequestedReasoning::Auto);
    assert_eq!(
        resolved.effective(),
        EffectiveReasoning::Tier(ReasoningTier::Max)
    );
    assert_eq!(resolved.source(), EffectiveReasoningSource::FleetRouter);

    // The router's own call ran at the configured `low`, and says so.
    let call = resolved
        .router()
        .expect("router identity")
        .call
        .as_ref()
        .expect("call disclosure");
    assert_eq!(call.requested, "low");
    assert_eq!(call.effective, "low");

    // The worker's provider/model did not move.
    assert_eq!(snapshot.member("implementer").unwrap().route, frozen);
}

/// One saved Router profile, referenced by two different Fleets.
#[test]
fn one_router_profile_serves_two_fleets() {
    let tmp = tempfile::tempdir().expect("tmp");
    std::fs::create_dir_all(tmp.path().join("fleets")).expect("fleets dir");
    std::fs::create_dir_all(tmp.path().join(REASONING_ROUTER_DIR)).expect("routers dir");
    std::fs::write(
        tmp.path().join(REASONING_ROUTER_DIR).join("luna-low.toml"),
        LUNA,
    )
    .expect("router");
    std::fs::write(tmp.path().join("fleets/glm-pair.toml"), GLM_FLEET).expect("first");
    std::fs::write(
        tmp.path().join("fleets/glm-solo.toml"),
        GLM_FLEET.replace("name = \"glm-pair\"", "name = \"glm-solo\""),
    )
    .expect("second");
    let roots = vec![FleetSearchRoot::new("workspace", tmp.path())];

    let mut snapshots = Vec::new();
    for name in ["glm-pair", "glm-solo"] {
        let (document, id) = FleetDocument::load_by_name(name, &roots).expect("fleet loads");
        let reference = document
            .exact()
            .expect("exact")
            .reasoning_router
            .clone()
            .expect("router reference");
        let (profile, router_id) =
            ReasoningRouterProfile::load_by_name(&reference, &roots).expect("router loads");
        snapshots.push(
            FleetSnapshot::capture(
                id,
                &document,
                "2026-07-26T00:00:00Z",
                Some(CapturedReasoningRouter::from_profile(
                    &profile,
                    router_id.origin,
                )),
            )
            .expect("capture"),
        );
    }

    assert_eq!(
        snapshots[0].router(),
        snapshots[1].router(),
        "both fleets attach the identical captured router service"
    );
    assert_ne!(snapshots[0].fleet(), snapshots[1].fleet());
    assert_eq!(
        snapshots[0].router().expect("router").qualified(),
        "workspace/luna-low"
    );
}

/// A bare Router name defined in two origins is ambiguous; a qualified origin
/// resolves it. Shadowing would silently change which provider sees every
/// routing summary.
#[test]
fn a_router_defined_in_two_origins_is_ambiguous_until_qualified() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    for root in [&home, &workspace] {
        std::fs::create_dir_all(root.join(REASONING_ROUTER_DIR)).expect("routers dir");
    }
    std::fs::write(
        home.join(REASONING_ROUTER_DIR).join("luna-low.toml"),
        LUNA.replace("gpt-5.6-luna", "gpt-5.6-luna-mini"),
    )
    .expect("home");
    std::fs::write(
        workspace.join(REASONING_ROUTER_DIR).join("luna-low.toml"),
        LUNA,
    )
    .expect("workspace");

    let roots = vec![
        FleetSearchRoot::new("codewhale_home", &home),
        FleetSearchRoot::new("workspace", &workspace),
    ];

    let err = ReasoningRouterProfile::load_by_name("luna-low", &roots)
        .expect_err("a bare name must not be resolved by shadowing");
    assert!(
        matches!(err, ReasoningRouterError::AmbiguousRouter { .. }),
        "{err:?}"
    );

    assert_eq!(
        ReasoningRouterProfile::load_by_name("workspace/luna-low", &roots)
            .expect("qualified")
            .0
            .model,
        "gpt-5.6-luna"
    );
    assert_eq!(
        ReasoningRouterProfile::load_by_name("codewhale_home/luna-low", &roots)
            .expect("qualified")
            .0
            .model,
        "gpt-5.6-luna-mini"
    );
}

/// A Router may only run at `off` or `low`. `medium`/`high`/`max` are rejected,
/// not silently clamped — the operator must see that their setting was refused.
#[test]
fn an_expensive_router_call_tier_is_rejected_rather_than_clamped() {
    for value in ["medium", "high", "max"] {
        let text = LUNA.replace("\"low\"", &format!("\"{value}\""));
        let err = ReasoningRouterProfile::parse(&text).expect_err("expensive tier");
        assert!(
            matches!(err, ReasoningRouterError::CallReasoningTooExpensive { .. }),
            "value={value} err={err:?}"
        );
    }

    // And `low` is honored end to end, never forced to `off` behind the label.
    let profile = ReasoningRouterProfile::parse(LUNA).expect("parse");
    let plan = router_call_plan(profile.call_reasoning, &ReasoningCapability::tiered());
    assert_eq!(plan.tier, ReasoningTier::Low);
    assert_eq!(plan.disclosure.effective, "low");
    assert_eq!(plan.disclosure.provider_effective, "low");
}

/// A manual tier consults no Router at all.
#[test]
fn a_non_auto_member_never_consults_the_router() {
    let document = FleetDocument::parse(GLM_FLEET).expect("parse");
    let exact = document.exact().expect("exact");
    let auditor = exact.member("auditor").expect("auditor");

    let resolved = resolve_exact_member_reasoning(
        &auditor.id,
        &auditor.frozen_route(),
        auditor.reasoning,
        &ReasoningCapability::tiered(),
        // Deliberately absent — an explicit tier must not need a router.
        &RouterAvailability::Absent,
        None,
        None,
    )
    .expect("explicit tier resolves");

    assert_eq!(
        resolved.effective(),
        EffectiveReasoning::Tier(ReasoningTier::High)
    );
    assert_eq!(resolved.source(), EffectiveReasoningSource::MemberExplicit);
    assert!(resolved.router().is_none(), "no router was involved");
}

#[test]
fn an_auto_member_in_a_router_less_fleet_fails_before_work_starts() {
    let router_less = GLM_FLEET.replace("reasoning_router = \"luna-low\"\n", "");
    let document = FleetDocument::parse(&router_less).expect("parse");
    let exact = document.exact().expect("exact");
    assert!(exact.router_ref().is_none());

    let member = exact.member("implementer").expect("member");
    let err = resolve_exact_member_reasoning(
        &member.id,
        &member.frozen_route(),
        member.reasoning,
        &ReasoningCapability::tiered(),
        &RouterAvailability::Absent,
        None,
        None,
    )
    .expect_err("auto without a router must fail closed");

    let message = err.to_string();
    assert!(message.contains("implementer"), "{message}");
    assert!(message.contains("reasoning_router"), "{message}");
}

/// A Fleet that references a Router profile which is not installed cannot be
/// resolved — decided locally, with no provider contacted.
#[test]
fn a_missing_router_profile_is_a_local_load_failure() {
    let (_tmp, roots) = workspace_with(GLM_FLEET, None);
    let (document, _id) = FleetDocument::load_by_name("glm-pair", &roots).expect("fleet loads");
    let reference = document
        .exact()
        .expect("exact")
        .reasoning_router
        .clone()
        .expect("reference");

    assert!(matches!(
        ReasoningRouterProfile::load_by_name(&reference, &roots).expect_err("missing"),
        ReasoningRouterError::NotFound { .. }
    ));
}

#[test]
fn legacy_permissions_do_not_enter_the_member_snapshot() {
    let document = FleetDocument::parse(GLM_FLEET).expect("parse");
    let snapshot = FleetSnapshot::capture(
        fleet_id("glm-pair"),
        &document,
        "2026-07-26T00:00:00Z",
        Some(luna()),
    )
    .expect("capture");

    let implementer = snapshot.member("implementer").expect("member");
    let encoded = serde_json::to_value(implementer).expect("serialize member snapshot");
    assert!(
        encoded.get("permissions").is_none(),
        "Fleet identity/snapshot must not own Runtime authority: {encoded}"
    );
}

/// The bounded routing summary is transmitted exactly once, and the receipt's
/// count and hash describe exactly those bytes.
#[test]
fn the_routing_summary_is_transmitted_once_and_disclosed_without_content() {
    let payload = bounded_routing_payload("refactor the parser in /Users/hunter/app");
    let disclosure = payload.disclosure().clone();
    let input = codewhale_workflow::RouterCallInput {
        fleet: "workspace/glm-pair".to_string(),
        member_id: "implementer".to_string(),
        frozen: codewhale_workflow::FrozenRoute {
            provider: "zai".to_string(),
            model: "glm-5".to_string(),
        },
        payload,
    };

    let system = router_system_prompt(&input);
    let user = router_user_message(&input);

    assert!(!system.contains("refactor the parser"), "{system}");
    assert!(!user.contains("/Users/"), "paths are redacted: {user}");
    // The disclosed count and hash describe exactly the bytes that were sent —
    // and the summary appears exactly once across the whole request.
    assert_eq!(disclosure.transmitted_bytes, user.len());
    assert_eq!(disclosure.transmitted_chars, user.chars().count());
    assert_eq!(
        format!("{system}\n{user}").matches(user.as_str()).count(),
        1,
        "the bounded summary must be transmitted once, not duplicated"
    );
    assert!(disclosure.redacted);
    assert!(disclosure.redactions.contains(&"absolute_path".to_string()));
}

/// Legacy role-map fleets keep loading through the same store.
#[test]
fn legacy_fleet_files_still_load_through_the_same_store() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let (document, id) =
        FleetDocument::load_by_name("stopship", &[FleetSearchRoot::new("workspace", root)])
            .expect("workspace legacy fleet loads");

    assert!(document.is_legacy());
    assert_eq!(document.schema_kind(), "legacy");
    assert_eq!(id.qualified(), "workspace/stopship");
    let legacy = document.legacy().expect("legacy body");
    legacy.validate_stopship_roles().expect("required roles");
    // The stopship fixture binds release_lead to the canonical advisor role
    // (role-only world: no saved manager member to bind).
    assert_eq!(legacy.resolve("release_lead").unwrap(), "advisor");
}

/// A personal `~/.codewhale` Fleet must not silently shadow — or be shadowed
/// by — a project Fleet of the same name.
#[test]
fn an_exact_fleet_defined_in_two_origins_is_ambiguous_until_qualified() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    for root in [&home, &workspace] {
        std::fs::create_dir_all(root.join("fleets")).expect("fleets dir");
    }
    std::fs::write(
        home.join("fleets/glm-pair.toml"),
        GLM_FLEET.replace("model = \"glm-5\"", "model = \"glm-4\""),
    )
    .expect("home fleet");
    std::fs::write(workspace.join("fleets/glm-pair.toml"), GLM_FLEET).expect("workspace fleet");

    let roots = vec![
        FleetSearchRoot::new("codewhale_home", &home),
        FleetSearchRoot::new("workspace", &workspace),
    ];

    let err = FleetDocument::load_by_name("glm-pair", &roots)
        .expect_err("an exact fleet must not be resolved by shadowing");
    assert!(
        matches!(err, NamedFleetError::AmbiguousFleet { .. }),
        "{err:?}"
    );

    let (document, id) =
        FleetDocument::load_by_name("workspace/glm-pair", &roots).expect("qualified load");
    assert_eq!(id.qualified(), "workspace/glm-pair");
    assert_eq!(
        document
            .exact()
            .expect("exact")
            .member("implementer")
            .expect("member")
            .model,
        "glm-5"
    );

    let (home_document, home_id) =
        FleetDocument::load_by_name("codewhale_home/glm-pair", &roots).expect("qualified load");
    assert_eq!(home_id.qualified(), "codewhale_home/glm-pair");
    assert_eq!(
        home_document
            .exact()
            .expect("exact")
            .member("implementer")
            .expect("member")
            .model,
        "glm-4"
    );
}

/// Legacy role maps keep their historic first-hit-wins behavior: a role map
/// resolves through the same profile store from either origin.
#[test]
fn legacy_fleets_in_two_origins_keep_first_hit_wins() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    for root in [&home, &workspace] {
        std::fs::create_dir_all(root.join("fleets")).expect("fleets dir");
    }
    std::fs::write(
        home.join("fleets/pair.toml"),
        "name = \"pair\"\n\n[roles]\nscout = \"home-scout\"\n",
    )
    .expect("home fleet");
    std::fs::write(
        workspace.join("fleets/pair.toml"),
        "name = \"pair\"\n\n[roles]\nscout = \"workspace-scout\"\n",
    )
    .expect("workspace fleet");

    let (document, id) = FleetDocument::load_by_name(
        "pair",
        &[
            FleetSearchRoot::new("codewhale_home", &home),
            FleetSearchRoot::new("workspace", &workspace),
        ],
    )
    .expect("legacy collisions stay resolvable");

    assert!(document.is_legacy());
    assert_eq!(id.origin, "codewhale_home");
    assert_eq!(
        document.legacy().expect("legacy").resolve("scout").unwrap(),
        "home-scout"
    );
}

/// A broken file in a *shadowed* origin must not fail a legacy load that has
/// always worked.
#[test]
fn a_malformed_shadowed_sibling_does_not_regress_legacy_first_hit() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    let workspace = tmp.path().join("workspace");
    for root in [&home, &workspace] {
        std::fs::create_dir_all(root.join("fleets")).expect("fleets dir");
    }
    std::fs::write(
        home.join("fleets/pair.toml"),
        "name = \"pair\"\n\n[roles]\nscout = \"home-scout\"\n",
    )
    .expect("home fleet");
    std::fs::write(
        workspace.join("fleets/pair.toml"),
        "name = \"pair\"\n[roles\nscout = = = \"\"\"broken\n",
    )
    .expect("workspace fleet");

    let (document, id) = FleetDocument::load_by_name(
        "pair",
        &[
            FleetSearchRoot::new("codewhale_home", &home),
            FleetSearchRoot::new("workspace", &workspace),
        ],
    )
    .expect("a broken shadowed sibling must not break first-hit-wins");

    assert_eq!(id.origin, "codewhale_home");
    assert_eq!(
        document.legacy().expect("legacy").resolve("scout").unwrap(),
        "home-scout"
    );
}

/// Roster invariants hold at the crate boundary, not just inside the parser.
#[test]
fn duplicate_roles_and_reserved_router_identities_are_rejected_at_load() {
    let duplicate_role = GLM_FLEET.replace("role = \"reviewer\"", "role = \"builder\"");
    assert!(
        FleetDocument::parse(&duplicate_role).is_err(),
        "two members must not share the role `builder`"
    );

    let worker_router = GLM_FLEET.replace("role = \"reviewer\"", "role = \"router\"");
    assert!(
        FleetDocument::parse(&worker_router).is_err(),
        "a worker must not claim the reserved role `router`"
    );
}

/// The legacy inline Router form still parses and normalizes into the same
/// captured service — one runtime representation, whichever way it was written.
#[test]
fn a_legacy_inline_router_still_works_and_normalizes() {
    let inline = format!(
        "{}\n[[members]]\nid = \"router\"\nkind = \"router\"\nprovider = \"zai\"\nmodel = \
         \"glm-5-turbo\"\n",
        GLM_FLEET.replace("reasoning_router = \"luna-low\"\n", "")
    );
    let document = FleetDocument::parse(&inline).expect("legacy inline parses");
    let exact = document.exact().expect("exact");
    let captured = codewhale_workflow::captured_legacy_inline_router(exact).expect("inline router");

    assert!(captured.legacy_inline);
    assert_eq!(captured.service_kind, REASONING_ROUTER_SERVICE_KIND);
    assert_eq!(captured.route.model, "glm-5-turbo");
    assert_eq!(captured.requested_call_reasoning, RouterCallReasoning::Off);
    assert!(!captured.is_dispatchable());

    let snapshot = FleetSnapshot::capture(
        fleet_id("glm-pair"),
        &document,
        "2026-07-26T00:00:00Z",
        Some(captured),
    )
    .expect("capture");
    assert!(snapshot.member("router").is_none());
    assert_eq!(snapshot.members().len(), 2);
}

/// The provider-effective control is reported separately from the selector
/// tier: a Z.AI GLM route expresses only thinking on/off, so `high` and `max`
/// must not be presented as two distinct provider-effective tiers.
#[test]
fn glm_receipts_do_not_invent_distinct_high_and_max_provider_tiers() {
    let document = FleetDocument::parse(GLM_FLEET).expect("parse");
    let exact = document.exact().expect("exact");
    let auditor = exact.member("auditor").expect("auditor");
    let glm = ReasoningCapability::enabled_disabled();

    let high = resolve_exact_member_reasoning(
        &auditor.id,
        &auditor.frozen_route(),
        RequestedReasoning::High,
        &glm,
        &RouterAvailability::Absent,
        None,
        None,
    )
    .expect("resolve");
    let max = resolve_exact_member_reasoning(
        &auditor.id,
        &auditor.frozen_route(),
        RequestedReasoning::Max,
        &glm,
        &RouterAvailability::Absent,
        None,
        None,
    )
    .expect("resolve");

    assert_eq!(
        high.provider_effective(),
        ProviderEffectiveReasoning::Enabled
    );
    assert_eq!(
        max.provider_effective(),
        ProviderEffectiveReasoning::Enabled
    );
    assert_ne!(high.effective(), max.effective());
}
