use std::fs;
use std::path::{Path, PathBuf};

use deepseek_game::cli::{RuntimeCommand, RuntimeRequest, handle_json_request, handle_request};
use serde_json::json;

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/games")
        .join(name)
        .to_string_lossy()
        .to_string()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "genmicon-runtime-cli-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
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
fn runtime_cli_returns_structured_invalid_request_error() {
    let response = handle_json_request("{not json");
    assert!(!response.ok);
    assert_eq!(response.error.expect("error").code, "invalid_request");
}

#[test]
fn runtime_cli_validates_reconciliation_fixture() {
    let response = handle_request(RuntimeRequest {
        command: RuntimeCommand::Validate,
        game_root: fixture("reconciliation-demo").into(),
        save_id: Some("default".to_string()),
        developer: false,
        payload: json!({}),
    });

    assert!(response.ok, "{response:?}");
    let data = response.data.expect("data");
    assert_eq!(
        data.pointer("/game/id"),
        Some(&json!("reconciliation-demo"))
    );
    assert_eq!(data.pointer("/driver/id"), Some(&json!("galgame")));
    assert!(data.pointer("/save/revision").is_some());
}

#[test]
fn runtime_cli_lists_thirteen_angry_man_saves() {
    let response = handle_request(RuntimeRequest {
        command: RuntimeCommand::ListSaves,
        game_root: fixture("thirteen-angry-man").into(),
        save_id: None,
        developer: false,
        payload: json!({}),
    });

    assert!(response.ok, "{response:?}");
    let saves = response
        .data
        .as_ref()
        .and_then(|data| data.pointer("/saves"))
        .and_then(|value| value.as_array())
        .expect("saves");
    assert!(saves.iter().any(|save| save["id"] == "default"));
}

#[test]
fn runtime_cli_fact_check_blocks_known_impossible_claim() {
    let response = handle_request(RuntimeRequest {
        command: RuntimeCommand::FactCheck,
        game_root: fixture("reconciliation-demo").into(),
        save_id: Some("default".to_string()),
        developer: false,
        payload: json!({
            "player_action": "I say I am pregnant with your child."
        }),
    });

    assert!(response.ok, "{response:?}");
    assert_eq!(
        response
            .data
            .as_ref()
            .and_then(|data| data.pointer("/passed")),
        Some(&json!(false))
    );
}

#[test]
fn runtime_cli_commit_turn_advances_save_once_and_rejects_replay() {
    let temp = TempDir::new("commit-once");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/games/reconciliation-demo");
    let game_root = temp.path().join("reconciliation-demo");
    copy_dir_all(&source, &game_root);

    let before = handle_request(RuntimeRequest {
        command: RuntimeCommand::Status,
        game_root: game_root.clone(),
        save_id: Some("default".to_string()),
        developer: false,
        payload: json!({}),
    });
    assert!(before.ok, "{before:?}");
    let before_data = before.data.as_ref().expect("before data");
    let before_revision = before_data
        .pointer("/save/revision")
        .and_then(|value| value.as_u64())
        .expect("before revision");
    let before_turn_count = before_data
        .pointer("/save/turn_count")
        .and_then(|value| value.as_u64())
        .expect("before turn count");

    let commit_payload = json!({
        "expected_revision": before_revision,
        "player_input": "I admit I was scared and ask her to wait.",
        "resolution": "She stops long enough to hear the apology.",
        "state_patch": {
            "scene": {
                "summary": "The player admits fear instead of deflecting."
            }
        }
    });
    let commit = handle_request(RuntimeRequest {
        command: RuntimeCommand::CommitTurn,
        game_root: game_root.clone(),
        save_id: Some("default".to_string()),
        developer: false,
        payload: commit_payload.clone(),
    });
    assert!(commit.ok, "{commit:?}");
    assert_eq!(
        commit
            .data
            .as_ref()
            .and_then(|data| data.pointer("/revision"))
            .and_then(|value| value.as_u64()),
        Some(before_revision + 1)
    );
    assert!(
        commit
            .data
            .as_ref()
            .and_then(|data| data.pointer("/view"))
            .is_some()
    );

    let replay = handle_request(RuntimeRequest {
        command: RuntimeCommand::CommitTurn,
        game_root: game_root.clone(),
        save_id: Some("default".to_string()),
        developer: false,
        payload: commit_payload,
    });
    assert!(!replay.ok);
    assert_eq!(
        replay.error.as_ref().map(|error| error.code.as_str()),
        Some("revision_conflict")
    );

    let after = handle_request(RuntimeRequest {
        command: RuntimeCommand::Status,
        game_root,
        save_id: Some("default".to_string()),
        developer: false,
        payload: json!({}),
    });
    assert!(after.ok, "{after:?}");
    let after_data = after.data.as_ref().expect("after data");
    assert_eq!(
        after_data
            .pointer("/save/revision")
            .and_then(|value| value.as_u64()),
        Some(before_revision + 1)
    );
    assert_eq!(
        after_data
            .pointer("/save/turn_count")
            .and_then(|value| value.as_u64()),
        Some(before_turn_count + 1)
    );
}

#[test]
fn runtime_cli_resumes_from_save_without_transcript_context() {
    let temp = TempDir::new("resume");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/games/thirteen-angry-man");
    let game_root = temp.path().join("thirteen-angry-man");
    copy_dir_all(&source, &game_root);

    let status = handle_request(RuntimeRequest {
        command: RuntimeCommand::Status,
        game_root: game_root.clone(),
        save_id: Some("default".to_string()),
        developer: false,
        payload: json!({}),
    });
    assert!(status.ok, "{status:?}");
    assert_eq!(
        status
            .data
            .as_ref()
            .and_then(|data| data.pointer("/save/id")),
        Some(&json!("default"))
    );

    let render = handle_request(RuntimeRequest {
        command: RuntimeCommand::Render,
        game_root,
        save_id: Some("default".to_string()),
        developer: false,
        payload: json!({}),
    });
    assert!(render.ok, "{render:?}");
    assert_eq!(
        render
            .data
            .as_ref()
            .and_then(|data| data.pointer("/view/revision")),
        status
            .data
            .as_ref()
            .and_then(|data| data.pointer("/save/revision"))
    );
}

fn copy_dir_all(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("create target dir");
    for entry in fs::read_dir(source).expect("read source dir") {
        let entry = entry.expect("read source entry");
        let ty = entry.file_type().expect("read file type");
        let target_path = target.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target_path);
        } else if ty.is_file() {
            fs::copy(entry.path(), target_path).expect("copy fixture file");
        }
    }
}
