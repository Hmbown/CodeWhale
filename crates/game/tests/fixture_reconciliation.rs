use std::path::Path;

use deepseek_game::cli::{RuntimeCommand, RuntimeRequest, handle_request};
use serde_json::json;

#[test]
fn reconciliation_fixture_status_render_and_playbook_load() {
    let game_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/games/reconciliation-demo");

    for command in [
        RuntimeCommand::Status,
        RuntimeCommand::Render,
        RuntimeCommand::Playbook,
    ] {
        let response = handle_request(RuntimeRequest {
            command,
            game_root: game_root.clone(),
            save_id: Some("default".to_string()),
            developer: false,
            payload: json!({}),
        });
        assert!(response.ok, "{response:?}");
    }
}
