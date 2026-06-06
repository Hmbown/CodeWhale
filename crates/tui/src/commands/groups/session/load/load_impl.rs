use std::path::PathBuf;

use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};
use crate::tui::history::history_cells_from_message;

pub(crate) fn load(app: &mut App, path: Option<&str>) -> CommandResult {
    let load_path = if let Some(p) = path {
        if p.contains('/') || p.contains('\\') {
            PathBuf::from(p)
        } else {
            app.workspace.join(p)
        }
    } else {
        return CommandResult::error("Usage: /load <path>");
    };

    let content = match std::fs::read_to_string(&load_path) {
        Ok(c) => c,
        Err(e) => {
            return CommandResult::error(format!("Failed to read session file: {e}"));
        }
    };

    let session: crate::session_manager::SavedSession = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::error(format!("Failed to parse session file: {e}"));
        }
    };

    app.api_messages.clone_from(&session.messages);
    app.clear_history();
    let cells_to_add: Vec<_> = app
        .api_messages
        .iter()
        .flat_map(history_cells_from_message)
        .collect();
    app.extend_history(cells_to_add);
    app.mark_history_updated();
    app.viewport.transcript_selection.clear();
    app.set_model_selection(session.metadata.model.clone());
    app.update_model_compaction_budget();
    app.workspace.clone_from(&session.metadata.workspace);
    app.session.total_tokens = u32::try_from(session.metadata.total_tokens).unwrap_or(u32::MAX);
    app.session.total_conversation_tokens = app.session.total_tokens;
    app.session.reset_token_breakdown();
    app.session.session_cost = 0.0;
    app.session.session_cost_cny = 0.0;
    app.session.subagent_cost = 0.0;
    app.session.subagent_cost_cny = 0.0;
    app.session.subagent_cost_event_seqs.clear();
    app.session.displayed_cost_high_water = 0.0;
    app.session.displayed_cost_high_water_cny = 0.0;
    app.session.last_prompt_tokens = None;
    app.session.last_completion_tokens = None;
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.last_reasoning_replay_tokens = None;
    app.session.turn_cache_history.clear();
    app.current_session_id = Some(session.metadata.id.clone());
    app.session_artifacts = session.artifacts.clone();
    if let Some(sp) = session.system_prompt {
        app.system_prompt = Some(crate::models::SystemPrompt::Text(sp));
    }
    app.scroll_to_bottom();

    CommandResult::with_message_and_action(
        format!(
            "Session loaded from {} (ID: {}, {} messages)",
            load_path.display(),
            crate::session_manager::truncate_id(&session.metadata.id),
            session.metadata.message_count
        ),
        AppAction::SyncSession {
            session_id: app.current_session_id.clone(),
            messages: app.api_messages.clone(),
            system_prompt: app.system_prompt.clone(),
            model: app.model.clone(),
            workspace: app.workspace.clone(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::session::save::save_impl::save;
    use crate::commands::groups::session::test_support::create_test_app_with_tmpdir;
    use crate::config::DEFAULT_TEXT_MODEL;
    use crate::tui::app::{ReasoningEffort, TurnCacheRecord};
    use std::time::Instant;
    use tempfile::TempDir;

    #[test]
    fn test_load_without_path_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = load(&mut app, None);
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Usage: /load"));
    }

    #[test]
    fn test_load_nonexistent_file_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = load(&mut app, Some("nonexistent.json"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Failed to read"));
    }

    #[test]
    fn test_load_invalid_json_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let bad_file = tmpdir.path().join("bad.json");
        std::fs::write(&bad_file, "not valid json").unwrap();
        let result = load(&mut app, Some(bad_file.to_str().unwrap()));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Failed to parse"));
    }

    #[test]
    fn test_load_valid_session_restores_state() {
        let tmpdir = TempDir::new().unwrap();
        let mut app1 = create_test_app_with_tmpdir(&tmpdir);
        app1.api_messages.push(crate::models::Message {
            role: "user".to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
        });
        app1.session.total_tokens = 500;
        let save_path = tmpdir.path().join("test.json");
        save(&mut app1, Some(save_path.to_str().unwrap()));

        let mut app2 = create_test_app_with_tmpdir(&tmpdir);
        let result = load(&mut app2, Some(save_path.to_str().unwrap()));

        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Session loaded from"));
        assert!(msg.contains("ID:"));
        assert!(msg.contains("messages"));
        assert_eq!(app2.api_messages.len(), 1);
        assert_eq!(app2.session.total_tokens, 500);
        assert!(app2.current_session_id.is_some());
        assert!(matches!(result.action, Some(AppAction::SyncSession { .. })));
    }

    #[test]
    fn load_auto_model_session_restores_auto_mode() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app.set_model_selection("auto".to_string());
        saved_app.last_effective_model = Some("deepseek-v4-flash".to_string());
        saved_app.last_effective_reasoning_effort = Some(ReasoningEffort::Low);
        let save_path = tmpdir.path().join("auto_model.json");
        save(&mut saved_app, Some(save_path.to_str().unwrap()));

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.set_model_selection("deepseek-v4-flash".to_string());
        app.reasoning_effort = ReasoningEffort::High;
        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert!(!result.is_error);
        assert!(app.auto_model);
        assert_eq!(app.model, "auto");
        assert_eq!(app.model_selection_for_persistence(), "auto");
        assert_eq!(app.last_effective_model, None);
        assert_eq!(app.last_effective_reasoning_effort, None);
        assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
        assert_eq!(app.effective_model_for_budget(), DEFAULT_TEXT_MODEL);
    }

    #[test]
    fn load_restores_artifact_registry() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app
            .session_artifacts
            .push(crate::artifacts::ArtifactRecord {
                id: "art_call_big".to_string(),
                kind: crate::artifacts::ArtifactKind::ToolOutput,
                session_id: "artifact-session".to_string(),
                tool_call_id: "call-big".to_string(),
                tool_name: "exec_shell".to_string(),
                created_at: chrono::Utc::now(),
                byte_size: 128,
                preview: "checking crate".to_string(),
                storage_path: tmpdir.path().join("call-big.txt"),
            });
        let save_path = tmpdir.path().join("artifact_load.json");
        save(&mut saved_app, Some(save_path.to_str().unwrap()));

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.session_artifacts
            .push(crate::artifacts::ArtifactRecord {
                id: "art_stale".to_string(),
                kind: crate::artifacts::ArtifactKind::ToolOutput,
                session_id: "stale-session".to_string(),
                tool_call_id: "stale".to_string(),
                tool_name: "exec_shell".to_string(),
                created_at: chrono::Utc::now(),
                byte_size: 1,
                preview: "stale".to_string(),
                storage_path: tmpdir.path().join("stale.txt"),
            });

        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert!(!result.is_error);
        assert_eq!(app.session_artifacts, saved_app.session_artifacts);
    }

    #[test]
    fn load_resets_cache_history_and_cost() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app.api_messages.push(crate::models::Message {
            role: "user".to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: "checkpoint".to_string(),
                cache_control: None,
            }],
        });
        saved_app.session.total_tokens = 500;
        let save_path = tmpdir.path().join("checkpoint.json");
        save(&mut saved_app, Some(save_path.to_str().unwrap()));

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.session.session_cost = 1.25;
        app.session.session_cost_cny = 9.13;
        app.session.subagent_cost = 0.75;
        app.session.subagent_cost_cny = 5.48;
        app.session.subagent_cost_event_seqs.insert(42);
        app.session.displayed_cost_high_water = 2.0;
        app.session.displayed_cost_high_water_cny = 14.61;
        app.session.last_prompt_tokens = Some(120);
        app.session.last_completion_tokens = Some(35);
        app.session.last_prompt_cache_hit_tokens = Some(80);
        app.session.last_prompt_cache_miss_tokens = Some(40);
        app.session.last_reasoning_replay_tokens = Some(12);
        app.push_turn_cache_record(TurnCacheRecord {
            input_tokens: 120,
            output_tokens: 35,
            cache_hit_tokens: Some(80),
            cache_miss_tokens: Some(40),
            reasoning_replay_tokens: Some(12),
            recorded_at: Instant::now(),
        });

        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert!(result.message.is_some());
        assert_eq!(app.session.total_tokens, 500);
        assert_eq!(app.session.total_conversation_tokens, 500);
        assert_eq!(app.session.session_cost, 0.0);
        assert_eq!(app.session.session_cost_cny, 0.0);
        assert_eq!(app.session.subagent_cost, 0.0);
        assert_eq!(app.session.subagent_cost_cny, 0.0);
        assert!(app.session.subagent_cost_event_seqs.is_empty());
        assert_eq!(app.session.displayed_cost_high_water, 0.0);
        assert_eq!(app.session.displayed_cost_high_water_cny, 0.0);
        assert_eq!(app.session.last_prompt_tokens, None);
        assert_eq!(app.session.last_completion_tokens, None);
        assert_eq!(app.session.last_prompt_cache_hit_tokens, None);
        assert_eq!(app.session.last_prompt_cache_miss_tokens, None);
        assert_eq!(app.session.last_reasoning_replay_tokens, None);
        assert!(app.session.turn_cache_history.is_empty());
    }
}
