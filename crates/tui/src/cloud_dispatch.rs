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
//! The engine that drives a confirmed job end to end (sandbox → harness →
//! forge PR → teardown) lives in [`crate::dispatch_runner`]; this module owns
//! the persisted contract, the launcher seam, and the fail-closed gates.
//! Auto-decide heuristics remain leftover: Codewhale may propose, never
//! confirm itself.

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
/// Path inside the sandbox where the target repository is cloned.
pub const SANDBOX_WORKSPACE: &str = "/workspace";
const MAX_HARNESS_OUTPUT_CHARS: usize = 200_000;
const READY_POLL_ATTEMPTS: u32 = 40;
const READY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
/// Sandbox label key carrying the cloud job id (see
/// [`LiveDaytonaLauncher::create_sandbox`]); the reconciler joins sandbox
/// labels back to job records through it.
pub const SANDBOX_JOB_LABEL: &str = "codewhale.job";
/// Sandbox label marking Codewhale dispatch sandboxes — the product tag the
/// label reconciler filters the provider's sandbox list on.
pub const SANDBOX_PRODUCT_LABEL: &str = "codewhale.product";
pub const SANDBOX_PRODUCT_VALUE: &str = "dispatch";
/// Active jobs older than this are stale. The declared harness budget for
/// one cloud-agent turn is an hour (`HARNESS_TIMEOUT_SECS`), so an active
/// record with no terminal state after 90 minutes (harness budget plus
/// control-plane slack) cannot be a healthy run — its runner is gone (TUI
/// quit, crash, killed process) and the record is the only witness. The
/// sweep fails such records and tears their sandboxes down.
pub const STALE_ACTIVE_JOB_SECS: u64 = 90 * 60;

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
///
/// The runner path is `Proposed` (queued for an explicit confirm) →
/// `Launching` → `Running` (harness turn in the sandbox) → `OpeningPr` →
/// `Done`, with `Failed` / `Canceled` reachable from every active state and
/// `Refused` reserved for the fail-closed membership/credential gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudJobStatus {
    Proposed,
    Refused,
    Launching,
    Running,
    #[serde(rename = "openingpr")]
    OpeningPr,
    Done,
    Failed,
    Canceled,
}

/// Durable cloud job record, listed on the same `/jobs` surface as Bash jobs.
///
/// Fields added after the first landing carry `#[serde(default)]` so job
/// records written by earlier builds still load.
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
    /// Default branch of the agent's clone (the PR base), when known.
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Head commit the agent produced, when known.
    #[serde(default)]
    pub head_sha: Option<String>,
    /// One-line truthful summary of what the agent did, when reported.
    #[serde(default)]
    pub agent_summary: Option<String>,
    /// When the job reached a terminal state (`done`/`failed`/`canceled`).
    #[serde(default)]
    pub finished_unix: Option<u64>,
    /// Intent record: a sandbox create POST is in flight for this job. Set
    /// and persisted *before* the POST so that a create whose response is
    /// slow (client timeout, process death) is still reconcilable by
    /// sandbox label even though the sandbox id never arrived. Cleared once
    /// the id is recorded.
    #[serde(default)]
    pub sandbox_pending: bool,
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

    /// Cancel-authoritative save: refuse to overwrite a `canceled` record.
    ///
    /// The runner does read-modify-write phase saves while `/dispatch
    /// cancel` (or `--cancel`) can flip the record concurrently; an
    /// unconditional phase save landing after the cancel would resurrect a
    /// dead run — including its branch push and PR. Returns `Ok(false)`
    /// (leaving the cancellation exactly as the user left it) when the
    /// persisted record is already `canceled`, `Ok(true)` after a normal
    /// save.
    ///
    /// This is load-check-save: the store is file-backed with no
    /// cross-process lock, so the check cannot remove the load→save window
    /// entirely — it narrows the clobber window from a whole phase (seconds
    /// to minutes) to the span of one save, which is the single-writer
    /// discipline this store assumes elsewhere.
    pub fn save_unless_canceled(&self, job: &CloudJob) -> Result<bool> {
        if let Ok(current) = self.load(&job.id)
            && current.status == CloudJobStatus::Canceled
        {
            return Ok(false);
        }
        self.save(job)?;
        Ok(true)
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
///
/// With `confirm`, the job is persisted as `launching` and returned as
/// `Accepted`; the caller then starts the remote runner
/// ([`crate::dispatch_runner::run_confirmed_job`]) so this function stays
/// synchronous and offline-testable.
pub fn execute_dispatch(
    store: &CloudJobStore,
    plan: DispatchPlan,
    confirm: bool,
    credentials: &CredentialState,
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
        base_branch: None,
        head_sha: None,
        agent_summary: None,
        finished_unix: None,
        sandbox_pending: false,
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
        job.finished_unix = Some(unix_now());
        store.save(&job)?;
        return Ok(DispatchOutcome::Refused(job));
    }

    job.status = CloudJobStatus::Launching;
    job.note = "Cloud agent confirmed; the sandbox is launching and the runner will raise the branch and open the PR. Watch `codewhale dispatch --show` or `/dispatch show`.".to_string();
    store.save(&job)?;
    Ok(DispatchOutcome::Accepted(job))
}

/// Confirm a previously proposed job — in place, under the SAME id.
///
/// The record is mutated (`proposed` → `launching`, `confirmed = true`) and
/// saved as itself; a second confirm finds `launching` and refuses. Routing
/// through `execute_dispatch` instead would allocate a fresh job id (its ids
/// hash the plan plus `unix_now()` at second granularity), leaving the
/// proposal re-confirmable without limit — every confirm another sandbox
/// and another PR.
pub fn confirm_job(
    store: &CloudJobStore,
    id: &str,
    credentials: &CredentialState,
) -> Result<DispatchOutcome> {
    let mut job = store.load(id)?;
    if job.status != CloudJobStatus::Proposed {
        bail!(
            "Cloud job {} is {} and cannot be confirmed.",
            job.id,
            status_label(job.status)
        );
    }
    job.confirmed = true;
    if matches!(credentials, CredentialState::Missing) {
        job.status = CloudJobStatus::Refused;
        job.refusal = Some(missing_credentials_message());
        job.note = missing_credentials_message();
        job.finished_unix = Some(unix_now());
        store.save(&job)?;
        return Ok(DispatchOutcome::Refused(job));
    }

    job.status = CloudJobStatus::Launching;
    job.note = "Cloud agent confirmed; the sandbox is launching and the runner will raise the branch and open the PR. Watch `codewhale dispatch --show` or `/dispatch show`.".to_string();
    store.save(&job)?;
    Ok(DispatchOutcome::Accepted(job))
}

/// True while the job may still hold a live sandbox.
pub fn job_is_active(status: CloudJobStatus) -> bool {
    matches!(
        status,
        CloudJobStatus::Launching | CloudJobStatus::Running | CloudJobStatus::OpeningPr
    )
}

/// Cancel a job, tearing down a live sandbox when one exists.
///
/// The record flips to `canceled` first (so a concurrent runner step sees it),
/// then teardown runs best-effort through the launcher; a teardown failure is
/// reported in the note, never silently dropped.
pub fn cancel_job(
    store: &CloudJobStore,
    id: &str,
    launcher: &dyn DaytonaLauncher,
) -> Result<CloudJob> {
    let mut job = store.load(id)?;
    if matches!(
        job.status,
        CloudJobStatus::Canceled | CloudJobStatus::Failed | CloudJobStatus::Refused
    ) {
        return Ok(job);
    }
    let had_sandbox = job.sandbox_id.is_some();
    let sandbox_may_exist = had_sandbox || job.sandbox_pending;
    job.status = CloudJobStatus::Canceled;
    job.finished_unix = Some(unix_now());
    job.note = if sandbox_may_exist {
        "Canceled locally; the cloud agent sandbox is being torn down.".to_string()
    } else {
        "Canceled locally before a sandbox was created.".to_string()
    };
    store.save(&job)?;
    if let Some(sandbox_id) = job.sandbox_id.clone() {
        let receipt = SandboxReceipt {
            sandbox_id,
            toolbox_url: None,
        };
        match launcher.teardown(&receipt) {
            Ok(()) => {
                job.note = "Canceled locally; the cloud agent sandbox was torn down.".to_string();
            }
            Err(error) => {
                job.note = format!(
                    "Canceled locally; sandbox teardown failed and may need a retry: {}",
                    sanitize_error(&error.to_string())
                );
            }
        }
        store.save(&job)?;
    }
    // A create whose POST landed but whose response never arrived leaves no
    // recorded id (`sandbox_pending` with no `sandbox_id`). Best-effort
    // label pass: delete any sandbox the provider still holds for this job.
    if job.sandbox_pending && job.sandbox_id.is_none() {
        match reconcile_job_sandboxes(launcher, &job.id) {
            Ok(0) => {}
            Ok(count) => {
                job.note = format!(
                    "Canceled locally; {count} unrecorded cloud agent sandbox(es) labeled for this job were torn down."
                );
                let _ = store.save(&job);
            }
            Err(error) => {
                job.note = format!(
                    "{} Unrecorded sandbox reconcile failed and may need a retry: {}",
                    job.note,
                    sanitize_error(&error.to_string())
                );
                let _ = store.save(&job);
            }
        }
    }
    Ok(job)
}

/// Outcome of one label-reconcile pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    /// Sandboxes deleted because their job is terminal or unknown.
    pub deleted: Vec<String>,
    /// Sandboxes left running because their job is still active.
    pub live: usize,
}

/// Delete dispatch-labeled sandboxes whose job no longer needs them.
///
/// Every sandbox [`LiveDaytonaLauncher::create_sandbox`] makes is labeled
/// with its job id and the dispatch product tag, so the provider's sandbox
/// list can be joined back to the store even when a record died mid-create.
/// A sandbox is deletable when its job label is missing or invalid, its job
/// record is absent from the store, or that job is terminal (`refused`,
/// `failed`, `canceled`, `done`). Sandboxes for active jobs are left alone —
/// their runner owns them. One sandbox failing to delete never stops the
/// rest; the report records what actually happened.
pub fn reconcile_sandboxes(
    store: &CloudJobStore,
    launcher: &dyn DaytonaLauncher,
) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    for sandbox in launcher.list_job_sandboxes()? {
        let deletable = match sandbox
            .job_id
            .as_deref()
            .filter(|job_id| valid_job_id(job_id))
        {
            None => true,
            Some(job_id) => match store.load(job_id) {
                Err(_) => true,
                Ok(job) => !job_is_active(job.status),
            },
        };
        if !deletable {
            report.live += 1;
            continue;
        }
        let receipt = SandboxReceipt {
            sandbox_id: sandbox.sandbox_id.clone(),
            toolbox_url: None,
        };
        if launcher.teardown(&receipt).is_ok() {
            report.deleted.push(sandbox.sandbox_id);
        }
    }
    Ok(report)
}

/// Best-effort: delete every dispatch sandbox labeled for one job id. Used
/// by cancel when the create POST may have landed without a receipt. Same
/// failure rule as [`reconcile_sandboxes`]: a sandbox that cannot be
/// deleted is skipped, not fatal.
pub fn reconcile_job_sandboxes(launcher: &dyn DaytonaLauncher, job_id: &str) -> Result<usize> {
    let mut deleted = 0;
    for sandbox in launcher.list_job_sandboxes()? {
        if sandbox.job_id.as_deref() != Some(job_id) {
            continue;
        }
        let receipt = SandboxReceipt {
            sandbox_id: sandbox.sandbox_id.clone(),
            toolbox_url: None,
        };
        if launcher.teardown(&receipt).is_ok() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Startup sweep: fail stale active jobs and tear their sandboxes down.
///
/// The TUI spawns the runner detached, so quitting the TUI (or crashing)
/// leaves a `launching`/`running`/`openingpr` record whose runner is gone —
/// nothing else reconciles it, and any sandbox it made bills forever. On
/// startup, every active job older than [`STALE_ACTIVE_JOB_SECS`] is marked
/// `failed` with a truthful note, its recorded sandbox is torn down, and the
/// record is saved *before* teardown so a crash mid-sweep still leaves a
/// terminal record the label reconciler will clean up after. Returns the
/// swept records (empty when the store is unreadable — never fatal).
pub fn sweep_stale_jobs(
    store: &CloudJobStore,
    launcher: &dyn DaytonaLauncher,
    now_unix: u64,
) -> Vec<CloudJob> {
    let Ok(jobs) = store.list() else {
        return Vec::new();
    };
    let mut swept = Vec::new();
    for mut job in jobs {
        if !job_is_active(job.status) {
            continue;
        }
        let age_secs = now_unix.saturating_sub(job.created_unix);
        if age_secs < STALE_ACTIVE_JOB_SECS {
            continue;
        }
        let sandbox_note = if job.sandbox_id.is_some() || job.sandbox_pending {
            "Sandbox teardown was attempted; any sandbox left behind is deleted by the label reconciler on this startup."
        } else {
            "No sandbox was recorded for this job."
        };
        job.status = CloudJobStatus::Failed;
        job.finished_unix = Some(now_unix);
        job.refusal =
            Some("stale: the runner stopped without recording a terminal state".to_string());
        job.note = format!(
            "Marked stale by the startup sweep: no terminal state for {} minutes and the declared harness budget is 60. {sandbox_note}",
            age_secs / 60,
        );
        if let Some(sandbox_id) = job.sandbox_id.clone() {
            let receipt = SandboxReceipt {
                sandbox_id,
                toolbox_url: None,
            };
            let _ = launcher.teardown(&receipt);
        }
        if store.save(&job).is_ok() {
            swept.push(job);
        }
    }
    swept
}

/// Quit-path warning for the TUI: names live cloud jobs that quitting would
/// leave behind. `None` when no job is `launching`/`running`/`openingpr` or
/// the store cannot be read (a warning must never block the quit path).
pub fn live_job_quit_warning(store: &CloudJobStore) -> Option<String> {
    let jobs = store.list().ok()?;
    let live: Vec<&str> = jobs
        .iter()
        .filter(|job| job_is_active(job.status))
        .map(|job| job.id.as_str())
        .take(3)
        .collect();
    if live.is_empty() {
        return None;
    }
    Some(format!(
        "Cloud job(s) {} still running: quitting now leaves the sandbox up until the next startup sweep reconciles it; /dispatch cancel <id> tears it down immediately.",
        live.join(", ")
    ))
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
        if let Some(minutes) = runtime_minutes(job) {
            lines.push(format!("  runtime: {minutes}m"));
        }
    }
    lines.push(
        "Controls: /dispatch show <id>, /dispatch confirm <id>, /dispatch cancel <id>, /jobs list."
            .to_string(),
    );
    lines.join("\n")
}

/// Whole minutes a terminal job was active, when internally known. This is
/// local bookkeeping (created → finished), not a provider billing figure;
/// sub-minute runs are omitted rather than rounded up.
pub fn runtime_minutes(job: &CloudJob) -> Option<u64> {
    job.finished_unix
        .map(|end| end.saturating_sub(job.created_unix) / 60)
        .filter(|minutes| *minutes > 0)
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
        format!(
            "Base: {}",
            job.base_branch
                .as_deref()
                .unwrap_or("(detected at run time)")
        ),
        format!("Confirmed: {}", job.confirmed),
        format!("Sandbox: {}", job.sandbox_id.as_deref().unwrap_or("(none)")),
        format!("PR: {}", job.pr_url.as_deref().unwrap_or("(not opened)")),
        format!("Head: {}", job.head_sha.as_deref().unwrap_or("(pending)")),
    ];
    if let Some(minutes) = runtime_minutes(job) {
        lines.push(format!(
            "Runtime: {minutes}m (Codewhale bookkeeping, not a bill)"
        ));
    }
    if let Some(summary) = job.agent_summary.as_deref() {
        lines.push(format!("Agent: {}", one_line(summary, 200)));
    }
    lines.push(format!("Prompt: {}", job.prompt));
    lines.push(format!("Note: {}", job.note));
    if let Some(refusal) = job.refusal.as_ref() {
        lines.push(format!("Refusal: {refusal}"));
    }
    lines.join("\n")
}

/// Status card for bare `/dispatch` and `codewhale dispatch --status`.
///
/// `recent` is the newest slice of the job store; when the runner has
/// receipts (sandbox id, PR URL, runtime) they are surfaced here verbatim.
pub fn format_status(
    remotes: &[GitRemote],
    credentials: &CredentialState,
    recent: &[CloudJob],
) -> String {
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
    if !recent.is_empty() {
        lines.push("Recent cloud jobs:".to_string());
        for job in recent.iter().take(5) {
            let mut line = format!(
                "  {}  {}  {}",
                job.id,
                status_label(job.status),
                one_line(&job.prompt, 60)
            );
            if let Some(pr) = job.pr_url.as_deref() {
                line.push_str(&format!("  pr: {pr}"));
            } else if let Some(sandbox) = job.sandbox_id.as_deref() {
                line.push_str(&format!("  sandbox: {sandbox}"));
            }
            if let Some(minutes) = runtime_minutes(job) {
                line.push_str(&format!("  {minutes}m"));
            }
            lines.push(line);
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
///
/// The methods after `create_sandbox` default to "unsupported" so partial
/// fixtures keep compiling; the real runner (and the recording tests) drive
/// the full protocol.
pub trait DaytonaLauncher {
    /// Create the sandbox and return its id plus the (validated) toolbox URL.
    fn create_sandbox(&self, job: &CloudJob) -> Result<SandboxReceipt>;
    /// Block until the sandbox accepts toolbox calls (bounded poll).
    fn wait_ready(&self, _receipt: &SandboxReceipt) -> Result<()> {
        Ok(())
    }
    /// Clone the target forge repository inside the sandbox.
    fn clone_repository(&self, _receipt: &SandboxReceipt, _url: &str, _path: &str) -> Result<()> {
        bail!("this launcher does not support repository clones")
    }
    /// Run one harness command inside the sandbox and return bounded stdout.
    fn run_harness(&self, _receipt: &SandboxReceipt, _command: &HarnessCommand) -> Result<String> {
        bail!("this launcher does not support harness execution")
    }
    /// Collect the agent's work product from the sandbox.
    fn collect_patch(&self, _receipt: &SandboxReceipt) -> Result<PatchReceipt> {
        bail!("this launcher does not support patch collection")
    }
    /// Tear the sandbox down. Called on cancel, failure, and completion.
    fn teardown(&self, _receipt: &SandboxReceipt) -> Result<()> {
        bail!("this launcher does not support teardown")
    }
    /// List Codewhale-dispatch sandboxes with their job labels. Used by the
    /// reconciler (startup sweep and cancel); not part of the run protocol.
    fn list_job_sandboxes(&self) -> Result<Vec<LabeledSandbox>> {
        bail!("this launcher does not support sandbox listing")
    }
}

/// One dispatch-labeled sandbox discovered by the reconciler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabeledSandbox {
    pub sandbox_id: String,
    /// Job id from the `codewhale.job` label, when the label is present.
    pub job_id: Option<String>,
}

/// Provider receipt for a created sandbox. `toolbox_url` is the validated
/// per-sandbox toolbox origin returned by the create call, when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxReceipt {
    pub sandbox_id: String,
    pub toolbox_url: Option<String>,
}

/// One command the runner asks the sandbox to execute. `argv` is exact: the
/// recording tests pin it so the live protocol cannot drift silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommand {
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_secs: u32,
}

/// The agent's work product collected from the sandbox clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchReceipt {
    /// Base branch of the clone (the PR base), e.g. `main`.
    pub base_branch: String,
    /// Head commit the agent produced.
    pub head_sha: String,
    /// One-line truthful summary of the agent's commit.
    pub summary: String,
    /// `git format-patch --stdout` payload (bounded) of the agent's commits.
    pub patch: String,
}

/// Validate an outbound origin for credential-bearing HTTP calls.
///
/// Rules:
/// - `https` only for public hosts.
/// - explicit loopback hosts (`localhost`, `127.0.0.1`, `::1`) are allowed
///   only in debug builds, as the escape hatch for local smoke tests against
///   a self-hosted sandbox service; release builds reject them outright.
/// - the host must not be a private / link-local / reserved / multicast
///   address or a `.local` / `.internal` name, and no userinfo may ride
///   along.
///
/// DNS-resolved rebinding is out of scope and documented as such.
pub fn validate_outbound_origin(raw: &str) -> Result<reqwest::Url> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_REMOTE_BYTES {
        bail!("outbound origin is empty or oversized");
    }
    let url = reqwest::Url::parse(trimmed).context("outbound origin is not a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("outbound origin must be http or https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("outbound origin must not embed credentials");
    }
    let host = url
        .host_str()
        .context("outbound origin has no host")?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    // `Url::host_str` keeps IPv6 brackets; strip them for the checks below.
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .map(str::to_string)
        .unwrap_or(host);
    let loopback_name = host == "localhost" || host == "127.0.0.1" || host == "::1";
    if loopback_name {
        if cfg!(debug_assertions) {
            return Ok(url);
        }
        bail!("loopback origins are not allowed in release builds");
    }
    if host.ends_with(".local") || host.ends_with(".internal") {
        bail!("outbound origin must be a public service host");
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let blocked = match ip {
            std::net::IpAddr::V4(v4) => {
                let octets = v4.octets();
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                    || v4.is_multicast()
                    || v4.is_documentation()
                    // 100.64.0.0/10 (carrier-grade NAT, `is_shared` is
                    // not stable yet)
                    || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000)
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        };
        if blocked {
            bail!("outbound origin must not target a loopback, private, or reserved address");
        }
    }
    if url.scheme() != "https" {
        bail!("outbound origin must use https");
    }
    Ok(url)
}

/// Real Daytona HTTP launcher. Fails closed on every step; never invents a
/// PR URL; never logs or returns the API key.
///
/// API shape (pinned against the published OpenAPI specs, see
/// docs/DAYTONA_CLOUD_DISPATCH.md):
/// - control plane `POST /sandbox`, `GET /sandbox/{id}`, `DELETE /sandbox/{id}`
/// - toolbox `{toolboxProxyUrl}/{sandboxId}` with `POST /git/clone` and
///   `POST /process/execute` (`{command, cwd, timeout}` → `{exitCode, result}`)
pub struct LiveDaytonaLauncher;

impl LiveDaytonaLauncher {
    /// Total timeout for short control-plane calls (create/status/delete/
    /// list). A dispatched harness turn is NOT a short call — see
    /// [`Self::harness_client`].
    const CONTROL_PLANE_TIMEOUT_SECS: u64 = 120;

    /// Slack added to a harness command's declared budget for the client
    /// that carries it: process start, clone drift, and response transfer
    /// are not part of the declared turn budget, but the client must still
    /// cut off eventually so a hung execute cannot hold a runner forever.
    const HARNESS_CLIENT_SLACK_SECS: u64 = 120;

    fn blocking_client() -> Result<reqwest::blocking::Client> {
        Self::blocking_client_with_timeout(Self::CONTROL_PLANE_TIMEOUT_SECS)
    }

    fn blocking_client_with_timeout(total_secs: u64) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(total_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to initialize the cloud agent client")
    }

    /// A client scoped to one harness command: its total timeout is the
    /// command's declared budget plus fixed slack. The declared turn budget
    /// is an hour, so the 120s control-plane cap must NOT carry this call —
    /// otherwise every dispatched turn longer than two minutes fails after
    /// the spend has already started.
    fn harness_client(command: &HarnessCommand) -> Result<reqwest::blocking::Client> {
        Self::blocking_client_with_timeout(Self::harness_client_budget_secs(command))
    }

    /// The total-timeout budget for a harness-carrying client, in seconds.
    /// Public to the crate so the runner's tests can pin it against the
    /// declared `HARNESS_TIMEOUT_SECS`.
    pub(crate) fn harness_client_budget_secs(command: &HarnessCommand) -> u64 {
        u64::from(command.timeout_secs).saturating_add(Self::HARNESS_CLIENT_SLACK_SECS)
    }

    fn api_key() -> Result<String> {
        read_api_key().ok_or_else(|| anyhow!(missing_credentials_message()))
    }

    /// Control-plane URL under the validated base.
    fn control_plane_url(path: &str) -> Result<reqwest::Url> {
        let base = validate_outbound_origin(&daytona_api_url())?;
        base.join(path.trim_start_matches('/'))
            .context("failed to build the cloud agent request URL")
    }

    /// Toolbox base for one sandbox: `{toolboxProxyUrl}/{sandboxId}`.
    fn toolbox_base(receipt: &SandboxReceipt) -> Result<reqwest::Url> {
        if !valid_sandbox_id(&receipt.sandbox_id) {
            bail!("the sandbox id is not a usable path token");
        }
        let fallback = format!("{}/toolbox", DEFAULT_DAYTONA_API);
        let raw = receipt.toolbox_url.as_deref().unwrap_or(&fallback);
        let base = validate_outbound_origin(raw)?;
        base.join(&format!("{}/", receipt.sandbox_id))
            .context("failed to build the sandbox toolbox URL")
    }

    fn send_json(
        method: reqwest::Method,
        url: &reqwest::Url,
        api_key: &str,
        body: serde_json::Value,
    ) -> Result<reqwest::blocking::Response> {
        Self::send_json_on(&Self::blocking_client()?, method, url, api_key, body)
    }

    /// [`Self::send_json`] on a caller-supplied client, so a call whose
    /// declared budget differs from the control-plane cap (the harness
    /// turn) can carry a client scoped to its own budget.
    fn send_json_on(
        client: &reqwest::blocking::Client,
        method: reqwest::Method,
        url: &reqwest::Url,
        api_key: &str,
        body: serde_json::Value,
    ) -> Result<reqwest::blocking::Response> {
        client
            .request(method, url.clone())
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .context("could not reach the cloud agent service")
    }
}

impl DaytonaLauncher for LiveDaytonaLauncher {
    fn create_sandbox(&self, job: &CloudJob) -> Result<SandboxReceipt> {
        let api_key = Self::api_key()?;
        let url = Self::control_plane_url("sandbox")?;
        // TODO(founding decision, deliberately not invented here): the
        // sandbox image / snapshot / env-vars design is still pending, so
        // this create body carries no image, snapshot, or envVars and the
        // created sandbox CANNOT be assumed to provide the `codewhale`
        // harness (`codewhale exec --auto`). Once that decision lands, the
        // confirm gate (confirm_job / execute_dispatch) must hard-fail
        // truthfully — "the cloud agent image cannot run this job" — rather
        // than take spend for a sandbox that cannot execute the harness
        // step. No gating flag exists today and none is added here; wire
        // the warning through whatever config surface that decision picks.
        let body = serde_json::json!({
            "name": format!("cw-{}", job.id.replace('_', "-")),
            "labels": {
                SANDBOX_JOB_LABEL: job.id,
                "codewhale.forge": job.forge.as_str(),
                SANDBOX_PRODUCT_LABEL: SANDBOX_PRODUCT_VALUE,
            }
        });
        let response = Self::send_json(reqwest::Method::POST, &url, &api_key, body)?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            bail!("Cloud agent create failed (HTTP {status}).");
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).context("the cloud agent service returned invalid JSON")?;
        let sandbox_id = parsed
            .get("id")
            .or_else(|| parsed.get("sandboxId"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !valid_sandbox_id(&sandbox_id) {
            bail!("Cloud agent create succeeded but returned no usable sandbox id.");
        }
        let toolbox_url = parsed
            .get("toolboxProxyUrl")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= MAX_REMOTE_BYTES)
            .and_then(|value| validate_outbound_origin(value).ok())
            .map(|url| url.to_string());
        Ok(SandboxReceipt {
            sandbox_id,
            toolbox_url,
        })
    }

    fn wait_ready(&self, receipt: &SandboxReceipt) -> Result<()> {
        let api_key = Self::api_key()?;
        if !valid_sandbox_id(&receipt.sandbox_id) {
            bail!("the sandbox id is not a usable path token");
        }
        let url = Self::control_plane_url(&format!("sandbox/{}", receipt.sandbox_id))?;
        for _ in 0..READY_POLL_ATTEMPTS {
            let response = Self::send_json(
                reqwest::Method::GET,
                &url,
                &api_key,
                serde_json::Value::Null,
            );
            match response {
                Ok(response) if response.status().is_success() => {
                    let text = response.text().unwrap_or_default();
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text)
                        && let Some(state) = parsed.get("state").and_then(|v| v.as_str())
                    {
                        match state {
                            "started" | "ready" => return Ok(()),
                            "error" | "destroyed" | "archived" => {
                                bail!("Cloud agent sandbox entered state '{state}'.");
                            }
                            _ => {}
                        }
                    } else {
                        return Ok(());
                    }
                }
                Err(error) => return Err(error),
                Ok(response) => {
                    if response.status().as_u16() == 404 {
                        bail!("Cloud agent sandbox disappeared before it was ready.");
                    }
                }
            }
            std::thread::sleep(READY_POLL_INTERVAL);
        }
        bail!("Cloud agent sandbox was not ready in time.");
    }

    fn clone_repository(&self, receipt: &SandboxReceipt, repo_url: &str, path: &str) -> Result<()> {
        let api_key = Self::api_key()?;
        let url = Self::toolbox_base(receipt)?.join("git/clone")?;
        let body = serde_json::json!({ "url": repo_url, "path": path });
        let response = Self::send_json(reqwest::Method::POST, &url, &api_key, body)?;
        let status = response.status();
        if !status.is_success() {
            bail!("Cloud agent repository clone failed (HTTP {status}).");
        }
        Ok(())
    }

    fn run_harness(&self, receipt: &SandboxReceipt, command: &HarnessCommand) -> Result<String> {
        let api_key = Self::api_key()?;
        let url = Self::toolbox_base(receipt)?.join("process/execute")?;
        // The toolbox executes one shell command string, so every argv
        // element is POSIX-single-quoted — a prompt cannot interpolate.
        let body = serde_json::json!({
            "command": shell_quote_join(&command.argv),
            "cwd": command.cwd,
            "timeout": command.timeout_secs,
        });
        // This call carries the declared turn budget (an hour for the agent
        // entry), so it rides a client scoped to that budget plus slack —
        // never the 120s control-plane client that used to cap it.
        let client = Self::harness_client(command)?;
        let response = Self::send_json_on(&client, reqwest::Method::POST, &url, &api_key, body)?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            bail!("Cloud agent harness execution failed (HTTP {status}).");
        }
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .context("the sandbox returned an unreadable harness result")?;
        let exit_code = parsed
            .get("exitCode")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let result = parsed
            .get("result")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if exit_code != 0 {
            bail!(
                "Cloud agent harness exited with code {exit_code}: {}",
                sanitize_error(result)
            );
        }
        Ok(result.chars().take(MAX_HARNESS_OUTPUT_CHARS).collect())
    }

    fn collect_patch(&self, receipt: &SandboxReceipt) -> Result<PatchReceipt> {
        let base_branch = self
            .run_harness(
                receipt,
                &HarnessCommand {
                    argv: vec![
                        "git".to_string(),
                        "rev-parse".to_string(),
                        "--abbrev-ref".to_string(),
                        "origin/HEAD".to_string(),
                    ],
                    cwd: SANDBOX_WORKSPACE.to_string(),
                    timeout_secs: 30,
                },
            )?
            .trim()
            .trim_start_matches("origin/")
            .to_string();
        let head_sha = self
            .run_harness(
                receipt,
                &HarnessCommand {
                    argv: vec![
                        "git".to_string(),
                        "rev-parse".to_string(),
                        "HEAD".to_string(),
                    ],
                    cwd: SANDBOX_WORKSPACE.to_string(),
                    timeout_secs: 30,
                },
            )?
            .trim()
            .to_string();
        let summary = self
            .run_harness(
                receipt,
                &HarnessCommand {
                    argv: vec![
                        "git".to_string(),
                        "log".to_string(),
                        "-1".to_string(),
                        "--format=%s".to_string(),
                    ],
                    cwd: SANDBOX_WORKSPACE.to_string(),
                    timeout_secs: 30,
                },
            )?
            .trim()
            .to_string();
        let patch = self.run_harness(
            receipt,
            &HarnessCommand {
                argv: vec![
                    "git".to_string(),
                    "format-patch".to_string(),
                    "origin/HEAD..HEAD".to_string(),
                    "--stdout".to_string(),
                ],
                cwd: SANDBOX_WORKSPACE.to_string(),
                timeout_secs: 60,
            },
        )?;
        if base_branch.is_empty() || head_sha.len() < 7 {
            bail!("Cloud agent produced no branch head to raise.");
        }
        if patch.trim().is_empty() {
            bail!("Cloud agent produced an empty patch; refusing to open a PR.");
        }
        Ok(PatchReceipt {
            base_branch,
            head_sha,
            summary,
            patch,
        })
    }

    fn teardown(&self, receipt: &SandboxReceipt) -> Result<()> {
        let api_key = Self::api_key()?;
        if !valid_sandbox_id(&receipt.sandbox_id) {
            bail!("the sandbox id is not a usable path token");
        }
        let url = Self::control_plane_url(&format!("sandbox/{}", receipt.sandbox_id))?;
        let response = Self::send_json(
            reqwest::Method::DELETE,
            &url,
            &api_key,
            serde_json::Value::Null,
        )?;
        // Daytona returns 204 on delete; treat 404 as already-gone success so
        // cancel/complete teardown is idempotent.
        let status = response.status();
        if status.is_success() || status.as_u16() == 404 {
            Ok(())
        } else {
            bail!("Cloud agent sandbox teardown failed (HTTP {status}).");
        }
    }

    fn list_job_sandboxes(&self) -> Result<Vec<LabeledSandbox>> {
        let api_key = Self::api_key()?;
        // The provider's list call takes a JSON-encoded exact-match labels
        // filter (same OpenAPI family as create/get/delete above); filtering
        // on the product tag keeps the response to Codewhale dispatch
        // sandboxes only — never the user's own sandboxes on a shared key.
        let mut url = Self::control_plane_url("sandbox")?;
        url.query_pairs_mut().append_pair(
            "labels",
            &format!("{{\"{SANDBOX_PRODUCT_LABEL}\":\"{SANDBOX_PRODUCT_VALUE}\"}}"),
        );
        let response = Self::send_json(
            reqwest::Method::GET,
            &url,
            &api_key,
            serde_json::Value::Null,
        )?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            bail!("Cloud agent sandbox listing failed (HTTP {status}).");
        }
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .context("the cloud agent service returned an unreadable sandbox list")?;
        let rows = parsed.as_array().cloned().unwrap_or_default();
        let mut sandboxes = Vec::new();
        for row in rows {
            let sandbox_id = row
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if !valid_sandbox_id(&sandbox_id) {
                continue;
            }
            let job_id = row
                .get("labels")
                .and_then(|labels| labels.get(SANDBOX_JOB_LABEL))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            sandboxes.push(LabeledSandbox { sandbox_id, job_id });
        }
        Ok(sandboxes)
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

/// Current unix time, shared with the runner for receipt timestamps.
pub fn unix_timestamp() -> u64 {
    unix_now()
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
        "Proposed Codewhale cloud-agent offload to {} ({}) raising branch {}.",
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
        CloudJobStatus::OpeningPr => "openingpr",
        CloudJobStatus::Done => "done",
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

/// Sanitized (control-character-free, bounded) error text for job notes.
pub fn sanitize_error(message: &str) -> String {
    message
        .chars()
        .filter(|ch| !ch.is_control())
        .take(240)
        .collect()
}

/// Join argv into one POSIX shell command with every element single-quoted,
/// so nothing (the prompt included) can interpolate when the sandbox toolbox
/// executes the string.
pub fn shell_quote_join(argv: &[String]) -> String {
    argv.iter()
        .map(|part| format!("'{}'", part.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sandbox ids must be plain URL-path-safe tokens before they are used in
/// control-plane or toolbox paths.
fn valid_sandbox_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
        let outcome = execute_dispatch(
            &store,
            plan,
            false,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
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
        let outcome = execute_dispatch(&store, plan, true, &CredentialState::Missing).unwrap();
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
    fn confirmed_dispatch_queues_launching_without_touching_the_forge() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let plan = plan_dispatch(
            &remotes(&[("gitee", "https://gitee.com/org/repo.git")]),
            "gitee offload",
            Some(Forge::Gitee),
            Some("codewhale/cloud-gitee"),
        )
        .unwrap();
        let outcome = execute_dispatch(
            &store,
            plan,
            true,
            &CredentialState::Present {
                source: CredentialSource::Keyring,
            },
        )
        .unwrap();
        let DispatchOutcome::Accepted(job) = outcome else {
            panic!("expected accept");
        };
        assert_eq!(job.status, CloudJobStatus::Launching);
        assert!(job.confirmed);
        assert!(job.sandbox_id.is_none());
        assert!(job.pr_url.is_none());
        assert!(job.note.contains("runner will raise the branch"));
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, job.id);
        // No sandbox exists yet, so cancel is a pure record flip.
        let canceled = cancel_job(&store, &job.id, &NoopLauncher).unwrap();
        assert_eq!(canceled.status, CloudJobStatus::Canceled);
        assert!(canceled.note.contains("before a sandbox"));
    }

    #[test]
    fn confirm_job_confirms_in_place_under_the_same_id_and_is_not_reconfirmable() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let plan = plan_dispatch(
            &remotes(&[("github", "https://github.com/org/repo.git")]),
            "open a PR for the flake",
            Some(Forge::Github),
            Some("codewhale/cloud-confirm"),
        )
        .unwrap();
        let DispatchOutcome::Proposal(job) = execute_dispatch(
            &store,
            plan,
            false,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
        )
        .unwrap() else {
            panic!("expected proposal");
        };
        let id = job.id.clone();

        let confirmed = match confirm_job(
            &store,
            &id,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
        )
        .unwrap()
        {
            DispatchOutcome::Accepted(job) => job,
            other => panic!("expected accept, got {other:?}"),
        };
        // Same id, one record, launching: the proposal became the run.
        assert_eq!(confirmed.id, id);
        assert_eq!(confirmed.status, CloudJobStatus::Launching);
        assert!(confirmed.confirmed);
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1, "confirm must not mint a second record");
        assert_eq!(listed[0].id, id);

        // A second confirm is refused — the status gate, not the id, is the
        // guard (ids hash unix_now() at second granularity and can collide
        // across confirms).
        let again = confirm_job(
            &store,
            &id,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(again.contains("cannot be confirmed"), "{again}");
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn confirm_job_without_credentials_refuses_in_place_under_the_same_id() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let plan = plan_dispatch(
            &remotes(&[("github", "https://github.com/org/repo.git")]),
            "refuse me in place",
            Some(Forge::Github),
            Some("codewhale/cloud-refuse"),
        )
        .unwrap();
        let DispatchOutcome::Proposal(job) = execute_dispatch(
            &store,
            plan,
            false,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
        )
        .unwrap() else {
            panic!("expected proposal");
        };
        let id = job.id.clone();
        let refused = match confirm_job(&store, &id, &CredentialState::Missing).unwrap() {
            DispatchOutcome::Refused(job) => job,
            other => panic!("expected refuse, got {other:?}"),
        };
        assert_eq!(refused.id, id);
        assert_eq!(refused.status, CloudJobStatus::Refused);
        assert!(refused.confirmed);
        assert!(refused.finished_unix.is_some());
        assert_eq!(store.list().unwrap().len(), 1);
        // And it cannot be confirmed again either.
        assert!(confirm_job(&store, &id, &CredentialState::Missing).is_err());
    }

    /// Launcher that can create but never tear down; used to pin the
    /// cancel-without-sandbox path without any network surface.
    struct NoopLauncher;

    impl DaytonaLauncher for NoopLauncher {
        fn create_sandbox(&self, _job: &CloudJob) -> Result<SandboxReceipt> {
            bail!("no sandbox in this fixture")
        }
    }

    #[test]
    fn old_job_records_without_runner_fields_still_load() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        std::fs::create_dir_all(&root).unwrap();
        // Exactly the JSON shape written by the first landing of the slice.
        let legacy = serde_json::json!({
            "id": "cloud_00000000000000ff",
            "kind": "cloud",
            "status": "running",
            "prompt": "legacy job",
            "forge": "github",
            "remote_name": "github",
            "remote_url": "https://github.com/org/repo.git",
            "branch": "codewhale/cloud-legacy",
            "confirmed": true,
            "sandbox_id": "sandbox_legacy",
            "pr_url": null,
            "refusal": null,
            "note": "legacy note",
            "created_unix": 1_000_u64
        });
        std::fs::write(
            root.join("cloud_00000000000000ff.json"),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();
        let store = CloudJobStore::from_path(root);
        let jobs = store.list().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].sandbox_id.as_deref(), Some("sandbox_legacy"));
        assert!(jobs[0].base_branch.is_none());
        assert!(jobs[0].finished_unix.is_none());
        assert!(jobs[0].status == CloudJobStatus::Running);
    }

    #[test]
    fn shell_quoting_and_sandbox_id_guards_hold() {
        // A hostile prompt stays one single-quoted argument: every embedded
        // quote becomes '\'' and no metacharacter can escape the quoting.
        let hostile = "fix it'; rm -rf /; echo '$(whoami)'";
        let joined = shell_quote_join(&[
            "codewhale".to_string(),
            "exec".to_string(),
            "--auto".to_string(),
            hostile.to_string(),
        ]);
        assert_eq!(
            joined,
            "'codewhale' 'exec' '--auto' 'fix it'\\''; rm -rf /; echo '\\''$(whoami)'\\'''"
        );
        // Behavioral pin: a real shell sees the prompt as ONE argument.
        let printed = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "printf %s {}",
                shell_quote_join(&[hostile.to_string()])
            ))
            .output()
            .expect("sh is available in test environments");
        assert!(printed.status.success());
        assert_eq!(String::from_utf8_lossy(&printed.stdout), hostile);
        assert_eq!(shell_quote_join(&["a'b".to_string()]), "'a'\\''b'");
        assert!(valid_sandbox_id("sbx-123_abc"));
        for bad in ["", "../../evil", "sbx 1"] {
            assert!(!valid_sandbox_id(bad), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn net_guard_rejects_private_and_non_https_origins() {
        assert!(validate_outbound_origin("https://app.daytona.io/api").is_ok());
        assert!(validate_outbound_origin("https://gitee.com/api/v5").is_ok());
        assert!(validate_outbound_origin("ftp://example.com").is_err());
        assert!(validate_outbound_origin("https://user:pw@example.com/x").is_err());
        for blocked in [
            "http://example.com",
            "https://10.1.2.3/api",
            "https://192.168.1.10/api",
            "https://172.16.0.1/api",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/api",
            "https://0.0.0.0/api",
            "https://[fc00::1]/api",
            "https://[fe80::1]/api",
            "https://router.internal/api",
            "https://printer.local/api",
        ] {
            assert!(
                validate_outbound_origin(blocked).is_err(),
                "expected {blocked} to be rejected"
            );
        }
        // Loopback is the debug-only escape hatch for local smoke tests;
        // release builds reject it (pinned by the cfg! branch above).
        if cfg!(debug_assertions) {
            assert!(validate_outbound_origin("http://127.0.0.1:3986/api").is_ok());
            assert!(validate_outbound_origin("https://localhost/api").is_ok());
        }
    }

    #[test]
    fn status_card_surfaces_runner_receipts_without_provider_branding() {
        let rows = remotes(&[("github", "https://github.com/org/repo.git")]);
        let jobs = vec![CloudJob {
            id: "cloud_00000000000000aa".to_string(),
            kind: "cloud".to_string(),
            status: CloudJobStatus::Done,
            prompt: "fix the flake".to_string(),
            forge: Forge::Github,
            remote_name: "github".to_string(),
            remote_url: "https://github.com/org/repo.git".to_string(),
            branch: "codewhale/cloud-1".to_string(),
            confirmed: true,
            sandbox_id: Some("sandbox_receipt_1".to_string()),
            pr_url: Some("https://github.com/org/repo/pull/7".to_string()),
            refusal: None,
            note: "done".to_string(),
            created_unix: 1_000,
            base_branch: Some("main".to_string()),
            head_sha: Some("abc1234def".to_string()),
            agent_summary: Some("Fixed the flake".to_string()),
            finished_unix: Some(1_960),
            sandbox_pending: false,
        }];
        let card = format_status(
            &rows,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
            &jobs,
        );
        assert!(card.contains("cloud_00000000000000aa"));
        assert!(card.contains("done"));
        assert!(card.contains("https://github.com/org/repo/pull/7"));
        assert!(card.contains("16m"));
        assert!(!card.contains("Daytona"));
        assert!(!card.contains("daytona"));
        let detail = format_job(&jobs[0]);
        assert!(detail.contains("Sandbox: sandbox_receipt_1"));
        assert!(detail.contains("PR: https://github.com/org/repo/pull/7"));
        assert!(detail.contains("Runtime: 16m"));
        assert!(detail.contains("Agent: Fixed the flake"));
        for banned in ["Daytona", "daytona"] {
            assert!(
                !detail.contains(banned),
                "the job card must not brand the sandbox operator: {banned}"
            );
        }
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

    /// Launcher fixture for sweep/reconcile tests: records teardowns and
    /// lists a configurable sandbox set; never touches a network.
    struct SweepLauncher {
        torn_down: std::sync::Mutex<Vec<String>>,
        listed: Vec<LabeledSandbox>,
    }

    impl SweepLauncher {
        fn new(listed: Vec<LabeledSandbox>) -> Self {
            Self {
                torn_down: std::sync::Mutex::new(Vec::new()),
                listed,
            }
        }

        fn torn_down(&self) -> Vec<String> {
            self.torn_down
                .lock()
                .map(|ids| ids.clone())
                .unwrap_or_default()
        }
    }

    impl DaytonaLauncher for SweepLauncher {
        fn create_sandbox(&self, _job: &CloudJob) -> Result<SandboxReceipt> {
            bail!("no sandbox create in this fixture")
        }
        fn teardown(&self, receipt: &SandboxReceipt) -> Result<()> {
            if let Ok(mut ids) = self.torn_down.lock() {
                ids.push(receipt.sandbox_id.clone());
            }
            Ok(())
        }
        fn list_job_sandboxes(&self) -> Result<Vec<LabeledSandbox>> {
            Ok(self.listed.clone())
        }
    }

    fn stored_job(status: CloudJobStatus, created_unix: u64) -> CloudJob {
        CloudJob {
            id: "cloud_00000000000000e1".to_string(),
            kind: "cloud".to_string(),
            status,
            prompt: "fix the flake".to_string(),
            forge: Forge::Github,
            remote_name: "github".to_string(),
            remote_url: "https://github.com/org/repo.git".to_string(),
            branch: "codewhale/cloud-1".to_string(),
            confirmed: true,
            sandbox_id: None,
            pr_url: None,
            refusal: None,
            note: "n".to_string(),
            created_unix,
            base_branch: None,
            head_sha: None,
            agent_summary: None,
            finished_unix: None,
            sandbox_pending: false,
        }
    }

    #[test]
    fn proposal_and_job_cards_never_carry_a_provider_brand() {
        // The proposal note is user copy (it rides `/dispatch show` and the
        // CLI card from the moment a job is proposed) — it names the
        // Codewhale cloud agent, never the sandbox operator.
        let plan = DispatchPlan {
            prompt: "offload me".to_string(),
            remote: SelectedRemote {
                forge: Forge::Github,
                name: "github".to_string(),
                url: "https://github.com/org/repo.git".to_string(),
            },
            branch: "codewhale/cloud-x".to_string(),
        };
        let note = proposal_note(&plan);
        assert!(note.contains("cloud-agent"), "{note}");
        let mut job = stored_job(CloudJobStatus::Proposed, 1);
        job.note = note.clone();
        let card = format_job(&job);
        for banned in ["Daytona", "daytona"] {
            assert!(
                !card.contains(banned),
                "the job card must not brand the sandbox operator: {banned}"
            );
        }
        assert!(card.contains("cloud-agent"));
        // And the list view (format_job_list) over the same record.
        let listing = format_job_list(&[job]);
        for banned in ["Daytona", "daytona"] {
            assert!(
                !listing.contains(banned),
                "listing must not brand: {banned}"
            );
        }
    }

    #[test]
    fn harness_client_budget_covers_the_declared_turn_budget() {
        let hour = HarnessCommand {
            argv: vec!["codewhale".to_string()],
            cwd: SANDBOX_WORKSPACE.to_string(),
            timeout_secs: 3_600,
        };
        let budget = LiveDaytonaLauncher::harness_client_budget_secs(&hour);
        assert!(
            budget >= u64::from(hour.timeout_secs),
            "the client budget must cover the declared budget ({budget} < {})",
            hour.timeout_secs
        );
        assert!(
            budget > LiveDaytonaLauncher::CONTROL_PLANE_TIMEOUT_SECS,
            "an hour-long dispatched turn must not ride the 120s control-plane client"
        );
        // The budget scales with the declared timeout, not a fixed cap.
        let double = HarnessCommand {
            timeout_secs: 7_200,
            ..hour.clone()
        };
        assert!(LiveDaytonaLauncher::harness_client_budget_secs(&double) >= 7_200);
        // Short helper commands (collect_patch's git probes) keep a sane
        // bounded budget too.
        let probe = HarnessCommand {
            timeout_secs: 30,
            ..hour
        };
        assert!(LiveDaytonaLauncher::harness_client_budget_secs(&probe) >= 30);
    }

    #[test]
    fn save_unless_canceled_refuses_to_resurrect_a_canceled_record() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let mut job = stored_job(CloudJobStatus::Running, 10_000_000);
        store.save(&job).unwrap();
        // A phase save while the record is still active goes through.
        job.note = "phase save".to_string();
        assert!(store.save_unless_canceled(&job).unwrap());
        assert_eq!(store.load(&job.id).unwrap().note, "phase save");
        // The user cancels; the runner's next phase save must be refused and
        // leave the cancellation exactly as written.
        let mut canceled = store.load(&job.id).unwrap();
        canceled.status = CloudJobStatus::Canceled;
        canceled.note = "Canceled locally".to_string();
        canceled.finished_unix = Some(10_000_060);
        store.save(&canceled).unwrap();
        let mut stale_runner_copy = job.clone();
        stale_runner_copy.status = CloudJobStatus::OpeningPr;
        stale_runner_copy.note = "the runner's read-modify-write".to_string();
        assert!(!store.save_unless_canceled(&stale_runner_copy).unwrap());
        let persisted = store.load(&job.id).unwrap();
        assert_eq!(persisted.status, CloudJobStatus::Canceled);
        assert_eq!(persisted.note, "Canceled locally");
        assert_eq!(persisted.finished_unix, Some(10_000_060));
    }

    #[test]
    fn sweep_fails_stale_active_jobs_and_tears_down_their_sandboxes() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let now = 10_000_000_u64;
        // Stale: active well past the harness budget plus slack.
        let mut stale = stored_job(CloudJobStatus::Running, now - STALE_ACTIVE_JOB_SECS - 60);
        stale.sandbox_id = Some("sandbox_stale".to_string());
        store.save(&stale).unwrap();
        // Fresh: active but young; must be left exactly as it is.
        let mut fresh = stored_job(CloudJobStatus::Launching, now - 60);
        fresh.id = "cloud_00000000000000e2".to_string();
        fresh.sandbox_id = Some("sandbox_fresh".to_string());
        store.save(&fresh).unwrap();
        // Terminal: old but already done; never touched.
        let mut done = stored_job(CloudJobStatus::Done, now - STALE_ACTIVE_JOB_SECS * 2);
        done.id = "cloud_00000000000000e3".to_string();
        store.save(&done).unwrap();

        let launcher = SweepLauncher::new(Vec::new());
        let swept = sweep_stale_jobs(&store, &launcher, now);
        assert_eq!(swept.len(), 1, "only the stale active job is swept");
        assert_eq!(swept[0].id, "cloud_00000000000000e1");
        let record = store.load("cloud_00000000000000e1").unwrap();
        assert_eq!(record.status, CloudJobStatus::Failed);
        assert_eq!(record.finished_unix, Some(now));
        assert!(record.note.contains("startup sweep"));
        assert!(record.note.contains("teardown was attempted"));
        assert_eq!(launcher.torn_down(), vec!["sandbox_stale".to_string()]);
        // The untouched records keep their state.
        assert_eq!(
            store.load("cloud_00000000000000e2").unwrap().status,
            CloudJobStatus::Launching
        );
        assert_eq!(
            store.load("cloud_00000000000000e3").unwrap().status,
            CloudJobStatus::Done
        );
    }

    #[test]
    fn reconcile_deletes_sandboxes_for_terminal_or_absent_jobs_and_keeps_active() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let now = 10_000_000_u64;
        // Terminal job: its sandbox must go.
        let mut terminal = stored_job(CloudJobStatus::Canceled, now - 600);
        terminal.id = "cloud_00000000000000f1".to_string();
        terminal.sandbox_id = Some("sandbox_terminal".to_string());
        store.save(&terminal).unwrap();
        // Active job: its sandbox must stay.
        let mut active = stored_job(CloudJobStatus::Running, now - 60);
        active.id = "cloud_00000000000000f2".to_string();
        store.save(&active).unwrap();

        let launcher = SweepLauncher::new(vec![
            LabeledSandbox {
                sandbox_id: "sandbox_terminal".to_string(),
                job_id: Some("cloud_00000000000000f1".to_string()),
            },
            LabeledSandbox {
                sandbox_id: "sandbox_active".to_string(),
                job_id: Some("cloud_00000000000000f2".to_string()),
            },
            // Labeled for a job that no longer exists in the store.
            LabeledSandbox {
                sandbox_id: "sandbox_ghost".to_string(),
                job_id: Some("cloud_0000000000000bad".to_string()),
            },
            // No usable job label at all.
            LabeledSandbox {
                sandbox_id: "sandbox_unlabeled".to_string(),
                job_id: None,
            },
        ]);
        let report = reconcile_sandboxes(&store, &launcher).unwrap();
        assert_eq!(report.deleted.len(), 3);
        assert!(report.deleted.contains(&"sandbox_terminal".to_string()));
        assert!(report.deleted.contains(&"sandbox_ghost".to_string()));
        assert!(report.deleted.contains(&"sandbox_unlabeled".to_string()));
        assert_eq!(report.live, 1);
        assert!(!launcher.torn_down().contains(&"sandbox_active".to_string()));
    }

    #[test]
    fn cancel_deletes_an_unrecorded_sandbox_by_label() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let mut pending = stored_job(CloudJobStatus::Running, 10_000_000);
        // The create POST landed but its response never arrived: the intent
        // is persisted and the id is unknowable — only the label can find it.
        pending.sandbox_pending = true;
        store.save(&pending).unwrap();
        let launcher = SweepLauncher::new(vec![
            LabeledSandbox {
                sandbox_id: "sandbox_lost".to_string(),
                job_id: Some(pending.id.clone()),
            },
            LabeledSandbox {
                sandbox_id: "sandbox_other".to_string(),
                job_id: Some("cloud_00000000000000f9".to_string()),
            },
        ]);
        let canceled = cancel_job(&store, &pending.id, &launcher).unwrap();
        assert_eq!(canceled.status, CloudJobStatus::Canceled);
        assert!(canceled.note.contains("unrecorded"));
        assert!(canceled.note.contains("torn down"));
        assert_eq!(
            launcher.torn_down(),
            vec!["sandbox_lost".to_string()],
            "cancel deletes only this job's labeled sandbox"
        );
    }

    #[test]
    fn quit_warning_names_live_jobs_and_stays_quiet_otherwise() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        assert_eq!(live_job_quit_warning(&store), None);
        let mut live = stored_job(CloudJobStatus::OpeningPr, 10_000_000);
        live.id = "cloud_00000000000000aa".to_string();
        store.save(&live).unwrap();
        let warning = live_job_quit_warning(&store).expect("live job must warn");
        assert!(warning.contains("cloud_00000000000000aa"));
        assert!(warning.contains("/dispatch cancel"));
        assert!(!warning.contains("Daytona"));
        let mut done = stored_job(CloudJobStatus::Done, 10_000_000);
        done.id = "cloud_00000000000000ab".to_string();
        store.save(&done).unwrap();
        let mut dead = stored_job(CloudJobStatus::Failed, 10_000_000);
        dead.id = "cloud_00000000000000ac".to_string();
        store.save(&dead).unwrap();
        // Still exactly one live job after adding terminal siblings.
        let warning = live_job_quit_warning(&store).expect("live job must warn");
        assert!(warning.contains("cloud_00000000000000aa"));
        assert!(!warning.contains("cloud_00000000000000ab"));
        assert!(!warning.contains("cloud_00000000000000ac"));
    }
}
