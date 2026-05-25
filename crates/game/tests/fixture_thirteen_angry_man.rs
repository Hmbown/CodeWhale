use std::path::Path;

use deepseek_game::cli::{RuntimeCommand, RuntimeRequest, handle_request};
use serde_json::json;

#[test]
fn thirteen_angry_man_fixture_status_render_and_lookup_load() {
    let game_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/games/thirteen-angry-man");

    for command in [RuntimeCommand::Status, RuntimeCommand::Render] {
        let response = handle_request(RuntimeRequest {
            command,
            game_root: game_root.clone(),
            save_id: Some("default".to_string()),
            developer: false,
            payload: json!({}),
        });
        assert!(response.ok, "{response:?}");
    }

    let lookup = handle_request(RuntimeRequest {
        command: RuntimeCommand::Lookup,
        game_root,
        save_id: Some("default".to_string()),
        developer: false,
        payload: json!({
            "handle": "case.md",
            "max_bytes": 2048
        }),
    });
    assert!(lookup.ok, "{lookup:?}");
}
