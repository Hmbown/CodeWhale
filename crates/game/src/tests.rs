use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::agents::build_agent_packs;
use crate::driver::{DriverResolver, load_driver};
use crate::interaction::build_playbook;
use crate::lookup::{HARD_LOOKUP_BYTES, LookupRequest, lookup};
use crate::manifest::load_game;
use crate::render::{RenderPanelKind, render_panels};
use crate::save::{CommitRequest, STATE_FILE, TURN_LOG_FILE, commit_turn, load_save};
use crate::script::{DriverCall, run_driver_function};
use crate::{GameError, demo};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "deepseek-game-test-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn game_manifest_loads_and_rejects_escaping_paths() {
    let temp = TempDir::new("manifest");
    let game = temp.path().join("game");
    write_demo_game(&game);

    let loaded = load_game(&game).expect("valid game should load");
    assert_eq!(loaded.manifest.game.id, "reconcile-demo");
    assert_eq!(
        loaded.saves_root,
        fs::canonicalize(&game)
            .expect("canonical game root")
            .join("saves")
    );
    assert!(loaded.warnings.is_empty());

    fs::write(
        game.join("game.toml"),
        r#"
[game]
id = "reconcile-demo"
title = "Reconcile Demo"
version = "0.1.0"

[driver]
id = "galgame"
version = "^0.1"

[content]
roots = ["../outside"]

[saves]
root = "saves"
"#,
    )
    .expect("write invalid manifest");

    let err = load_game(&game).expect_err("escaping content root should fail");
    assert!(matches!(err, GameError::InvalidPath { .. }));
}

#[test]
fn driver_resolver_selects_highest_matching_version_and_exact_reload() {
    let temp = TempDir::new("driver");
    let drivers = temp.path().join("drivers");
    write_driver(&drivers, "galgame", "0.1.0");
    write_driver(&drivers, "galgame", "0.1.5");
    write_driver(&drivers, "galgame", "0.2.0");

    let resolver = DriverResolver::new([drivers.clone()]);
    let resolved = resolver
        .resolve("galgame", "^0.1")
        .expect("caret requirement should resolve");
    assert_eq!(resolved.loaded.manifest.driver.version, "0.1.5");

    let exact = resolver
        .resolve_exact("galgame", "0.1.0")
        .expect("exact version should resolve");
    assert_eq!(exact.loaded.manifest.driver.version, "0.1.0");

    let err = resolver
        .resolve_exact("galgame", "9.9.9")
        .expect_err("missing exact version should fail");
    assert!(matches!(err, GameError::DriverNotFound { .. }));
}

#[test]
fn driver_resolver_rejects_manifest_version_mismatch() {
    let temp = TempDir::new("driver-mismatch");
    let drivers = temp.path().join("drivers");
    write_driver(&drivers, "galgame", "0.1.0");
    fs::write(
        drivers.join("galgame/0.1.0/driver.toml"),
        r#"
[driver]
id = "galgame"
title = "Galgame Driver"
version = "0.2.0"

[scripts]
root = "scripts"
"#,
    )
    .expect("write mismatched driver manifest");

    let resolver = DriverResolver::new([drivers]);
    let err = resolver
        .resolve_exact("galgame", "0.1.0")
        .expect_err("manifest version mismatch should fail");
    assert!(matches!(err, GameError::InvalidDriverManifest(_)));
}

#[test]
fn save_loads_commits_merge_patch_and_rejects_stale_revision() {
    let temp = TempDir::new("save");
    let save_root = temp.path().join("saves/default");
    fs::create_dir_all(&save_root).expect("create save");
    fs::write(
        save_root.join(STATE_FILE),
        serde_json::to_vec_pretty(&demo::reconciliation_initial_state("galgame", "0.1.0"))
            .expect("serialize state"),
    )
    .expect("write state");
    fs::write(save_root.join(TURN_LOG_FILE), "").expect("write turn log");

    let loaded = load_save(temp.path().join("saves"), "default").expect("save should load");
    assert_eq!(loaded.turns.len(), 0);

    let outcome = commit_turn(
        &save_root,
        CommitRequest {
            expected_revision: 0,
            player_input: "I tell her I was scared, not indifferent.".to_string(),
            resolution: "She stops on the last stair.".to_string(),
            state_patch: json!({
                "scene": {
                    "summary": "The player admitted fear instead of deflecting."
                },
                "world": {
                    "flags": {
                        "honest_apology": true
                    }
                }
            }),
            driver_results: [("relationship_score".to_string(), json!({"delta": 2}))]
                .into_iter()
                .collect(),
            metadata: Default::default(),
        },
    )
    .expect("commit should succeed");

    assert_eq!(outcome.turn.turn_id, "000001");
    assert_eq!(outcome.turn.revision_after, 1);
    assert_eq!(outcome.state["revision"], 1);
    assert_eq!(
        outcome.state.pointer("/world/flags/honest_apology"),
        Some(&Value::Bool(true))
    );

    let reloaded = load_save(temp.path().join("saves"), "default").expect("save should reload");
    assert_eq!(reloaded.turns.len(), 1);
    assert_eq!(reloaded.state["revision"], 1);

    let stale = commit_turn(
        &save_root,
        CommitRequest {
            expected_revision: 0,
            player_input: "stale".to_string(),
            resolution: "stale".to_string(),
            state_patch: json!({}),
            driver_results: Default::default(),
            metadata: Default::default(),
        },
    )
    .expect_err("stale revision should fail");
    assert!(matches!(stale, GameError::RevisionConflict { .. }));
    let after_stale = load_save(temp.path().join("saves"), "default").expect("save should reload");
    assert_eq!(after_stale.turns.len(), 1);
}

#[test]
fn lookup_is_bounded_and_rejects_handle_traversal() {
    let temp = TempDir::new("lookup");
    let game = temp.path().join("game");
    fs::create_dir_all(game.join("content/locations")).expect("create content");
    let long_body = "trust ".repeat(HARD_LOOKUP_BYTES);
    fs::write(game.join("content/locations/overpass.md"), &long_body).expect("write content");

    let result = lookup(
        &game,
        &[game.join("content")],
        LookupRequest {
            handle: Some("locations/overpass".to_string()),
            query: None,
            max_bytes: Some(usize::MAX),
        },
    )
    .expect("lookup should succeed");
    assert_eq!(result.bytes_returned, HARD_LOOKUP_BYTES);
    assert!(result.truncated);
    assert_eq!(
        result.excerpts[0].source_handle,
        "content/locations/overpass.md"
    );

    let err = lookup(
        &game,
        &[game.join("content")],
        LookupRequest {
            handle: Some("../saves/default/STATE.json".to_string()),
            query: None,
            max_bytes: None,
        },
    )
    .expect_err("lookup traversal should fail");
    assert!(matches!(err, GameError::InvalidPath { .. }));
}

#[test]
fn render_panels_use_state_ui_or_structured_fallback() {
    let state = demo::reconciliation_initial_state("galgame", "0.1.0");
    let panels = render_panels(&state);
    assert_eq!(panels.len(), 4);
    assert_eq!(panels[0].kind, RenderPanelKind::Scene);
    assert_eq!(panels[2].kind, RenderPanelKind::Actions);
    assert_eq!(panels[3].kind, RenderPanelKind::Story);

    let fallback = json!({
        "scene": {"location": "Room", "summary": "A quiet room."},
        "player": {"name": "Ari", "inventory": ["key"]},
        "world": {"quests": ["leave"]}
    });
    let panels = render_panels(&fallback);
    assert_eq!(panels.len(), 3);
    assert_eq!(panels[0].title, "Room");
}

#[test]
fn playbook_exposes_choices_and_git_like_story_branch() {
    let state = demo::reconciliation_initial_state("galgame", "0.1.0");
    let playbook = build_playbook(&state);

    assert_eq!(playbook.active_branch.as_deref(), Some("mainline"));
    assert_eq!(
        playbook.story_style.as_ref().map(|style| style.id.as_str()),
        Some("emotional_reconciliation")
    );
    assert_eq!(
        playbook.active_node.as_ref().map(|node| node.id.as_str()),
        Some("opening_apology")
    );
    assert_eq!(playbook.suggestions.len(), 3);
    assert_eq!(
        playbook.suggestions[0].target_node.as_deref(),
        Some("honest_admission")
    );
    assert!(playbook.warnings.is_empty(), "{:?}", playbook.warnings);
}

#[test]
fn playbook_reports_non_fatal_story_warnings() {
    let state = json!({
        "schema_version": 1,
        "revision": 0,
        "driver": {
            "id": "test",
            "version": "0.1.0"
        },
        "interaction": {
            "suggestions": [
                {
                    "id": "bad_choice",
                    "label": "Bad",
                    "input": "[ASK] Missing target",
                    "target_node": "missing"
                }
            ]
        },
        "story": {
            "active_branch": "mainline",
            "active_node": "start",
            "branches": {
                "mainline": {
                    "head": "start"
                }
            },
            "nodes": {
                "start": {
                    "title": "Start",
                    "status": "active",
                    "next": ["missing"]
                }
            }
        }
    });

    let playbook = build_playbook(&state);
    assert_eq!(
        playbook.active_node.as_ref().map(|node| node.id.as_str()),
        Some("start")
    );
    assert!(
        playbook
            .warnings
            .iter()
            .any(|warning| warning.contains("missing next node missing")),
        "{:?}",
        playbook.warnings
    );
    assert!(
        playbook
            .warnings
            .iter()
            .any(|warning| warning.contains("targets missing story node missing")),
        "{:?}",
        playbook.warnings
    );
}

#[test]
fn agent_packs_are_limited_to_driver_declared_roles() {
    let temp = TempDir::new("agents");
    let drivers = temp.path().join("drivers");
    write_driver(&drivers, "galgame", "0.1.0");
    let loaded = load_driver(drivers.join("galgame/0.1.0")).expect("driver should load");
    let state = demo::reconciliation_initial_state("galgame", "0.1.0");
    let packs = build_agent_packs(&loaded.manifest, &state);

    assert_eq!(packs.len(), 2);
    assert_eq!(packs[0].role, "state_manager");
    assert_eq!(
        packs[0].callable_driver_functions,
        vec!["relationship_score".to_string()]
    );
    assert!(packs[0].relevant_save_slice.get("player").is_some());
}

#[test]
fn driver_script_runs_only_declared_starlark_functions() {
    let temp = TempDir::new("script");
    let drivers = temp.path().join("drivers");
    write_driver(&drivers, "galgame", "0.1.0");
    let loaded = load_driver(drivers.join("galgame/0.1.0")).expect("driver should load");

    let result = run_driver_function(
        &loaded.root,
        &loaded.manifest,
        DriverCall {
            function: "relationship_score".to_string(),
            args: [
                ("current_score".to_string(), json!(3)),
                ("player_action".to_string(), json!("apologize clearly")),
            ]
            .into_iter()
            .collect(),
        },
    )
    .expect("declared starlark function should run");

    assert_eq!(result.result["delta"], 2);
    assert_eq!(result.result["score"], 5);

    let undeclared = run_driver_function(
        &loaded.root,
        &loaded.manifest,
        DriverCall {
            function: "open".to_string(),
            args: Default::default(),
        },
    )
    .expect_err("undeclared functions should fail before script execution");
    assert!(matches!(undeclared, GameError::Script(_)));
}

fn write_demo_game(game: &Path) {
    fs::create_dir_all(game.join("content")).expect("create content");
    fs::create_dir_all(game.join("saves/default")).expect("create saves");
    fs::create_dir_all(game.join("skills/dm")).expect("create skill");
    fs::write(game.join("content/INDEX.md"), "# Index\n").expect("write index");
    fs::write(game.join("skills/dm/SKILL.md"), "# DM\n").expect("write skill");
    fs::write(
        game.join("game.toml"),
        r#"
[game]
id = "reconcile-demo"
title = "Reconcile Demo"
version = "0.1.0"
entry_skill = "dm"
default_save = "default"

[driver]
id = "galgame"
version = "^0.1"

[content]
index = "content/INDEX.md"
roots = ["content"]

[saves]
root = "saves"
"#,
    )
    .expect("write game manifest");
}

fn write_driver(root: &Path, id: &str, version: &str) {
    let driver = root.join(id).join(version);
    fs::create_dir_all(driver.join("scripts")).expect("create scripts");
    fs::create_dir_all(driver.join("agent_templates")).expect("create templates");
    fs::write(
        driver.join("scripts/relationship.star"),
        "def relationship_score(current_score, player_action):\n    delta = 2 if \"apologize\" in player_action else -1\n    return {\"score\": current_score + delta, \"delta\": delta}\n",
    )
    .expect("write script");
    fs::write(driver.join("agent_templates/state_manager.md"), "state").expect("write template");
    fs::write(driver.join("agent_templates/plot_manager.md"), "plot").expect("write template");
    fs::write(
        driver.join("driver.toml"),
        format!(
            r#"
[driver]
id = "{id}"
title = "Galgame Driver"
version = "{version}"

[runtime]
script_engine = "starlark"
default_topology = "dynamic-main-plus-managers"

[scripts]
root = "scripts"

[subagents]
default_roles = ["state_manager", "plot_manager", "npc_manager_a"]
max_active = 2

[subagents.templates]
state_manager = "agent_templates/state_manager.md"
plot_manager = "agent_templates/plot_manager.md"
npc_manager = "agent_templates/npc_manager.md"

[functions.relationship_score]
script = "scripts/relationship.star"
function = "relationship_score"
mutates = false
"#
        ),
    )
    .expect("write driver manifest");
}
