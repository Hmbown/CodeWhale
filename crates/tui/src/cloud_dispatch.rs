//! Daytona cloud-agent dispatch contract.
//!
//! Local `cw` / Codewhale may propose sending a coding agent to Daytona so
//! heavy work can raise a branch and open a PR while the TUI stays responsive.
//! This module is the only first-class offload seam:
//!
//! - remotes are explicit forges: `github`, `cnb`, `gitee`
//! - CWC's convention is preserved: a remote *named* `github` is authoritative
//!   GitHub; `origin` is classified by URL and is often the CNB mirror
//! - confirmation is required; nothing spends or pushes silently
//! - missing Daytona credentials fail closed; success is never faked
//!
//! Watch/cancel of a live sandbox and auto-decide heuristics are leftover.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use codewhale_paths::codewhale_home;
use codewhale_secrets::Secrets;
use serde::{Deserialize, Serialize};

const MAX_PROMPT_CHARS: usize = 4_000;
const MAX_REMOTE_BYTES: usize = 4 * 1024;
const JOB_KIND: &str = "cloud";
const DAYTONA_API_KEY_ENV: &str = "DAYTONA_API_KEY";
const DAYTONA_API_URL_ENV: &str = "DAYTONA_API_URL";
const CWC_DAYTONA_TOKEN_ENV: &str = "CWC_DAYTONA_TOKEN";
const CWC_DAYTONA_ENDPOINT_ENV: &str = "CWC_DAYTONA_ENDPOINT";
const KEYRING_SLOT: &str = "daytona";
const DEFAULT_DAYTONA_API: &str = "https://app.daytona.io/api";

/// Explicit PR forge. Never inferred from a generic "origin means GitHub" rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    Github,
    Cnb,
    Gitee,
}

impl Forge {
    /// Stable CLI / TUI slug.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Cnb => "cnb",
            Self::Gitee => "gitee",
        }
    }

    /// Parse a user-supplied forge slug.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "github" | "gh" => Some(Self::Github),
            "cnb" => Some(Self::Cnb),
            "gitee" => Some(Self::Gitee),
            _ => None,
        }
    }
}

/// One `git remote` row after fetch/push duplicates are collapsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

/// A remote that has been classified as a supported forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedRemote {
    pub forge: Forge,
    pub name: String,
    pub url: String,
}

/// Where a Daytona API key was found. Never carries the secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Env,
    CwcEnv,
    Keyring,
}

/// Presence of Daytona credentials. Absence is fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialState {
    Missing,
    Present { source: CredentialSource },
}

/// First-class cloud job lifecycle. `kind` is always `cloud`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudJobStatus {
    Proposed,
    Refused,
    Launching,
    Running,
    Failed,
    Canceled,
}

/// Durable cloud job record, listed on the same `/jobs` surface as Bash jobs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudJob {
    pub id: String,
    pub kind: String,
    pub status: CloudJobStatus,
    pub prompt: String,
    pub forge: Forge,
    pub remote_name: String,
    pub remote_url: String,
    pub branch: String,
    pub confirmed: bool,
    pub sandbox_id: Option<String>,
    pub pr_url: Option<String>,
    pub refusal: Option<String>,
    pub note: String,
    pub created_unix: u64,
}

/// Validated plan that still requires an explicit confirm to spend or push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchPlan {
    pub prompt: String,
    pub remote: SelectedRemote,
    pub branch: String,
}

/// Result of proposing or confirming a dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    Proposal(CloudJob),
    Refused(CloudJob),
    Accepted(CloudJob),
}

/// Why a plan or launch cannot proceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    EmptyPrompt,
    PromptTooLong,
    InvalidBranch,
    UnknownForge,
    AmbiguousRemote,
    RemoteMissing(Forge),
    NoSupportedRemote,
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPrompt => write!(f, "A cloud dispatch needs a non-empty task prompt."),
            Self::PromptTooLong => write!(
                f,
                "A cloud dispatch prompt must be at most {MAX_PROMPT_CHARS} characters."
            ),
            Self::InvalidBranch => write!(
                f,
                "The requested branch is not a safe git ref (no leading '-', no '..', no shell metacharacters)."
            ),
            Self::UnknownForge => write!(
                f,
                "Remote must be one of github, cnb, or gitee. CWC treats the `github` remote as GitHub and `origin` as the CNB mirror when that URL is cnb.cool."
            ),
            Self::AmbiguousRemote => write!(
                f,
                "This workspace has more than one forge remote. Pass --remote github|cnb|gitee (CWC: `github` is GitHub, `origin` is often CNB)."
            ),
            Self::RemoteMissing(forge) => write!(
                f,
                "No {0} remote is configured. Add a `{0}` remote or a URL on {1}.",
                forge.as_str(),
                forge_host(*forge)
            ),
            Self::NoSupportedRemote => write!(
                f,
                "No GitHub, CNB, or Gitee remote was found. Remotes are classified by name (`github`/`cnb`/`gitee`) then by host."
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

/// File-backed store under `$CODEWHALE_HOME/cloud-jobs`.
#[derive(Debug, Clone)]
pub struct CloudJobStore {
    root: PathBuf,
}

impl CloudJobStore {
    /// Resolve the process Codewhale home.
    pub fn from_env() -> Result<Self> {
        let home = codewhale_home()
            .map_err(|err| anyhow!(err.to_string()))?
            .ok_or_else(|| anyhow!("CODEWHALE_HOME / user home is unavailable"))?;
        Ok(Self::from_path(home.join("cloud-jobs")))
    }

    /// Test and injected-root constructor.
    pub fn from_path(root: PathBuf) -> Self {
        Self { root }
    }

    /// Persist a job atomically. Never writes credentials.
    pub fn save(&self, job: &CloudJob) -> Result<()> {
        fs::create_dir_all(&self.root).context("failed to create cloud-jobs directory")?;
        let path = self.job_path(&job.id)?;
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(job).context("failed to encode cloud job")?;
        {
            let mut file =
                fs::File::create(&tmp).context("failed to start a private cloud job write")?;
            file.write_all(&body)
                .context("failed to write the cloud job record")?;
            file.sync_all().ok();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(&tmp, &path).context("failed to commit the cloud job record")?;
        Ok(())
    }

    /// Load one job by id.
    pub fn load(&self, id: &str) -> Result<CloudJob> {
        let path = self.job_path(id)?;
        let body = fs::read(&path).with_context(|| format!("cloud job {id} was not found"))?;
        serde_json::from_slice(&body).context("cloud job record is invalid JSON")
    }

    /// Newest-first listing.
    pub fn list(&self) -> Result<Vec<CloudJob>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&self.root).context("failed to read cloud-jobs")? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("cloud_") || !name.ends_with(".json") {
                continue;
            }
            let body = fs::read(entry.path())?;
            if let Ok(job) = serde_json::from_slice::<CloudJob>(&body) {
                jobs.push(job);
            }
        }
        jobs.sort_by_key(|a| std::cmp::Reverse(a.created_unix));
        Ok(jobs)
    }

    fn job_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_job_id(id) {
            bail!("cloud job id must look like cloud_<hex>");
        }
        Ok(self.root.join(format!("{id}.json")))
    }
}

/// Classify a git remote. Named `github` / `cnb` / `gitee` win over URL.
pub fn classify_remote(name: &str, url: &str) -> Option<Forge> {
    match name.trim().to_ascii_lowercase().as_str() {
        "github" => Some(Forge::Github),
        "cnb" => Some(Forge::Cnb),
        "gitee" => Some(Forge::Gitee),
        _ => classify_url(url),
    }
}

/// Classify a clone URL by host. `origin` uses this path.
pub fn classify_url(url: &str) -> Option<Forge> {
    let host = remote_host(url)?;
    match host.as_str() {
        "github.com" | "www.github.com" => Some(Forge::Github),
        "cnb.cool" | "www.cnb.cool" => Some(Forge::Cnb),
        "gitee.com" | "www.gitee.com" => Some(Forge::Gitee),
        _ => None,
    }
}

/// Parse `git remote -v` text into unique remotes (fetch URL preferred).
pub fn parse_remote_listing(text: &str) -> Vec<GitRemote> {
    let mut remotes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.len() > MAX_REMOTE_BYTES {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(url) = parts.next() else { continue };
        if name.is_empty() || url.is_empty() || name.chars().any(char::is_control) {
            continue;
        }
        if remotes.iter().any(|remote: &GitRemote| remote.name == name) {
            continue;
        }
        remotes.push(GitRemote {
            name: name.to_string(),
            url: url.to_string(),
        });
    }
    remotes
}

/// Read remotes from a workspace. Missing git is an empty list, not a panic.
pub fn discover_remotes(workspace: &Path) -> Vec<GitRemote> {
    let output = Command::new("git")
        .args(["-C", &workspace.to_string_lossy(), "remote", "-v"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse_remote_listing(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// Choose the forge remote. Multiple forges require an explicit request.
pub fn select_remote(
    remotes: &[GitRemote],
    requested: Option<Forge>,
) -> Result<SelectedRemote, DispatchError> {
    let classified: Vec<SelectedRemote> = remotes
        .iter()
        .filter_map(|remote| {
            classify_remote(&remote.name, &remote.url).map(|forge| SelectedRemote {
                forge,
                name: remote.name.clone(),
                url: remote.url.clone(),
            })
        })
        .collect();

    if let Some(forge) = requested {
        return classified
            .into_iter()
            .find(|remote| remote.forge == forge)
            .or_else(|| prefer_named(remotes, forge))
            .ok_or(DispatchError::RemoteMissing(forge));
    }

    let mut unique = Vec::new();
    for remote in classified {
        if !unique
            .iter()
            .any(|seen: &SelectedRemote| seen.forge == remote.forge)
        {
            unique.push(remote);
        }
    }
    match unique.len() {
        0 => Err(DispatchError::NoSupportedRemote),
        1 => Ok(unique.remove(0)),
        _ => Err(DispatchError::AmbiguousRemote),
    }
}

/// Build a plan. Does not spend, push, or write a job.
pub fn plan_dispatch(
    remotes: &[GitRemote],
    prompt: &str,
    requested: Option<Forge>,
    branch: Option<&str>,
) -> Result<DispatchPlan, DispatchError> {
    let prompt = validate_prompt(prompt)?;
    let remote = select_remote(remotes, requested)?;
    let branch = match branch.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            if !valid_branch(value) {
                return Err(DispatchError::InvalidBranch);
            }
            value.to_string()
        }
        None => default_branch(),
    };
    Ok(DispatchPlan {
        prompt,
        remote,
        branch,
    })
}

/// Discover Daytona credentials without returning or logging the secret.
pub fn discover_credentials() -> CredentialState {
    if env_present(DAYTONA_API_KEY_ENV) {
        return CredentialState::Present {
            source: CredentialSource::Env,
        };
    }
    if env_present(CWC_DAYTONA_TOKEN_ENV) {
        return CredentialState::Present {
            source: CredentialSource::CwcEnv,
        };
    }
    match Secrets::auto_detect().get(KEYRING_SLOT) {
        Ok(Some(value)) if !value.trim().is_empty() => CredentialState::Present {
            source: CredentialSource::Keyring,
        },
        _ => CredentialState::Missing,
    }
}

/// Codewhale membership check. `/dispatch` cloud agents ship with the
/// account, so the fail-closed gate is sign-in — never a provider key.
fn membership_signed_in() -> bool {
    let Ok(secrets) = codewhale_secrets::account::secure_account_session_secrets() else {
        return false;
    };
    let store = codewhale_secrets::account::AccountSessionStore::new(
        secrets,
        None,
        codewhale_secrets::account::DEFAULT_ACCOUNT_API_BASE,
    );
    matches!(store.load(), Ok(Some(_)))
}

/// Auto-decide leftover: Codewhale may propose, but never confirm itself.
pub fn should_auto_confirm(_plan: &DispatchPlan) -> bool {
    false
}

/// Propose or launch. Confirmation and credentials are enforced here.
pub fn execute_dispatch(
    store: &CloudJobStore,
    plan: DispatchPlan,
    confirm: bool,
    credentials: &CredentialState,
    launcher: &dyn DaytonaLauncher,
) -> Result<DispatchOutcome> {
    let mut job = CloudJob {
        id: allocate_job_id(&plan),
        kind: JOB_KIND.to_string(),
        status: CloudJobStatus::Proposed,
        prompt: plan.prompt.clone(),
        forge: plan.remote.forge,
        remote_name: plan.remote.name.clone(),
        remote_url: plan.remote.url.clone(),
        branch: plan.branch.clone(),
        confirmed: false,
        sandbox_id: None,
        pr_url: None,
        refusal: None,
        note: proposal_note(&plan),
        created_unix: unix_now(),
    };

    if !confirm {
        job.note = format!(
            "{}. Confirm with `codewhale dispatch --confirm {}` or `/dispatch confirm {}`. Never silent spend, never silent push.",
            job.note, job.id, job.id
        );
        store.save(&job)?;
        return Ok(DispatchOutcome::Proposal(job));
    }

    job.confirmed = true;
    if matches!(credentials, CredentialState::Missing) {
        job.status = CloudJobStatus::Refused;
        job.refusal = Some(missing_credentials_message());
        job.note = missing_credentials_message();
        store.save(&job)?;
        return Ok(DispatchOutcome::Refused(job));
    }

    job.status = CloudJobStatus::Launching;
    store.save(&job)?;
    match launcher.create_sandbox(&job) {
        Ok(receipt) => {
            job.sandbox_id = Some(receipt.sandbox_id);
            job.status = CloudJobStatus::Running;
            job.pr_url = None;
            job.note = "Cloud agent sandbox created. PR open is not claimed: this slice does not fake a forge pull request.".to_string();
            store.save(&job)?;
            Ok(DispatchOutcome::Accepted(job))
        }
        Err(error) => {
            job.status = CloudJobStatus::Failed;
            job.refusal = Some(sanitize_error(&error.to_string()));
            job.note = format!(
                "Cloud agent launch failed closed. {}",
                job.refusal.as_deref().unwrap_or("unknown error")
            );
            store.save(&job)?;
            Ok(DispatchOutcome::Refused(job))
        }
    }
}

/// Confirm a previously proposed job.
pub fn confirm_job(
    store: &CloudJobStore,
    id: &str,
    credentials: &CredentialState,
    launcher: &dyn DaytonaLauncher,
) -> Result<DispatchOutcome> {
    let job = store.load(id)?;
    if job.status != CloudJobStatus::Proposed {
        bail!(
            "Cloud job {} is {} and cannot be confirmed.",
            job.id,
            status_label(job.status)
        );
    }
    let plan = DispatchPlan {
        prompt: job.prompt,
        remote: SelectedRemote {
            forge: job.forge,
            name: job.remote_name,
            url: job.remote_url,
        },
        branch: job.branch,
    };
    execute_dispatch(store, plan, true, credentials, launcher)
}

/// Cancel a job. Live sandbox teardown is leftover when no sandbox id exists.
pub fn cancel_job(store: &CloudJobStore, id: &str) -> Result<CloudJob> {
    let mut job = store.load(id)?;
    if matches!(
        job.status,
        CloudJobStatus::Canceled | CloudJobStatus::Failed | CloudJobStatus::Refused
    ) {
        return Ok(job);
    }
    job.status = CloudJobStatus::Canceled;
    job.note =
        "Canceled locally. Live sandbox teardown is leftover when one was created.".to_string();
    store.save(&job)?;
    Ok(job)
}

/// Human list used by `/jobs` (cloud kind) and `codewhale dispatch --list`.
pub fn format_job_list(jobs: &[CloudJob]) -> String {
    if jobs.is_empty() {
        return "Cloud jobs (0)\nNo cloud-agent jobs yet. Use `codewhale dispatch <prompt>` or `/dispatch <prompt>`.".to_string();
    }
    let mut lines = vec![
        format!("Cloud jobs ({})  kind=cloud", jobs.len()),
        "----------------------------------------".to_string(),
    ];
    for job in jobs {
        lines.push(format!(
            "{}  {:9}  {}  {}  branch={}",
            job.id,
            status_label(job.status),
            job.forge.as_str(),
            job.remote_name,
            job.branch
        ));
        lines.push(format!("  prompt: {}", one_line(&job.prompt, 120)));
        if let Some(sandbox) = job.sandbox_id.as_ref() {
            lines.push(format!("  sandbox: {sandbox}"));
        }
        if let Some(pr) = job.pr_url.as_ref() {
            lines.push(format!("  pr: {pr}"));
        }
    }
    lines.push(
        "Controls: /dispatch show <id>, /dispatch confirm <id>, /dispatch cancel <id>, /jobs list."
            .to_string(),
    );
    lines.join("\n")
}

/// Job inspector used by `/dispatch show` and `/jobs show cloud_*`.
pub fn format_job(job: &CloudJob) -> String {
    let mut lines = vec![
        format!("Cloud job {}", job.id),
        format!("Kind: {}", job.kind),
        format!("Status: {}", status_label(job.status)),
        format!("Forge: {}", job.forge.as_str()),
        format!("Remote: {} {}", job.remote_name, job.remote_url),
        format!("Branch: {}", job.branch),
        format!("Confirmed: {}", job.confirmed),
        format!("Sandbox: {}", job.sandbox_id.as_deref().unwrap_or("(none)")),
        format!("PR: {}", job.pr_url.as_deref().unwrap_or("(not opened)")),
        format!("Prompt: {}", job.prompt),
        format!("Note: {}", job.note),
    ];
    if let Some(refusal) = job.refusal.as_ref() {
        lines.push(format!("Refusal: {refusal}"));
    }
    lines.join("\n")
}

/// Status card for bare `/dispatch` and `codewhale dispatch --status`.
pub fn format_status(remotes: &[GitRemote], credentials: &CredentialState) -> String {
    let mut lines = vec!["Codewhale cloud dispatch".to_string()];
    match credentials {
        CredentialState::Missing => {
            if membership_signed_in() {
                lines.push(
                    "Cloud agents are not available for this account yet; cloud dispatch fails closed (no sandbox, no push, no PR)."
                        .to_string(),
                );
            } else {
                lines.push(
                    "Cloud agents are included with your Codewhale membership. Sign in with `codewhale login` to enable `/dispatch`; cloud dispatch fails closed until then (no sandbox, no push, no PR)."
                        .to_string(),
                );
            }
        }
        CredentialState::Present { .. } => {
            lines.push(
                "Cloud agents: ready (account-linked). Confirmation is still required before spend or push."
                    .to_string(),
            );
        }
    }
    if remotes.is_empty() {
        lines.push("Remotes: none discovered.".to_string());
    } else {
        lines.push("Remotes:".to_string());
        for remote in remotes {
            let forge = classify_remote(&remote.name, &remote.url)
                .map(Forge::as_str)
                .unwrap_or("unsupported");
            lines.push(format!("  {}  {forge}  {}", remote.name, remote.url));
        }
    }
    lines.push(
        "Offload: `codewhale dispatch \"<prompt>\" --remote github|cnb|gitee` then `--confirm`, or `/dispatch <prompt>` then `/dispatch confirm <id>`."
            .to_string(),
    );
    lines.join("\n")
}

/// Resolve the API origin used by the live launcher. Never logs credentials.
pub fn daytona_api_url() -> String {
    std::env::var(DAYTONA_API_URL_ENV)
        .ok()
        .or_else(|| std::env::var(CWC_DAYTONA_ENDPOINT_ENV).ok())
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_DAYTONA_API.to_string())
}

/// Launch seam. Tests inject a recorder; production uses [`LiveDaytonaLauncher`].
pub trait DaytonaLauncher {
    fn create_sandbox(&self, job: &CloudJob) -> Result<SandboxReceipt>;
}

/// Provider receipt. `pr_url` is intentionally omitted from this slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxReceipt {
    pub sandbox_id: String,
}

/// Real Daytona HTTP create. Fails closed; never invents a PR URL.
pub struct LiveDaytonaLauncher;

impl DaytonaLauncher for LiveDaytonaLauncher {
    fn create_sandbox(&self, job: &CloudJob) -> Result<SandboxReceipt> {
        let api_key = read_api_key().ok_or_else(|| anyhow!(missing_credentials_message()))?;
        let base = daytona_api_url();
        if !base.starts_with("https://") && !base.starts_with("http://127.0.0.1") {
            bail!("Daytona API URL must be HTTPS (loopback HTTP is allowed for tests).");
        }
        let url = format!("{}/sandbox", base.trim_end_matches('/'));
        let body = serde_json::json!({
            "name": format!("cw-{}", job.id.replace('_', "-")),
            "labels": {
                "codewhale.job": job.id,
                "codewhale.forge": job.forge.as_str(),
                "codewhale.product": "dispatch",
            }
        });
        let response = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to initialize the Daytona client")?
            .post(&url)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .context("could not reach Daytona")?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            bail!("Cloud agent create failed (HTTP {status}).",);
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).context("Daytona returned invalid JSON")?;
        let sandbox_id = parsed
            .get("id")
            .or_else(|| parsed.get("sandboxId"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if sandbox_id.is_empty() || sandbox_id.len() > 128 {
            bail!("Daytona create succeeded but returned no sandbox id.");
        }
        let _ = text;
        Ok(SandboxReceipt { sandbox_id })
    }
}

/// Status / enablement copy shared by CLI and TUI fail-closed paths.
/// Membership-first: the gate is sign-in, never a provider key.
pub fn missing_credentials_message() -> String {
    if membership_signed_in() {
        "Cloud agents are not available for this account yet; cloud dispatch fails closed (no sandbox, no push, no PR).".to_string()
    } else {
        "Cloud agents are included with your Codewhale membership. Sign in with `codewhale login` to enable `/dispatch`; cloud dispatch fails closed until then (no sandbox, no push, no PR).".to_string()
    }
}

fn prefer_named(remotes: &[GitRemote], forge: Forge) -> Option<SelectedRemote> {
    remotes
        .iter()
        .find(|remote| remote.name.eq_ignore_ascii_case(forge.as_str()))
        .map(|remote| SelectedRemote {
            forge,
            name: remote.name.clone(),
            url: remote.url.clone(),
        })
}

fn validate_prompt(prompt: &str) -> Result<String, DispatchError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(DispatchError::EmptyPrompt);
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(DispatchError::PromptTooLong);
    }
    Ok(prompt.to_string())
}

fn valid_branch(branch: &str) -> bool {
    if branch.is_empty()
        || branch.len() > 255
        || branch.starts_with('-')
        || branch.starts_with('/')
        || branch.ends_with('/')
        || branch.ends_with('.')
        || branch.contains("..")
        || branch.contains("//")
        || branch.contains("@{")
    {
        return false;
    }
    branch
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
}

fn valid_job_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("cloud_") else {
        return false;
    };
    (8..=32).contains(&rest.len()) && rest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn allocate_job_id(plan: &DispatchPlan) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    plan.prompt.hash(&mut hasher);
    plan.remote.forge.as_str().hash(&mut hasher);
    plan.branch.hash(&mut hasher);
    unix_now().hash(&mut hasher);
    format!("cloud_{:016x}", hasher.finish())
}

fn default_branch() -> String {
    format!("codewhale/cloud-{}", unix_now())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn env_present(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
}

fn read_api_key() -> Option<String> {
    for name in [DAYTONA_API_KEY_ENV, CWC_DAYTONA_TOKEN_ENV] {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    Secrets::auto_detect()
        .get(KEYRING_SLOT)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn remote_host(url: &str) -> Option<String> {
    let url = url.trim();
    if url.len() > MAX_REMOTE_BYTES {
        return None;
    }
    let host = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
    {
        let authority = rest.split('/').next()?;
        authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host)
    } else if let Some((_, rest)) = url.split_once('@') {
        rest.split(':').next()?
    } else {
        return None;
    };
    let host = host.split(':').next()?.trim().to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

fn forge_host(forge: Forge) -> &'static str {
    match forge {
        Forge::Github => "github.com",
        Forge::Cnb => "cnb.cool",
        Forge::Gitee => "gitee.com",
    }
}

fn proposal_note(plan: &DispatchPlan) -> String {
    format!(
        "Proposed Daytona offload to {} ({}) raising branch {}.",
        plan.remote.forge.as_str(),
        plan.remote.name,
        plan.branch
    )
}

fn status_label(status: CloudJobStatus) -> &'static str {
    match status {
        CloudJobStatus::Proposed => "proposed",
        CloudJobStatus::Refused => "refused",
        CloudJobStatus::Launching => "launching",
        CloudJobStatus::Running => "running",
        CloudJobStatus::Failed => "failed",
        CloudJobStatus::Canceled => "canceled",
    }
}

fn one_line(value: &str, max: usize) -> String {
    let flat: String = value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if flat.chars().count() <= max {
        flat
    } else {
        let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn sanitize_error(message: &str) -> String {
    message
        .chars()
        .filter(|ch| !ch.is_control())
        .take(240)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remotes(rows: &[(&str, &str)]) -> Vec<GitRemote> {
        rows.iter()
            .map(|(name, url)| GitRemote {
                name: (*name).to_string(),
                url: (*url).to_string(),
            })
            .collect()
    }

    struct RecordingLauncher {
        sandbox_id: String,
        fail: bool,
    }

    impl DaytonaLauncher for RecordingLauncher {
        fn create_sandbox(&self, _job: &CloudJob) -> Result<SandboxReceipt> {
            if self.fail {
                bail!("fixture launcher refused");
            }
            Ok(SandboxReceipt {
                sandbox_id: self.sandbox_id.clone(),
            })
        }
    }

    #[test]
    fn named_github_is_authoritative_even_when_origin_is_cnb() {
        assert_eq!(
            classify_remote("github", "https://cnb.cool/mirror/app.git"),
            Some(Forge::Github)
        );
        assert_eq!(
            classify_remote("origin", "https://cnb.cool/mirror/app.git"),
            Some(Forge::Cnb)
        );
        assert_eq!(
            classify_remote("origin", "https://github.com/Hmbown/CodeWhale.git"),
            Some(Forge::Github)
        );
        assert_eq!(
            classify_remote("gitee", "git@gitee.com:org/app.git"),
            Some(Forge::Gitee)
        );
        assert_eq!(
            classify_remote("upstream", "https://example.test/x.git"),
            None
        );
    }

    #[test]
    fn ambiguous_github_and_cnb_require_an_explicit_remote() {
        let rows = remotes(&[
            ("github", "https://github.com/Hmbown/CodeWhale.git"),
            ("origin", "https://cnb.cool/codewhale.net/codewhale.git"),
        ]);
        assert_eq!(
            select_remote(&rows, None),
            Err(DispatchError::AmbiguousRemote)
        );
        let github = select_remote(&rows, Some(Forge::Github)).unwrap();
        assert_eq!(github.name, "github");
        let cnb = select_remote(&rows, Some(Forge::Cnb)).unwrap();
        assert_eq!(cnb.name, "origin");
    }

    #[test]
    fn plan_rejects_empty_prompt_and_hostile_branch() {
        let rows = remotes(&[("github", "https://github.com/org/repo.git")]);
        assert_eq!(
            plan_dispatch(&rows, "   ", None, None).unwrap_err(),
            DispatchError::EmptyPrompt
        );
        assert_eq!(
            plan_dispatch(&rows, "fix flake", None, Some("-bad;rm")).unwrap_err(),
            DispatchError::InvalidBranch
        );
        let plan = plan_dispatch(
            &rows,
            "fix flake",
            Some(Forge::Github),
            Some("codewhale/cloud-1"),
        )
        .unwrap();
        assert_eq!(plan.remote.forge, Forge::Github);
        assert_eq!(plan.branch, "codewhale/cloud-1");
    }

    #[test]
    fn unconfirmed_dispatch_writes_a_proposal_and_never_launches() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let plan = plan_dispatch(
            &remotes(&[("github", "https://github.com/org/repo.git")]),
            "open a PR for the flake",
            Some(Forge::Github),
            Some("codewhale/cloud-test"),
        )
        .unwrap();
        let launcher = RecordingLauncher {
            sandbox_id: "should-not-create".to_string(),
            fail: false,
        };
        let outcome = execute_dispatch(
            &store,
            plan,
            false,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
            &launcher,
        )
        .unwrap();
        let DispatchOutcome::Proposal(job) = outcome else {
            panic!("expected proposal");
        };
        assert_eq!(job.status, CloudJobStatus::Proposed);
        assert!(!job.confirmed);
        assert!(job.sandbox_id.is_none());
        assert!(job.pr_url.is_none());
        assert_eq!(job.kind, "cloud");
        assert!(!should_auto_confirm(&DispatchPlan {
            prompt: job.prompt.clone(),
            remote: SelectedRemote {
                forge: job.forge,
                name: job.remote_name.clone(),
                url: job.remote_url.clone(),
            },
            branch: job.branch.clone(),
        }));
    }

    #[test]
    fn confirmed_dispatch_fails_closed_without_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let plan = plan_dispatch(
            &remotes(&[("origin", "https://cnb.cool/org/repo.git")]),
            "raise a CNB PR",
            Some(Forge::Cnb),
            Some("codewhale/cloud-cnb"),
        )
        .unwrap();
        let launcher = RecordingLauncher {
            sandbox_id: "should-not-create".to_string(),
            fail: false,
        };
        let outcome =
            execute_dispatch(&store, plan, true, &CredentialState::Missing, &launcher).unwrap();
        let DispatchOutcome::Refused(job) = outcome else {
            panic!("expected refuse");
        };
        assert_eq!(job.status, CloudJobStatus::Refused);
        assert!(job.confirmed);
        assert!(job.sandbox_id.is_none());
        assert!(job.pr_url.is_none());
        assert!(job.note.contains("cloud dispatch fails closed"));
        assert!(!job.note.contains("DAYTONA"));
        assert!(!job.note.contains("sk-"));
    }

    #[test]
    fn confirmed_dispatch_with_launcher_never_claims_a_pr() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let plan = plan_dispatch(
            &remotes(&[("gitee", "https://gitee.com/org/repo.git")]),
            "gitee offload",
            Some(Forge::Gitee),
            Some("codewhale/cloud-gitee"),
        )
        .unwrap();
        let launcher = RecordingLauncher {
            sandbox_id: "sandbox_fixture_1".to_string(),
            fail: false,
        };
        let outcome = execute_dispatch(
            &store,
            plan,
            true,
            &CredentialState::Present {
                source: CredentialSource::Keyring,
            },
            &launcher,
        )
        .unwrap();
        let DispatchOutcome::Accepted(job) = outcome else {
            panic!("expected accept");
        };
        assert_eq!(job.status, CloudJobStatus::Running);
        assert_eq!(job.sandbox_id.as_deref(), Some("sandbox_fixture_1"));
        assert!(job.pr_url.is_none());
        assert!(job.note.contains("not claimed"));
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, job.id);
        let canceled = cancel_job(&store, &job.id).unwrap();
        assert_eq!(canceled.status, CloudJobStatus::Canceled);
    }

    #[test]
    fn parse_git_remote_listing_prefers_first_url_per_name() {
        let parsed = parse_remote_listing(
            "github\thttps://github.com/Hmbown/CodeWhale.git (fetch)\n\
             github\thttps://github.com/Hmbown/CodeWhale.git (push)\n\
             origin\thttps://cnb.cool/codewhale.net/codewhale.git (fetch)\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "github");
        assert_eq!(parsed[1].name, "origin");
        assert_eq!(
            classify_remote(&parsed[0].name, &parsed[0].url),
            Some(Forge::Github)
        );
        assert_eq!(
            classify_remote(&parsed[1].name, &parsed[1].url),
            Some(Forge::Cnb)
        );
    }

    #[test]
    fn missing_credentials_copy_never_embeds_a_secret() {
        let message = missing_credentials_message();
        assert!(message.contains("cloud dispatch fails closed"));
        assert!(!message.contains("DAYTONA"));
        assert!(!message.contains("sk-"));
        assert!(!message.contains("Bearer"));
    }
}
