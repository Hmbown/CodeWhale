use super::{
    CODE_EXECUTION_DESCRIPTION, DEFAULT_ACTIVE_NATIVE_TOOLS,
    allowlist_is_native_file_and_shell_only, apply_mcp_tool_deferral, apply_native_tool_deferral,
    apply_registry_first_shell_guidance, build_model_tool_catalog_with_surface,
    default_synthetic_catalog_tool_names, ensure_advanced_tooling, execute_tool_search_with_cache,
    initial_active_tools, is_synthetic_catalog_tool, remove_evicted_cache_activations,
    tool_matches_any_rule, touch_cached_tool_after_execution,
};
use crate::core::session::ToolActivationCache;
use crate::models::Tool;
use crate::tui::app::AppMode;
use serde_json::json;
use std::collections::{BTreeSet, HashSet};

fn tool(name: &str) -> Tool {
    Tool {
        tool_type: None,
        name: name.to_string(),
        description: format!("{name} test tool"),
        input_schema: json!({"type": "object", "properties": {}}),
        allowed_callers: None,
        defer_loading: None,
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

/// `code_execution` writes the script to a tempdir and runs it as a plain
/// child process in the workspace — no seccomp, no jail, no container. The
/// description is model-facing, so calling it a sandbox would tell the model
/// it has isolation the runtime never provides.
#[test]
fn code_execution_description_does_not_claim_process_sandboxing() {
    assert!(CODE_EXECUTION_DESCRIPTION.contains("local Python interpreter"));
    assert!(!CODE_EXECUTION_DESCRIPTION.contains("sandbox"));
}

/// The published synthetic-name list and the predicate that classifies a
/// catalog entry as synthetic must agree. A name that appears in the list
/// but is not classified synthetic would let the request projection report
/// a provenance the engine itself disputes.
#[test]
fn published_synthetic_names_agree_with_the_synthetic_predicate() {
    let names = default_synthetic_catalog_tool_names();
    assert!(!names.is_empty());
    for name in &names {
        assert!(
            is_synthetic_catalog_tool(name),
            "'{name}' is published as synthetic but the predicate disagrees"
        );
    }
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(names, sorted, "the list must be sorted and deduplicated");

    // MCP names resolve through the real pool, so they are deliberately
    // absent here even though the predicate accepts them.
    assert!(!names.iter().any(|name| name.starts_with("mcp_")));

    // `multi_tool_use.parallel` is a call name, never a catalog entry, so
    // it has no catalog provenance and must not be published as synthetic.
    assert!(
        !names
            .iter()
            .any(|name| name == super::MULTI_TOOL_PARALLEL_NAME)
    );
}

#[test]
fn first_turn_surface_is_stable_across_plan_work_and_full_access() {
    assert_eq!(
        DEFAULT_ACTIVE_NATIVE_TOOLS,
        &["read", "write", "edit", "bash", "agent", "todo_write"]
    );
    let expected = [
        "agent",
        "bash",
        "edit",
        "read",
        "todo_write",
        "tool_search",
        "write",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    for mode in [AppMode::Plan, AppMode::Agent, AppMode::Yolo] {
        let mut catalog = [
            "read",
            "write",
            "edit",
            "bash",
            "agent",
            "todo_write",
            "Git",
            "Run",
            "tasks",
            "load_skill",
        ]
        .into_iter()
        .map(tool)
        .collect::<Vec<_>>();
        let always_load = HashSet::new();
        apply_native_tool_deferral(&mut catalog, &always_load);
        ensure_advanced_tooling(&mut catalog, mode, &always_load);
        let active = initial_active_tools(&catalog)
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(active, expected, "{mode:?}");
    }
}

#[test]
fn mcp_tools_are_searchable_not_eager_in_every_mode() {
    for mode in [AppMode::Plan, AppMode::Agent, AppMode::Yolo] {
        let mut catalog = vec![tool("read_mcp_resource"), tool("mcp_acme_lookup")];
        apply_mcp_tool_deferral(&mut catalog, mode, &HashSet::new());
        assert!(
            catalog
                .iter()
                .all(|definition| definition.defer_loading == Some(true)),
            "{mode:?}: {catalog:?}"
        );
    }
}

#[test]
fn cache_eviction_does_not_hide_a_tool_that_became_eager() {
    let mut catalog = vec![tool("promoted")];
    catalog[0].defer_loading = Some(true);
    let mut cache = ToolActivationCache::default();
    cache.activate(&catalog, &["promoted".to_string()]);

    catalog[0].defer_loading = Some(false);
    let mut active = initial_active_tools(&catalog);
    let evicted = cache.revalidate(&catalog);
    remove_evicted_cache_activations(&catalog, &mut active, evicted);

    assert!(active.contains("promoted"));
    assert_eq!(cache.names().count(), 0);
}

#[test]
fn successful_cached_execution_updates_lru_without_granting_uncached_names() {
    let catalog = (0..=8)
        .map(|index| {
            let mut definition = tool(&format!("deferred-{index}"));
            definition.defer_loading = Some(true);
            definition
        })
        .collect::<Vec<_>>();
    let mut cache = ToolActivationCache::default();
    let first = (0..8)
        .map(|index| format!("deferred-{index}"))
        .collect::<Vec<_>>();
    let mut active = HashSet::new();
    let delta = cache.activate(&catalog, &first);
    active.extend(delta.admitted);

    assert!(touch_cached_tool_after_execution(
        &catalog,
        &mut active,
        &mut cache,
        "deferred-0"
    ));
    let delta = cache.activate(&catalog, &["deferred-8".to_string()]);
    remove_evicted_cache_activations(&catalog, &mut active, delta.evicted);
    active.extend(delta.admitted);
    assert!(cache.names().any(|name| name == "deferred-0"));
    assert!(!cache.names().any(|name| name == "deferred-1"));

    assert!(!touch_cached_tool_after_execution(
        &catalog,
        &mut active,
        &mut cache,
        "never-activated"
    ));
    assert!(!active.contains("never-activated"));
}

#[test]
fn searching_for_an_eager_tool_is_not_reported_as_cache_rejected() {
    let mut catalog = vec![tool("read")];
    catalog[0].defer_loading = Some(false);
    let mut active = initial_active_tools(&catalog);
    let mut cache = ToolActivationCache::default();

    let result = execute_tool_search_with_cache(
        super::TOOL_SEARCH_NAME,
        &json!({"query": "read"}),
        &catalog,
        &mut active,
        &mut cache,
    )
    .expect("tool search should succeed");

    let metadata = result.metadata.expect("search metadata");
    assert_eq!(metadata["tool_references"], json!([]));
    assert_eq!(metadata["unavailable_tool_references"], json!([]));
    assert!(active.contains("read"));
    assert_eq!(cache.names().count(), 0);
}

#[test]
fn allow_and_deny_rules_cover_visible_and_hidden_compat_aliases_symmetrically() {
    for family in [
        &["read", "read_file"][..],
        &["write", "write_file"][..],
        &["edit", "edit_file"][..],
        &["bash", "Bash", "exec_shell"][..],
    ] {
        for rule in family {
            let rules = vec![(*rule).to_string()];
            for tool_name in family {
                assert!(
                    tool_matches_any_rule(&rules, tool_name),
                    "rule {rule:?} should cover alias {tool_name:?}"
                );
            }
        }
    }

    assert!(tool_matches_any_rule(&["exec_shell*".to_string()], "bash"));
    assert!(tool_matches_any_rule(
        &["exec_shell*".to_string()],
        "exec_shell_wait"
    ));
    for primitive in ["read", "write", "edit"] {
        assert!(tool_matches_any_rule(&["File".to_string()], primitive));
    }
    assert!(!tool_matches_any_rule(&["read".to_string()], "write"));
}

#[test]
fn native_file_and_shell_allowlist_can_skip_mcp_startup() {
    let native = ["bash", "read", "write", "edit"].map(str::to_string);
    assert!(allowlist_is_native_file_and_shell_only(Some(&native)));
    assert!(!allowlist_is_native_file_and_shell_only(None));
}

#[test]
fn unknown_and_wildcard_allowlists_keep_mcp_startup() {
    for rule in ["mcp_github_list_prs", "mcp_*", "m*", "*", "other_tool"] {
        assert!(
            !allowlist_is_native_file_and_shell_only(Some(&[rule.to_string()])),
            "{rule} may admit an MCP-backed tool"
        );
    }
}

#[test]
fn compact_surface_keeps_the_exact_eager_agent_head() {
    let catalog = build_model_tool_catalog_with_surface(
        ["read", "write", "edit", "bash", "agent", "todo_write"]
            .into_iter()
            .map(tool)
            .collect(),
        Vec::new(),
        AppMode::Agent,
        &HashSet::new(),
        crate::model_profile::ToolSurfaceBudget::Compact,
    );

    assert_eq!(
        catalog
            .iter()
            .find(|definition| definition.name == "agent")
            .and_then(|definition| definition.defer_loading),
        Some(false)
    );
}

#[test]
fn registry_first_guidance_does_not_expand_contract_bash_schema_text() {
    let mut catalog = vec![tool("bash")];
    let description = catalog[0].description.clone();

    apply_registry_first_shell_guidance(&mut catalog);

    assert_eq!(catalog[0].description, description);
}
