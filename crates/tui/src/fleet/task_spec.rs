//! Typed task-spec loading, artifact refs, deterministic scorers, and receipts.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use codewhale_protocol::fleet::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ledger::FleetLedger;

const MAX_SCORER_READ_BYTES: u64 = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetTaskSpecDocument {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_policy: Option<FleetSecurityPolicy>,
    #[serde(default, alias = "worker_specs")]
    pub workers: Vec<FleetWorkerSpec>,
    #[serde(default)]
    pub tasks: Vec<FleetTaskSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum FleetTaskSpecFile {
    Document(FleetTaskSpecDocument),
    Tasks(Vec<FleetTaskSpec>),
    Single(Box<FleetTaskSpec>),
}

impl FleetTaskSpecFile {
    fn into_document(self, fallback_name: String) -> FleetTaskSpecDocument {
        match self {
            Self::Document(mut doc) => {
                if doc.name.as_deref().is_none_or(str::is_empty) {
                    doc.name = Some(fallback_name);
                }
                doc
            }
            Self::Tasks(tasks) => FleetTaskSpecDocument {
                name: Some(fallback_name),
                labels: BTreeMap::new(),
                security_policy: None,
                workers: Vec::new(),
                tasks,
            },
            Self::Single(task) => FleetTaskSpecDocument {
                name: Some(fallback_name),
                labels: BTreeMap::new(),
                security_policy: None,
                workers: Vec::new(),
                tasks: vec![*task],
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct FleetTaskVerificationInput {
    pub run_id: FleetRunId,
    pub task_id: String,
    pub worker_id: String,
    /// Durable lease generation whose result is being verified.
    pub attempt: u32,
    pub exit_code: Option<i32>,
    pub artifacts: Vec<FleetArtifactRef>,
    /// Resolved-route snapshot to persist on the receipt (#3154).
    pub resolved_route: Option<FleetResolvedRoute>,
    /// Effective worker authority snapshot to persist on the receipt (#3211).
    pub effective_permissions: Option<FleetEffectivePermissions>,
}

#[derive(Debug, Clone)]
pub struct FleetTaskVerification {
    pub result: FleetTaskResult,
    pub failure_kind: Option<FleetTaskFailureKind>,
    pub score: FleetScore,
    pub evidence: Vec<String>,
}

pub fn load_task_spec_document(path: &Path) -> Result<FleetTaskSpecDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading fleet task spec {}", path.display()))?;
    let fallback_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("fleet-run")
        .to_string();
    let parsed = match path.extension().and_then(|s| s.to_str()) {
        Some("toml") => toml::from_str::<FleetTaskSpecFile>(&raw)
            .with_context(|| format!("parsing TOML fleet task spec {}", path.display()))?,
        _ => serde_json::from_str::<FleetTaskSpecFile>(&raw)
            .with_context(|| format!("parsing JSON fleet task spec {}", path.display()))?,
    };
    let doc = parsed.into_document(fallback_name);
    validate_task_spec_document(&doc)?;
    Ok(doc)
}

pub fn validate_task_spec_document(doc: &FleetTaskSpecDocument) -> Result<()> {
    if doc.tasks.is_empty() {
        bail!("fleet task spec must include at least one task");
    }
    let mut ids = BTreeSet::new();
    for task in &doc.tasks {
        if task.id.trim().is_empty() {
            bail!("fleet task id cannot be empty");
        }
        if !ids.insert(task.id.clone()) {
            bail!("duplicate fleet task id {}", task.id);
        }
        if task.name.trim().is_empty() {
            bail!("fleet task {} name cannot be empty", task.id);
        }
        if task.instructions.trim().is_empty() {
            bail!("fleet task {} instructions cannot be empty", task.id);
        }
        if let Some(objective) = &task.objective
            && objective.trim().is_empty()
        {
            bail!("fleet task {} objective cannot be empty", task.id);
        }
        validate_worker_profile(&task.id, task.worker.as_ref())?;
        validate_tags(&task.id, &task.tags)?;
        validate_workspace_requirements(task)?;
    }
    Ok(())
}

fn validate_worker_profile(task_id: &str, worker: Option<&FleetTaskWorkerProfile>) -> Result<()> {
    let Some(worker) = worker else {
        return Ok(());
    };
    validate_worker_token(
        task_id,
        "worker.agent_profile",
        worker.agent_profile.as_deref(),
    )?;
    validate_worker_token(task_id, "worker.loadout", worker.loadout.as_deref())?;
    validate_worker_token(task_id, "worker.model_class", worker.model_class.as_deref())?;
    validate_worker_model(task_id, worker.model.as_deref())?;
    Ok(())
}

fn validate_worker_token(task_id: &str, field: &str, value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("fleet task {task_id} {field} cannot be empty");
    }
    if trimmed != value || !trimmed.chars().all(is_worker_token_char) {
        bail!(
            "fleet task {task_id} {field} must be a simple token, not a path or provider/model id"
        );
    }
    Ok(())
}

fn is_worker_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

fn validate_worker_model(task_id: &str, value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("fleet task {task_id} worker.model cannot be empty");
    }
    if trimmed != value
        || !trimmed
            .chars()
            .all(|ch| ch.is_ascii_graphic() && !matches!(ch, '=' | '\'' | '"'))
    {
        bail!(
            "fleet task {task_id} worker.model must be a visible model id without whitespace or secrets"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn write_fleet_artifact_ref(
    workspace: &Path,
    run_id: &FleetRunId,
    task_id: &str,
    worker_id: &str,
    kind: FleetArtifactKind,
    filename: &str,
    contents: &[u8],
    mime_type: Option<&str>,
) -> Result<FleetArtifactRef> {
    let rel_path = PathBuf::from(".codewhale")
        .join("fleet")
        .join(safe_path_segment(&run_id.0))
        .join(safe_path_segment(task_id))
        .join(safe_path_segment(worker_id))
        .join(safe_path_segment(filename));
    let abs_path = workspace.join(&rel_path);
    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating fleet artifact dir {}", parent.display()))?;
    }
    std::fs::write(&abs_path, contents)
        .with_context(|| format!("writing fleet artifact {}", abs_path.display()))?;
    Ok(FleetArtifactRef {
        kind,
        path: rel_path,
        checksum: Some(format!("sha256:{}", crate::hashing::sha256_hex(contents))),
        mime_type: mime_type.map(str::to_string),
        size_bytes: Some(contents.len() as u64),
    })
}

pub fn verify_task_result(
    workspace: &Path,
    task: &FleetTaskSpec,
    input: &FleetTaskVerificationInput,
) -> FleetTaskVerification {
    match &task.scorer {
        Some(FleetScorerSpec::ExitCode) => verify_exit_code(input.exit_code),
        Some(FleetScorerSpec::FileExists { path }) => verify_file_exists(workspace, path),
        Some(FleetScorerSpec::RegexMatch { path, pattern }) => {
            verify_regex_match(workspace, path, pattern)
        }
        Some(FleetScorerSpec::JsonPath { path, expression }) => {
            verify_json_path(workspace, path, expression)
        }
        Some(FleetScorerSpec::Command { command, .. }) => partial(
            format!("external scorer command configured: {command}"),
            "run the configured scorer command to finalize this receipt",
        ),
        Some(FleetScorerSpec::CodeWhaleVerifierPrompt { .. }) => partial(
            "Codewhale verifier prompt configured",
            "run a verifier prompt pass to finalize this receipt",
        ),
        Some(FleetScorerSpec::Manual) => partial(
            "manual scorer configured",
            "manual verification is required to finalize this receipt",
        ),
        None if !has_verifiable_artifact(input) => partial(
            "no scorer configured and no verifiable artifacts recorded",
            "worker exited successfully but produced no verifiable output",
        ),
        None => partial(
            "no scorer configured",
            "task has artifacts but no deterministic scorer",
        ),
    }
}

pub fn prepare_verification_receipt(
    workspace: &Path,
    input: &FleetTaskVerificationInput,
    verification: FleetTaskVerification,
) -> Result<FleetReceipt> {
    let evidence = json!({
        "run_id": input.run_id.0.clone(),
        "task_id": input.task_id.clone(),
        "worker_id": input.worker_id.clone(),
        "attempt": input.attempt,
        "result": verification.result.clone(),
        "failure_kind": verification.failure_kind.clone(),
        "score": verification.score.clone(),
        "evidence": verification.evidence.clone(),
        "artifacts": input.artifacts.clone(),
    });
    let bytes =
        serde_json::to_vec_pretty(&evidence).context("serializing fleet receipt evidence")?;
    // Content-address the evidence as well as namespacing it by attempt. A
    // stale verifier may finish after a retry has started; it is allowed to
    // leave an orphaned evidence file, but it must never overwrite the file a
    // winning attempt's durable receipt references.
    let evidence_hash = crate::hashing::sha256_hex(&bytes);
    let filename = format!(
        "verification-receipt-attempt-{:010}-{}.json",
        input.attempt, evidence_hash
    );
    let receipt_artifact = write_fleet_artifact_ref(
        workspace,
        &input.run_id,
        &input.task_id,
        &input.worker_id,
        FleetArtifactKind::Receipt,
        &filename,
        &bytes,
        Some("application/json"),
    )?;
    let mut artifacts = input.artifacts.clone();
    artifacts.push(receipt_artifact);
    let receipt = FleetReceipt {
        run_id: input.run_id.clone(),
        task_id: input.task_id.clone(),
        worker_id: input.worker_id.clone(),
        attempt: Some(input.attempt),
        terminal_seq: None,
        completed_at: timestamp(),
        result: verification.result,
        failure_kind: verification.failure_kind,
        artifacts,
        score: Some(verification.score),
        resolved_route: input.resolved_route.clone(),
        effective_permissions: input.effective_permissions.clone(),
    };
    Ok(receipt)
}

pub fn record_verification_receipt(
    ledger: &FleetLedger,
    workspace: &Path,
    input: &FleetTaskVerificationInput,
    verification: FleetTaskVerification,
) -> Result<FleetReceipt> {
    let receipt = prepare_verification_receipt(workspace, input, verification)?;
    ledger.record_receipt(receipt.clone())?;
    Ok(receipt)
}

fn validate_tags(task_id: &str, tags: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for tag in tags {
        if tag.trim().is_empty() {
            bail!("fleet task {task_id} tag cannot be empty");
        }
        if !seen.insert(tag) {
            bail!("fleet task {task_id} has duplicate tag {tag}");
        }
    }
    Ok(())
}

fn validate_workspace_requirements(task: &FleetTaskSpec) -> Result<()> {
    let Some(workspace) = &task.workspace else {
        return Ok(());
    };
    let env = workspace.environment.as_ref();
    for name in env
        .into_iter()
        .flat_map(|env| env.required.iter().chain(env.allowlist.iter()))
    {
        if name.trim().is_empty() {
            bail!(
                "fleet task {} environment variable name cannot be empty",
                task.id
            );
        }
    }
    Ok(())
}

fn verify_exit_code(exit_code: Option<i32>) -> FleetTaskVerification {
    match exit_code {
        Some(0) => pass("exit_code=0"),
        Some(code) => fail(
            FleetTaskFailureKind::Task,
            0.0,
            format!("exit_code={code}"),
            "worker task exited unsuccessfully",
        ),
        None => fail(
            FleetTaskFailureKind::Transport,
            0.0,
            "missing exit code",
            "worker transport did not report a process result",
        ),
    }
}

fn verify_file_exists(workspace: &Path, path: &Path) -> FleetTaskVerification {
    let abs_path = resolve_workspace_path(workspace, path);
    if abs_path.is_file() {
        pass(format!("file exists: {}", path.display()))
    } else {
        fail(
            FleetTaskFailureKind::Task,
            0.0,
            format!("missing file: {}", path.display()),
            "expected artifact file was not produced",
        )
    }
}

fn verify_regex_match(workspace: &Path, path: &Path, pattern: &str) -> FleetTaskVerification {
    let regex = match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(err) => {
            return fail(
                FleetTaskFailureKind::Verifier,
                0.0,
                format!("invalid regex: {err}"),
                "regex scorer could not be compiled",
            );
        }
    };
    let contents = match read_bounded_to_string(workspace, path) {
        Ok(contents) => contents,
        Err(err) => {
            return fail(
                err.failure_kind,
                0.0,
                err.evidence,
                "regex scorer could not read bounded evidence",
            );
        }
    };
    if regex.is_match(&contents) {
        pass(format!("regex matched {}: {pattern}", path.display()))
    } else {
        fail(
            FleetTaskFailureKind::Task,
            0.0,
            format!("regex did not match {}: {pattern}", path.display()),
            "worker output did not satisfy the regex scorer",
        )
    }
}

fn verify_json_path(workspace: &Path, path: &Path, expression: &str) -> FleetTaskVerification {
    let Some(segments) = json_path_segments(expression) else {
        return fail(
            FleetTaskFailureKind::Verifier,
            0.0,
            format!("unsupported JSON path expression: {expression}"),
            "json_path scorer supports $.field or .field paths",
        );
    };
    let contents = match read_bounded_to_string(workspace, path) {
        Ok(contents) => contents,
        Err(err) => {
            return fail(
                err.failure_kind,
                0.0,
                err.evidence,
                "json_path scorer could not read bounded evidence",
            );
        }
    };
    let value: Value = match serde_json::from_str(&contents) {
        Ok(value) => value,
        Err(err) => {
            return fail(
                FleetTaskFailureKind::Task,
                0.0,
                format!("invalid JSON in {}: {err}", path.display()),
                "worker artifact was not valid JSON",
            );
        }
    };
    match json_path_lookup(&value, &segments) {
        Some(found) if json_truthy(found) => pass(format!(
            "json_path matched {}: {expression}",
            path.display()
        )),
        _ => fail(
            FleetTaskFailureKind::Task,
            0.0,
            format!(
                "json_path missing or false in {}: {expression}",
                path.display()
            ),
            "worker JSON artifact did not satisfy the scorer",
        ),
    }
}

fn pass(evidence: impl Into<String>) -> FleetTaskVerification {
    let evidence = evidence.into();
    FleetTaskVerification {
        result: FleetTaskResult::Pass,
        failure_kind: None,
        score: FleetScore {
            value: 1.0,
            max: Some(1.0),
            notes: Some(evidence.clone()),
        },
        evidence: vec![evidence],
    }
}

fn partial(evidence: impl Into<String>, notes: impl Into<String>) -> FleetTaskVerification {
    let evidence = evidence.into();
    let notes = notes.into();
    FleetTaskVerification {
        result: FleetTaskResult::Partial,
        failure_kind: None,
        score: FleetScore {
            value: 0.5,
            max: Some(1.0),
            notes: Some(notes),
        },
        evidence: vec![evidence],
    }
}

fn fail(
    failure_kind: FleetTaskFailureKind,
    value: f64,
    evidence: impl Into<String>,
    notes: impl Into<String>,
) -> FleetTaskVerification {
    let evidence = evidence.into();
    FleetTaskVerification {
        result: FleetTaskResult::Fail,
        failure_kind: Some(failure_kind),
        score: FleetScore {
            value,
            max: Some(1.0),
            notes: Some(notes.into()),
        },
        evidence: vec![evidence],
    }
}

fn has_verifiable_artifact(input: &FleetTaskVerificationInput) -> bool {
    input.artifacts.iter().any(|artifact| {
        !matches!(
            artifact.kind,
            FleetArtifactKind::Log | FleetArtifactKind::Receipt
        )
    })
}

#[derive(Debug)]
struct EvidenceReadError {
    failure_kind: FleetTaskFailureKind,
    evidence: String,
}

fn read_bounded_to_string(
    workspace: &Path,
    path: &Path,
) -> std::result::Result<String, EvidenceReadError> {
    let abs_path = resolve_workspace_path(workspace, path);
    let metadata = std::fs::metadata(&abs_path).map_err(|err| EvidenceReadError {
        failure_kind: if err.kind() == std::io::ErrorKind::NotFound {
            FleetTaskFailureKind::Task
        } else {
            FleetTaskFailureKind::Verifier
        },
        evidence: format!("cannot read {}: {err}", path.display()),
    })?;
    if metadata.len() > MAX_SCORER_READ_BYTES {
        return Err(EvidenceReadError {
            failure_kind: FleetTaskFailureKind::Verifier,
            evidence: format!(
                "refusing to read oversized evidence {}: {} bytes",
                path.display(),
                metadata.len()
            ),
        });
    }
    std::fs::read_to_string(&abs_path).map_err(|err| EvidenceReadError {
        failure_kind: FleetTaskFailureKind::Verifier,
        evidence: format!("cannot decode {} as UTF-8: {err}", path.display()),
    })
}

fn resolve_workspace_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn json_path_segments(expression: &str) -> Option<Vec<&str>> {
    let trimmed = expression.trim();
    let path = trimmed
        .strip_prefix("$.")
        .or_else(|| trimmed.strip_prefix('.'))?;
    if path.is_empty() {
        return None;
    }
    let segments: Vec<_> = path.split('.').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    Some(segments)
}

fn json_path_lookup<'a>(value: &'a Value, segments: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in segments {
        current = current.as_object()?.get(*segment)?;
    }
    Some(current)
}

fn json_truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn task(id: &str, scorer: Option<FleetScorerSpec>) -> FleetTaskSpec {
        FleetTaskSpec {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            objective: Some(format!("Verify {id}")),
            instructions: format!("do {id}"),
            worker: Some(FleetTaskWorkerProfile {
                agent_profile: None,
                role: Some("reviewer".to_string()),
                loadout: None,
                model_class: None,
                model: None,
                tool_profile: Some("read-only".to_string()),
                tools: vec!["git".to_string()],
                capabilities: vec!["rust".to_string()],
            }),
            workspace: Some(FleetWorkspaceRequirements {
                root: Some(PathBuf::from(".")),
                required_files: vec![PathBuf::from("Cargo.toml")],
                writable_paths: vec![PathBuf::from(".codewhale/fleet")],
                environment: Some(FleetEnvironmentRequirements {
                    required: vec!["PATH".to_string()],
                    allowlist: vec!["RUST_LOG".to_string()],
                }),
            }),
            input_files: vec![PathBuf::from("Cargo.toml")],
            context: vec!["fleet verifier test".to_string()],
            budget: Some(FleetTaskBudget {
                max_tokens: Some(4000),
                max_tool_calls: Some(12),
                max_seconds: Some(120),
            }),
            expected_artifacts: vec![FleetArtifactKind::Log, FleetArtifactKind::Receipt],
            scorer,
            retry_policy: Some(FleetRetryPolicy::default()),
            alert_policy: None,
            timeout_seconds: Some(120),
            tags: vec!["review".to_string()],
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn fleet_task_spec_document_parses_multi_task_verified_shape() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("fleet-tasks.json");
        let doc = json!({
            "name": "release triage",
            "labels": {"milestone": "v0.8.60"},
            "tasks": [
                task("release-notes", Some(FleetScorerSpec::ExitCode)),
                task("risk-review", Some(FleetScorerSpec::Manual))
            ]
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        let parsed = load_task_spec_document(&path).unwrap();

        assert_eq!(parsed.name.as_deref(), Some("release triage"));
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(
            parsed.tasks[0].objective.as_deref(),
            Some("Verify release-notes")
        );
        assert_eq!(
            parsed.tasks[0].worker.as_ref().unwrap().role.as_deref(),
            Some("reviewer")
        );
        assert_eq!(parsed.tasks[1].tags, vec!["review"]);
    }

    #[test]
    fn fleet_task_spec_document_parses_worker_profile_loadout_intent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("fleet-profile-task.json");
        let doc = json!({
            "name": "profile loadout smoke",
            "tasks": [{
                "id": "review",
                "name": "review",
                "instructions": "review the patch",
                "worker": {
                    "profile": "adversarial_reviewer",
                    "role": "reviewer",
                    "loadout": "auto",
                    "model_class": "balanced",
                    "model": "deepseek-v4-pro",
                    "tool_profile": "read-only",
                    "tools": ["read_file", "grep_files"],
                    "capabilities": ["rust"]
                }
            }]
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        let parsed = load_task_spec_document(&path).unwrap();
        let worker = parsed.tasks[0].worker.as_ref().unwrap();

        assert_eq!(
            worker.agent_profile.as_deref(),
            Some("adversarial_reviewer")
        );
        assert_eq!(worker.role.as_deref(), Some("reviewer"));
        assert_eq!(worker.loadout.as_deref(), Some("auto"));
        assert_eq!(worker.model_class.as_deref(), Some("balanced"));
        assert_eq!(worker.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(worker.tool_profile.as_deref(), Some("read-only"));
    }

    #[test]
    fn fleet_task_spec_rejects_unsafe_worker_profile_intent_tokens() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("unsafe-profile-task.json");
        let doc = json!({
            "tasks": [{
                "id": "review",
                "name": "review",
                "instructions": "review the patch",
                "worker": {
                    "profile": "../secrets",
                    "loadout": "openrouter/deepseek",
                    "model_class": "",
                    "model": "deepseek/deepseek-v4-pro"
                }
            }]
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        let err = load_task_spec_document(&path).unwrap_err().to_string();

        assert!(
            err.contains("worker.agent_profile must be a simple token"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn fleet_task_spec_rejects_secret_like_worker_model() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("unsafe-worker-model.json");
        let doc = json!({
            "tasks": [{
                "id": "review",
                "name": "review",
                "instructions": "review the patch",
                "worker": {
                    "model": "deepseek-v4-pro api_key=secret"
                }
            }]
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        let err = load_task_spec_document(&path).unwrap_err().to_string();

        assert!(
            err.contains("worker.model must be a visible model id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn fleet_task_spec_artifact_refs_are_bounded_paths() {
        let tmp = TempDir::new().unwrap();
        let artifact = write_fleet_artifact_ref(
            tmp.path(),
            &FleetRunId::from("run-1"),
            "task-a",
            "worker-1",
            FleetArtifactKind::Log,
            "worker.log",
            b"this is artifact content",
            Some("text/plain"),
        )
        .unwrap();

        let json = serde_json::to_string(&artifact).unwrap();
        assert!(!json.contains("this is artifact content"));
        assert!(json.contains("worker.log"));
        assert_eq!(artifact.size_bytes, Some(24));
        assert!(artifact.checksum.as_deref().unwrap().starts_with("sha256:"));
        assert!(tmp.path().join(&artifact.path).exists());
    }

    #[test]
    fn fleet_task_spec_scorers_record_pass_fail_partial_evidence() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("result.txt"), "status=ok\n").unwrap();
        std::fs::write(tmp.path().join("result.json"), r#"{"status":"ok"}"#).unwrap();
        let input = FleetTaskVerificationInput {
            run_id: FleetRunId::from("run-1"),
            task_id: "task-a".to_string(),
            worker_id: "worker-1".to_string(),
            attempt: 1,
            exit_code: Some(0),
            artifacts: vec![],
            resolved_route: None,
            effective_permissions: None,
        };

        let pass = verify_task_result(
            tmp.path(),
            &task("exit", Some(FleetScorerSpec::ExitCode)),
            &input,
        );
        assert_eq!(pass.result, FleetTaskResult::Pass);
        assert_eq!(pass.failure_kind, None);

        let regex = verify_task_result(
            tmp.path(),
            &task(
                "regex",
                Some(FleetScorerSpec::RegexMatch {
                    path: PathBuf::from("result.txt"),
                    pattern: "status=ok".to_string(),
                }),
            ),
            &input,
        );
        assert_eq!(regex.result, FleetTaskResult::Pass);

        let json_path = verify_task_result(
            tmp.path(),
            &task(
                "json",
                Some(FleetScorerSpec::JsonPath {
                    path: PathBuf::from("result.json"),
                    expression: "$.status".to_string(),
                }),
            ),
            &input,
        );
        assert_eq!(json_path.result, FleetTaskResult::Pass);

        let manual = verify_task_result(
            tmp.path(),
            &task("manual", Some(FleetScorerSpec::Manual)),
            &input,
        );
        assert_eq!(manual.result, FleetTaskResult::Partial);

        let no_scorer_empty = verify_task_result(tmp.path(), &task("unscored", None), &input);
        assert_eq!(no_scorer_empty.result, FleetTaskResult::Partial);
        assert!(
            no_scorer_empty
                .score
                .notes
                .as_deref()
                .unwrap_or_default()
                .contains("no verifiable output")
        );

        let failed = verify_task_result(
            tmp.path(),
            &task(
                "missing",
                Some(FleetScorerSpec::FileExists {
                    path: PathBuf::from("missing.txt"),
                }),
            ),
            &input,
        );
        assert_eq!(failed.result, FleetTaskResult::Fail);
        assert_eq!(failed.failure_kind, Some(FleetTaskFailureKind::Task));

        let verifier_failed = verify_task_result(
            tmp.path(),
            &task(
                "bad-regex",
                Some(FleetScorerSpec::RegexMatch {
                    path: PathBuf::from("result.txt"),
                    pattern: "[".to_string(),
                }),
            ),
            &input,
        );
        assert_eq!(verifier_failed.result, FleetTaskResult::Fail);
        assert_eq!(
            verifier_failed.failure_kind,
            Some(FleetTaskFailureKind::Verifier)
        );
    }

    #[test]
    fn fleet_task_spec_receipt_records_artifacts_scores_and_failure_kind() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let log = write_fleet_artifact_ref(
            tmp.path(),
            &FleetRunId::from("run-1"),
            "task-a",
            "worker-1",
            FleetArtifactKind::Log,
            "worker.log",
            b"exit_code=1",
            Some("text/plain"),
        )
        .unwrap();
        let input = FleetTaskVerificationInput {
            run_id: FleetRunId::from("run-1"),
            task_id: "task-a".to_string(),
            worker_id: "worker-1".to_string(),
            attempt: 3,
            exit_code: Some(1),
            artifacts: vec![log],
            resolved_route: None,
            effective_permissions: Some(FleetEffectivePermissions {
                write: false,
                network: false,
                shell: "read_only".to_string(),
                tool_scope: "explicit".to_string(),
                tools: vec!["read_file".to_string()],
                background: true,
                max_spawn_depth: 0,
                profile_id: None,
                profile_origin: None,
                source: "worker_runtime_profile".to_string(),
            }),
        };
        let verification = verify_task_result(
            tmp.path(),
            &task("task-a", Some(FleetScorerSpec::ExitCode)),
            &input,
        );

        let receipt =
            record_verification_receipt(&ledger, tmp.path(), &input, verification).unwrap();

        assert_eq!(receipt.result, FleetTaskResult::Fail);
        assert_eq!(receipt.failure_kind, Some(FleetTaskFailureKind::Task));
        assert_eq!(receipt.attempt, Some(3));
        assert_eq!(receipt.terminal_seq, None);
        assert_eq!(receipt.effective_permissions, input.effective_permissions);
        assert_eq!(receipt.artifacts.len(), 2);
        assert!(matches!(
            receipt.artifacts.last().unwrap().kind,
            FleetArtifactKind::Receipt
        ));
        assert!(
            receipt
                .artifacts
                .last()
                .unwrap()
                .path
                .to_string_lossy()
                .contains("verification-receipt-attempt-0000000003-")
        );
        let state = ledger.rebuild_state().unwrap();
        assert_eq!(
            state.receipts["run-1:task-a"].failure_kind,
            Some(FleetTaskFailureKind::Task)
        );
    }

    #[test]
    fn verification_evidence_is_attempt_and_content_addressed() {
        let tmp = TempDir::new().unwrap();
        let mut input = FleetTaskVerificationInput {
            run_id: FleetRunId::from("run-1"),
            task_id: "task-a".to_string(),
            worker_id: "worker-1".to_string(),
            attempt: 1,
            exit_code: Some(1),
            artifacts: Vec::new(),
            resolved_route: None,
            effective_permissions: None,
        };
        let scorer = task("task-a", Some(FleetScorerSpec::ExitCode));
        let stale_verification = verify_task_result(tmp.path(), &scorer, &input);
        let stale = prepare_verification_receipt(tmp.path(), &input, stale_verification).unwrap();

        input.attempt = 2;
        input.exit_code = Some(0);
        let winning_verification = verify_task_result(tmp.path(), &scorer, &input);
        let winning =
            prepare_verification_receipt(tmp.path(), &input, winning_verification).unwrap();

        let stale_path = &stale.artifacts.last().unwrap().path;
        let winning_path = &winning.artifacts.last().unwrap().path;
        assert_ne!(stale_path, winning_path);
        assert!(stale_path.to_string_lossy().contains("attempt-0000000001-"));
        assert!(
            winning_path
                .to_string_lossy()
                .contains("attempt-0000000002-")
        );
        assert!(tmp.path().join(stale_path).is_file());
        assert!(tmp.path().join(winning_path).is_file());
        assert_eq!(stale.result, FleetTaskResult::Fail);
        assert_eq!(winning.result, FleetTaskResult::Pass);
    }
}
