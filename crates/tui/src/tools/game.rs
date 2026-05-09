//! Native Game Console tools.

use std::collections::BTreeMap;

use async_trait::async_trait;
use deepseek_game::lookup::LookupRequest;
use deepseek_game::save::CommitRequest;
use deepseek_game::script::DriverCall;
use serde_json::{Map, Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

pub struct GameStatusTool;
pub struct GameRenderTool;
pub struct GamePlaybookTool;
pub struct GameLookupTool;
pub struct GameRunDriverTool;
pub struct GameCommitTurnTool;

#[async_trait]
impl ToolSpec for GameStatusTool {
    fn name(&self) -> &'static str {
        "game_status"
    }

    fn description(&self) -> &'static str {
        "Return validation and identity details for the active Game Console session."
    }

    fn input_schema(&self) -> Value {
        empty_schema()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, _input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let session = loaded_game(context)?;
        ToolResult::json(&json!({
            "game_id": session.game_id,
            "title": session.title,
            "save_id": session.save_id,
            "revision": session.revision,
            "driver_id": session.driver_id,
            "driver_version": session.locked_driver_version.as_deref().unwrap_or(&session.driver_requirement),
            "driver_resolved": session.driver_root.is_some(),
            "developer_mode": session.developer_mode,
            "warnings": session.warnings,
            "status": context.game_session.as_ref().map(crate::game::GameSession::status_report),
        }))
        .map_err(to_tool_error)
    }
}

#[async_trait]
impl ToolSpec for GameRenderTool {
    fn name(&self) -> &'static str {
        "game_render"
    }

    fn description(&self) -> &'static str {
        "Return structured player-facing panels rendered from the active game save."
    }

    fn input_schema(&self) -> Value {
        empty_schema()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, _input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let session = loaded_game(context)?;
        let save = deepseek_game::save::load_save(&session.saves_root, &session.save_id)
            .map_err(to_tool_error)?;
        let panels = deepseek_game::render::render_panels(&save.state);
        ToolResult::json(&json!({
            "save_id": save.id,
            "revision": save.state.get("revision").and_then(Value::as_u64),
            "panels": panels,
        }))
        .map_err(to_tool_error)
    }
}

#[async_trait]
impl ToolSpec for GamePlaybookTool {
    fn name(&self) -> &'static str {
        "game_playbook"
    }

    fn description(&self) -> &'static str {
        "Return the active game's current commands, suggested choices, and visible story-branch nodes."
    }

    fn input_schema(&self) -> Value {
        empty_schema()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, _input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let session = loaded_game(context)?;
        let save = deepseek_game::save::load_save(&session.saves_root, &session.save_id)
            .map_err(to_tool_error)?;
        let playbook = deepseek_game::interaction::build_playbook(&save.state);
        ToolResult::json(&json!({
            "save_id": save.id,
            "revision": save.state.get("revision").and_then(Value::as_u64),
            "playbook": playbook,
            "display": deepseek_game::interaction::format_playbook(&playbook),
        }))
        .map_err(to_tool_error)
    }
}

#[async_trait]
impl ToolSpec for GameLookupTool {
    fn name(&self) -> &'static str {
        "game_lookup"
    }

    fn description(&self) -> &'static str {
        "Retrieve bounded markdown/text content excerpts from declared content roots in the active game package."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "handle": {
                    "type": "string",
                    "description": "Optional content handle such as 'lore/scene.md'."
                },
                "query": {
                    "type": "string",
                    "description": "Optional case-insensitive search query over game content."
                },
                "max_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": deepseek_game::lookup::HARD_LOOKUP_BYTES,
                    "description": "Maximum bytes to return. Defaults to 16 KiB and is hard-capped at 32 KiB."
                }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let session = loaded_game(context)?;
        let loaded_game =
            deepseek_game::manifest::load_game(&session.game_root).map_err(to_tool_error)?;
        let request = LookupRequest {
            handle: optional_string(&input, "handle"),
            query: optional_string(&input, "query"),
            max_bytes: input
                .get("max_bytes")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
        };
        let result =
            deepseek_game::lookup::lookup(&session.game_root, &loaded_game.content_roots, request)
                .map_err(to_tool_error)?;
        ToolResult::json(&result).map_err(to_tool_error)
    }
}

#[async_trait]
impl ToolSpec for GameRunDriverTool {
    fn name(&self) -> &'static str {
        "game_run_driver"
    }

    fn description(&self) -> &'static str {
        "Run a manifest-declared deterministic Starlark driver function for the active game."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "function": {
                    "type": "string",
                    "description": "Declared driver function name."
                },
                "args": {
                    "type": "object",
                    "description": "JSON-compatible named arguments passed to the Starlark function.",
                    "additionalProperties": true
                }
            },
            "required": ["function"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let session = loaded_game(context)?;
        let driver_root = session
            .driver_root
            .as_ref()
            .ok_or_else(|| ToolError::not_available("active game driver is not resolved"))?;
        let loaded_driver =
            deepseek_game::driver::load_driver(driver_root).map_err(to_tool_error)?;
        let function = required_string(&input, "function")?;
        let args = input
            .get("args")
            .and_then(Value::as_object)
            .map(map_to_btree)
            .unwrap_or_default();
        let result = deepseek_game::script::run_driver_function(
            driver_root,
            &loaded_driver.manifest,
            DriverCall { function, args },
        )
        .map_err(to_tool_error)?;
        ToolResult::json(&result).map_err(to_tool_error)
    }
}

#[async_trait]
impl ToolSpec for GameCommitTurnTool {
    fn name(&self) -> &'static str {
        "game_commit_turn"
    }

    fn description(&self) -> &'static str {
        "Atomically append one game turn and apply an RFC 7396 JSON Merge Patch to the active save."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "expected_revision": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Current save revision expected by the model."
                },
                "player_input": {
                    "type": "string",
                    "description": "Player action being resolved."
                },
                "resolution": {
                    "type": "string",
                    "description": "Player-facing turn resolution."
                },
                "state_patch": {
                    "type": "object",
                    "description": "RFC 7396 JSON Merge Patch to apply to STATE.json.",
                    "additionalProperties": true
                },
                "driver_results": {
                    "type": "object",
                    "description": "Optional deterministic driver outputs used during the turn.",
                    "additionalProperties": true
                },
                "metadata": {
                    "type": "object",
                    "description": "Optional internal metadata for the turn log.",
                    "additionalProperties": true
                }
            },
            "required": ["expected_revision", "player_input", "resolution", "state_patch"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let session = loaded_game(context)?;
        let expected_revision = input
            .get("expected_revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| ToolError::missing_field("expected_revision"))?;
        let player_input = required_string(&input, "player_input")?;
        let resolution = required_string(&input, "resolution")?;
        let state_patch = input
            .get("state_patch")
            .cloned()
            .ok_or_else(|| ToolError::missing_field("state_patch"))?;
        if !state_patch.is_object() {
            return Err(ToolError::invalid_input(
                "state_patch must be a JSON object merge patch",
            ));
        }
        let driver_results = input
            .get("driver_results")
            .and_then(Value::as_object)
            .map(map_to_btree)
            .unwrap_or_default();
        let metadata = input
            .get("metadata")
            .and_then(Value::as_object)
            .map(map_to_btree)
            .unwrap_or_default();

        let outcome = deepseek_game::save::commit_turn(
            session.saves_root.join(&session.save_id),
            CommitRequest {
                expected_revision,
                player_input,
                resolution,
                state_patch,
                driver_results,
                metadata,
            },
        )
        .map_err(to_tool_error)?;
        let panels = deepseek_game::render::render_panels(&outcome.state);
        ToolResult::json(&json!({
            "turn": outcome.turn,
            "state": outcome.state,
            "panels": panels,
        }))
        .map_err(to_tool_error)
    }
}

fn loaded_game(context: &ToolContext) -> Result<&crate::game::LoadedGameSession, ToolError> {
    match context.game_session.as_ref() {
        Some(crate::game::GameSession::Loaded(session)) => Ok(session),
        Some(crate::game::GameSession::Notice(notice)) => Err(ToolError::not_available(format!(
            "no loaded game session: {}",
            notice.message
        ))),
        None => Err(ToolError::not_available(
            "no active game session; use `deepseek play` or `/play` first",
        )),
    }
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn optional_string(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn required_string(input: &Value, key: &str) -> Result<String, ToolError> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ToolError::missing_field(key))
}

fn map_to_btree(map: &Map<String, Value>) -> BTreeMap<String, Value> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn to_tool_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::execution_failed(error.to_string())
}
