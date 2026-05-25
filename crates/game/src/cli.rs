use std::io::{Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::driver::{DriverResolver, ResolvedDriver};
use crate::interaction::{build_playbook, format_playbook};
use crate::lookup::{LookupRequest, lookup};
use crate::manifest::{LoadedGame, load_game};
use crate::render::{render_panels, render_view_snapshot};
use crate::save::{CommitRequest, LoadedSave, commit_turn, driver_lock, load_save, revision};
use crate::script::{DriverCall, run_driver_function};
use crate::{GameError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeRequest {
    pub command: RuntimeCommand,
    pub game_root: PathBuf,
    #[serde(default)]
    pub save_id: Option<String>,
    #[serde(default)]
    pub developer: bool,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCommand {
    Validate,
    Status,
    Render,
    Playbook,
    Lookup,
    FactCheck,
    RunDriver,
    CommitTurn,
    ListSaves,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeResponse {
    pub ok: bool,
    pub data: Option<Value>,
    pub warnings: Vec<String>,
    pub error: Option<RuntimeError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

pub fn run_stdio() -> i32 {
    let mut input = String::new();
    let response = match std::io::stdin().read_to_string(&mut input) {
        Ok(_) => handle_json_request(&input),
        Err(err) => RuntimeResponse::failure(RuntimeError {
            code: "io_error".to_string(),
            message: err.to_string(),
            recoverable: false,
        }),
    };

    let output = serde_json::to_string(&response).unwrap_or_else(|err| {
        format!(
            r#"{{"ok":false,"data":null,"warnings":[],"error":{{"code":"serialization_error","message":"{}","recoverable":false}}}}"#,
            escape_json_string(&err.to_string())
        )
    });
    let _ = writeln!(std::io::stdout(), "{output}");
    if response.ok { 0 } else { 1 }
}

pub fn handle_json_request(input: &str) -> RuntimeResponse {
    match serde_json::from_str::<RuntimeRequest>(input) {
        Ok(request) => handle_request(request),
        Err(err) => RuntimeResponse::failure(RuntimeError {
            code: "invalid_request".to_string(),
            message: err.to_string(),
            recoverable: true,
        }),
    }
}

pub fn handle_request(request: RuntimeRequest) -> RuntimeResponse {
    let result = match request.command {
        RuntimeCommand::Validate => validate(&request),
        RuntimeCommand::Status => status(&request),
        RuntimeCommand::Render => render(&request),
        RuntimeCommand::Playbook => playbook(&request),
        RuntimeCommand::Lookup => run_lookup(&request),
        RuntimeCommand::FactCheck => fact_check(&request),
        RuntimeCommand::RunDriver => run_driver(&request),
        RuntimeCommand::CommitTurn => commit(&request),
        RuntimeCommand::ListSaves => list_saves(&request),
    };
    match result {
        Ok((data, warnings)) => RuntimeResponse::success(data, warnings),
        Err(err) => RuntimeResponse::failure(error_from_game_error(&err)),
    }
}

impl RuntimeResponse {
    fn success(data: Value, warnings: Vec<String>) -> Self {
        Self {
            ok: true,
            data: Some(data),
            warnings,
            error: None,
        }
    }

    fn failure(error: RuntimeError) -> Self {
        Self {
            ok: false,
            data: None,
            warnings: Vec::new(),
            error: Some(error),
        }
    }
}

fn validate(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let game = load_game(&request.game_root)?;
    let save_id = save_id(request, &game);
    let mut warnings = game.warnings.clone();
    let driver = resolve_manifest_driver(&game)?;
    warnings.extend(driver.loaded.warnings.clone());
    let save = load_save(&game.saves_root, &save_id)?;

    Ok((
        json!({
            "game": game_summary(&game),
            "driver": driver_summary(&driver),
            "save": save_summary(&save)?,
            "active_tool_profile": "player",
        }),
        warnings,
    ))
}

fn status(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let (game, save, driver, warnings) = load_ready_save(request)?;
    Ok((
        json!({
            "game": game_summary(&game),
            "driver": driver_summary(&driver),
            "save": save_summary(&save)?,
            "warnings": warnings,
        }),
        warnings,
    ))
}

fn render(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let (_game, save, _driver, warnings) = load_ready_save(request)?;
    Ok((
        json!({
            "view": render_view_snapshot(&save.state),
            "panels": render_panels(&save.state),
        }),
        warnings,
    ))
}

fn playbook(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let (_game, save, _driver, warnings) = load_ready_save(request)?;
    let playbook = build_playbook(&save.state);
    Ok((
        json!({
            "playbook": playbook,
            "text": format_playbook(&playbook),
        }),
        warnings,
    ))
}

fn run_lookup(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let game = load_game(&request.game_root)?;
    let lookup_request =
        serde_json::from_value::<LookupRequest>(request.payload.clone()).map_err(payload_error)?;
    let result = lookup(&game.root, &game.content_roots, lookup_request)?;
    Ok((json!(result), game.warnings.clone()))
}

fn fact_check(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let (_game, save, _driver, warnings) = load_ready_save(request)?;
    let payload = serde_json::from_value::<FactCheckPayload>(request.payload.clone())
        .map_err(payload_error)?;
    let text = payload_text(&payload);
    let issues = fact_gate_issues(&save.state, &text);
    Ok((
        json!({
            "passed": issues.is_empty(),
            "issues": issues,
            "checked_text": text,
        }),
        warnings,
    ))
}

fn run_driver(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let (_game, save, driver, warnings) = load_ready_save(request)?;
    let call =
        serde_json::from_value::<DriverCall>(request.payload.clone()).map_err(payload_error)?;
    let result = run_driver_function(&driver.loaded.root, &driver.loaded.manifest, call)?;
    Ok((
        json!({
            "save_revision": revision(&save.state)?,
            "result": result,
        }),
        warnings,
    ))
}

fn commit(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let game = load_game(&request.game_root)?;
    let save_id = save_id(request, &game);
    let mut warnings = game.warnings.clone();
    let save = load_save(&game.saves_root, &save_id)?;
    let commit_request =
        serde_json::from_value::<CommitRequest>(request.payload.clone()).map_err(payload_error)?;
    let outcome = commit_turn(&save.root, commit_request)?;
    warnings.push("save committed through runtime authority".to_string());
    Ok((
        json!({
            "save_id": save_id,
            "turn": outcome.turn,
            "revision": revision(&outcome.state)?,
            "view": render_view_snapshot(&outcome.state),
        }),
        warnings,
    ))
}

fn list_saves(request: &RuntimeRequest) -> Result<(Value, Vec<String>)> {
    let game = load_game(&request.game_root)?;
    let mut warnings = game.warnings.clone();
    let mut saves = Vec::new();

    let entries = std::fs::read_dir(&game.saves_root).map_err(|source| GameError::Read {
        path: game.saves_root.clone(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| GameError::Read {
            path: game.saves_root.clone(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| GameError::Read {
            path: entry.path(),
            source,
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        match load_save(&game.saves_root, &id).and_then(|save| save_summary(&save)) {
            Ok(summary) => saves.push(summary),
            Err(err) => warnings.push(format!("save {id} skipped: {err}")),
        }
    }

    Ok((
        json!({
            "game": game_summary(&game),
            "saves": saves,
        }),
        warnings,
    ))
}

fn load_ready_save(
    request: &RuntimeRequest,
) -> Result<(LoadedGame, LoadedSave, ResolvedDriver, Vec<String>)> {
    let game = load_game(&request.game_root)?;
    let save_id = save_id(request, &game);
    let save = load_save(&game.saves_root, &save_id)?;
    let lock = driver_lock(&save.state)?;
    let driver = DriverResolver::new(driver_roots(&game)).resolve_exact(&lock.id, &lock.version)?;
    let mut warnings = game.warnings.clone();
    warnings.extend(driver.loaded.warnings.clone());
    Ok((game, save, driver, warnings))
}

fn resolve_manifest_driver(game: &LoadedGame) -> Result<ResolvedDriver> {
    DriverResolver::new(driver_roots(game))
        .resolve(&game.manifest.driver.id, &game.manifest.driver.version)
}

fn driver_roots(game: &LoadedGame) -> Vec<PathBuf> {
    vec![game.root.join("drivers")]
}

fn save_id(request: &RuntimeRequest, game: &LoadedGame) -> String {
    request
        .save_id
        .clone()
        .or_else(|| game.manifest.game.default_save.clone())
        .unwrap_or_else(|| "default".to_string())
}

fn game_summary(game: &LoadedGame) -> Value {
    json!({
        "id": game.manifest.game.id,
        "title": game.manifest.game.title,
        "version": game.manifest.game.version,
        "root": game.root,
        "entry_skill": game.manifest.game.entry_skill,
        "default_save": game.manifest.game.default_save,
        "content_index": game.content_index,
        "content_roots": game.content_roots,
        "saves_root": game.saves_root,
    })
}

fn driver_summary(driver: &ResolvedDriver) -> Value {
    json!({
        "id": driver.loaded.manifest.driver.id,
        "title": driver.loaded.manifest.driver.title,
        "version": driver.loaded.manifest.driver.version,
        "root": driver.loaded.root,
        "install_root": driver.install_root,
        "functions": driver.loaded.manifest.functions.keys().collect::<Vec<_>>(),
    })
}

fn save_summary(save: &LoadedSave) -> Result<Value> {
    Ok(json!({
        "id": save.id,
        "root": save.root,
        "revision": revision(&save.state)?,
        "driver": driver_lock(&save.state)?,
        "turn_count": save.turns.len(),
        "last_turn_id": save.turns.last().map(|turn| turn.turn_id.clone()),
        "has_summary": save.summary.is_some(),
        "has_agents": save.agents.is_some(),
    }))
}

#[derive(Debug, Clone, Deserialize)]
struct FactCheckPayload {
    #[serde(default)]
    player_action: Option<String>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

fn payload_text(payload: &FactCheckPayload) -> String {
    [
        payload.player_action.as_deref(),
        payload.resolution.as_deref(),
        payload.text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
}

fn fact_gate_issues(state: &Value, text: &str) -> Vec<Value> {
    let normalized = text.to_lowercase();
    let Some(rules) = state
        .pointer("/facts/fact_gate/rules")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    rules
        .iter()
        .filter_map(|rule| {
            let patterns = rule.get("patterns").and_then(Value::as_array)?;
            let matched = patterns
                .iter()
                .filter_map(Value::as_str)
                .any(|pattern| normalized.contains(&pattern.to_lowercase()));
            if !matched || unless_path_allows(state, rule) || !block_if_matches(state, rule) {
                return None;
            }
            Some(json!({
                "id": rule.get("id").and_then(Value::as_str).unwrap_or("fact_gate"),
                "reason": rule.get("reason").and_then(Value::as_str).unwrap_or("Protected fact gate failed."),
                "correction": rule.get("correction").and_then(Value::as_str),
            }))
        })
        .collect()
}

fn unless_path_allows(state: &Value, rule: &Value) -> bool {
    let Some(path) = rule.get("unless_path").and_then(Value::as_str) else {
        return false;
    };
    state
        .pointer(path)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn block_if_matches(state: &Value, rule: &Value) -> bool {
    let Some(blocks) = rule.get("block_if").and_then(Value::as_array) else {
        return rule.get("unless_path").is_some();
    };
    blocks.iter().any(|block| {
        let Some(path) = block.get("path").and_then(Value::as_str) else {
            return false;
        };
        let Some(expected) = block.get("equals") else {
            return false;
        };
        state.pointer(path) == Some(expected)
    })
}

fn payload_error(source: serde_json::Error) -> GameError {
    GameError::SaveValidation(format!("invalid runtime payload: {source}"))
}

fn error_from_game_error(error: &GameError) -> RuntimeError {
    RuntimeError {
        code: error_code(error).to_string(),
        message: error.to_string(),
        recoverable: is_recoverable(error),
    }
}

fn error_code(error: &GameError) -> &'static str {
    match error {
        GameError::Read { .. } => "read_error",
        GameError::Write { .. } => "write_error",
        GameError::Toml { .. } => "toml_error",
        GameError::Json { .. } => "json_error",
        GameError::InvalidManifest(_) => "invalid_manifest",
        GameError::InvalidDriverManifest(_) => "invalid_driver_manifest",
        GameError::InvalidPath { .. } => "invalid_path",
        GameError::PathEscape { .. } => "path_escape",
        GameError::DriverNotFound { .. } => "driver_not_found",
        GameError::InvalidVersionRequirement(_) => "invalid_version_requirement",
        GameError::InvalidVersion(_) => "invalid_version",
        GameError::SaveValidation(_) => "save_validation",
        GameError::RevisionConflict { .. } => "revision_conflict",
        GameError::Lookup(_) => "lookup_error",
        GameError::Script(_) => "script_error",
    }
}

fn is_recoverable(error: &GameError) -> bool {
    matches!(
        error,
        GameError::RevisionConflict { .. }
            | GameError::Lookup(_)
            | GameError::SaveValidation(_)
            | GameError::DriverNotFound { .. }
    )
}

fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
