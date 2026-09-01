//! Cloud-dispatch remote runner: confirmed job → sandbox → forge PR.
//!
//! This module is the engine behind `confirm_job` /
//! [`crate::cloud_dispatch::execute_dispatch`] with `confirm`: it drives one
//! cloud job from `launching` through a Codewhale-operated sandbox to a pull
//! request on the target forge (`github` | `cnb` | `gitee`), then tears the
//! sandbox down on completion, failure, or cancellation. The persisted
//! contract, credential discovery, and fail-closed membership gate stay in
//! [`crate::cloud_dispatch`]; nothing here re-implements them.
//!
//! Invariants:
//!
//! - one harness: the sandbox runs the same `codewhale exec --auto`
//!   one-shot entry every local non-interactive caller uses (one
//!   `Engine::run_turn` inside); the runner itself runs no model turn.
//! - credentials never widen: the sandbox credential stays inside
//!   [`crate::cloud_dispatch::LiveDaytonaLauncher`]; forge tokens are read
//!   from Codewhale service slots only at PR-open time and are never
//!   printed, logged, or persisted into job records.
//! - fail closed: every phase that cannot honestly complete records a
//!   `failed` (or keeps `canceled`) job with a truthful note; a PR URL is
//!   never invented.
//! - teardown always runs: cancel, failure, and success all attempt sandbox
//!   teardown; the job note records whether it succeeded.
//! - orphans reconcile: the TUI detaches this runner, so quitting the TUI
//!   can orphan a live sandbox. The job record persists a create intent
//!   before the POST, every sandbox is labeled with its job id, and
//!   [`startup_reconcile`] fails stale active jobs and deletes labeled
//!   sandboxes whose job no longer needs them.

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};

use crate::cloud_dispatch::{
    self, CloudJob, CloudJobStatus, CloudJobStore, DaytonaLauncher, Forge, HarnessCommand,
    PatchReceipt, SANDBOX_WORKSPACE, SandboxReceipt, sanitize_error, unix_timestamp,
    validate_outbound_origin,
};
use crate::dependencies::ExternalTool;

/// Ceiling for PR titles (forges truncate longer titles).
const MAX_TITLE_CHARS: usize = 96;
/// Ceiling for the PR body, kept well under forge limits.
const MAX_BODY_CHARS: usize = 6_000;
/// Harness timeout for one cloud-agent turn.
const HARNESS_TIMEOUT_SECS: u32 = 3_600;

/// The pull request the forge opened (or that `gh` reported).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrOpened {
    pub url: String,
    /// SHA actually applied and pushed — not the sandbox `patch.head_sha`.
    pub head_sha: String,
}

/// Forge seam: raise the agent's branch and open the PR. Tests inject a
/// recorder; production uses [`LiveForgePr`].
pub trait ForgePr {
    fn open(&self, job: &CloudJob, patch: &PatchReceipt) -> Result<PrOpened>;
}

/// Run one confirmed job end to end.
///
/// Requires the job to be `launching` (or `running`, for a resumed runner):
/// a `proposed` job is refused — confirmation is the caller's explicit act,
/// never the runner's. Every phase persists its transition so `/dispatch
/// show` and `/jobs` stream real progress, and a `canceled` record at any
/// checkpoint stops the run and tears the sandbox down.
pub fn run_confirmed_job(
    store: &CloudJobStore,
    id: &str,
    launcher: &dyn DaytonaLauncher,
    forge: &dyn ForgePr,
) -> Result<CloudJob> {
    let mut job = store.load(id)?;
    if !matches!(
        job.status,
        CloudJobStatus::Launching | CloudJobStatus::Running
    ) {
        bail!(
            "Cloud job {id} is {} and cannot be run; confirm it first with `/dispatch confirm {id}`.",
            status_word(job.status)
        );
    }
    match drive(store, &mut job, launcher, forge) {
        Ok(()) => store.load(id),
        Err(error) => {
            let message = sanitize_error(&error.to_string());
            // Re-load before writing any failure: a job the user canceled
            // stays canceled — a later run error must not overwrite the
            // user's terminal word with `failed`. The error is appended to
            // the note instead. And if a cancel lands inside the load→save
            // span of the failure write, the disk record (already
            // `canceled`, with the user's note) wins and is left alone.
            let current = match store.load(id) {
                Ok(mut record) if record.status == CloudJobStatus::Canceled => {
                    record.finished_unix =
                        Some(record.finished_unix.unwrap_or_else(unix_timestamp));
                    record.note = format!("{}. Run error after cancel: {message}", record.note);
                    let _ = store.save(&record);
                    record
                }
                loaded => {
                    let mut failed = loaded.unwrap_or_else(|_| job.clone());
                    failed.status = CloudJobStatus::Failed;
                    failed.refusal = Some(message.clone());
                    failed.finished_unix = Some(unix_timestamp());
                    failed.note = format!("Cloud agent run failed closed. {message}");
                    match store.save_unless_canceled(&failed) {
                        Ok(true) => failed,
                        _ => store.load(id).unwrap_or(failed),
                    }
                }
            };
            teardown_best_effort(launcher, &current);
            Err(error)
        }
    }
}

/// Background runner used by the CLI and TUI confirm paths. The store is
/// the source of truth; the thread result is intentionally dropped
/// (failures are recorded inside the job record). The CLI joins the handle
/// before exiting so a confirmed run is never orphaned mid-flight; the TUI
/// detaches it (the job record keeps the truth across restarts, and
/// `/dispatch cancel` tears a live sandbox down at any time).
pub fn spawn_confirmed_runner(
    store: CloudJobStore,
    id: String,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("cw-dispatch-{id}"))
        .spawn(move || {
            let launcher = cloud_dispatch::LiveDaytonaLauncher;
            let forge = LiveForgePr;
            let _ = run_confirmed_job(&store, &id, &launcher, &forge);
        })
        .ok()
}

/// Best-effort startup reconciliation for the detached runner.
///
/// The TUI spawns [`spawn_confirmed_runner`] detached, so quitting the TUI
/// or crashing mid-run orphans the record (and possibly a billing sandbox)
/// with nothing to reconcile it. Two passes, in order:
///
/// 1. [`cloud_dispatch::sweep_stale_jobs`] — active records older than the
///    declared harness budget plus slack are failed and their recorded
///    sandboxes torn down;
/// 2. [`cloud_dispatch::reconcile_sandboxes`] — any dispatch-labeled
///    sandbox whose job is terminal or absent from the store is deleted by
///    label, covering creates whose id was never recorded.
///
/// Never fatal and never blocks the caller's critical path beyond the
/// launcher's own bounded HTTP budget; returns a human receipt for the log
/// (empty when there was nothing to do).
pub fn startup_reconcile(store: &CloudJobStore, launcher: &dyn DaytonaLauncher) -> String {
    let swept = cloud_dispatch::sweep_stale_jobs(store, launcher, cloud_dispatch::unix_timestamp());
    let mut lines = Vec::new();
    for job in &swept {
        lines.push(format!(
            "cloud dispatch startup sweep: job {} marked stale (failed) and its sandbox teardown attempted",
            job.id
        ));
    }
    match cloud_dispatch::reconcile_sandboxes(store, launcher) {
        Ok(report) if !report.deleted.is_empty() => {
            lines.push(format!(
                "cloud dispatch label reconcile: deleted orphaned sandbox(es) {}",
                report.deleted.join(", ")
            ));
        }
        Ok(_) => {}
        Err(error) => lines.push(format!(
            "cloud dispatch label reconcile skipped: {}",
            sanitize_error(&error.to_string())
        )),
    }
    lines.join("\n")
}

fn drive(
    store: &CloudJobStore,
    job: &mut CloudJob,
    launcher: &dyn DaytonaLauncher,
    forge: &dyn ForgePr,
) -> Result<()> {
    // Launching → Running: create the sandbox. The intent record goes down
    // BEFORE the POST: if the create response is slow and the client gives
    // up (or the process dies), the sandbox may still come into being with
    // no recorded id — `sandbox_pending` is what cancel and the label
    // reconciler use to find and delete it by label.
    //
    // Every phase save below is cancel-authoritative
    // (`save_unless_canceled`): a cancel that lands while a phase is in
    // flight wins over the runner's read-modify-write, so a canceled job
    // can never be resurrected into a later phase — above all never into
    // the branch push / PR open.
    job.sandbox_pending = true;
    store.save(job)?;
    let receipt = launcher.create_sandbox(job)?;
    job.status = CloudJobStatus::Running;
    job.sandbox_pending = false;
    job.sandbox_id = Some(receipt.sandbox_id.clone());
    job.note = format!(
        "Sandbox {} created; the Codewhale cloud agent turn is running.",
        receipt.sandbox_id
    );
    if !store.save_unless_canceled(job)? {
        return finish_canceled(store, job, launcher, &receipt);
    }

    launcher.wait_ready(&receipt)?;
    let clone_url = cloud_dispatch::validate_git_remote_url(&job.remote_url)?;
    launcher.clone_repository(&receipt, &clone_url, SANDBOX_WORKSPACE)?;
    if cancel_requested(store, job)? {
        return finish_canceled(store, job, launcher, &receipt);
    }

    // One agent turn through the standard one-shot harness entry.
    let output = launcher.run_harness(&receipt, &harness_command(job))?;
    job.agent_summary = Some(summary_line(&output));
    if cancel_requested(store, job)? {
        return finish_canceled(store, job, launcher, &receipt);
    }

    // Running → OpeningPr: collect the agent's work product.
    let patch = launcher.collect_patch(&receipt)?;
    job.status = CloudJobStatus::OpeningPr;
    job.base_branch = Some(patch.base_branch.clone());
    job.head_sha = Some(patch.head_sha.clone()); // replaced with the pushed sha after `forge.open`
    job.note = format!(
        "Agent turn complete ({}); raising branch {} and opening the PR on {}.",
        patch.summary,
        job.branch,
        job.forge.as_str()
    );
    if !store.save_unless_canceled(job)? {
        return finish_canceled(store, job, launcher, &receipt);
    }

    // OpeningPr → Done: push the branch and open the PR. The cancel check
    // is the last gate before money-adjacent side effects on the forge.
    if cancel_requested(store, job)? {
        return finish_canceled(store, job, launcher, &receipt);
    }
    let opened = forge.open(job, &patch)?;
    job.status = CloudJobStatus::Done;
    job.pr_url = Some(opened.url.clone());
    job.head_sha = Some(opened.head_sha.clone());
    job.finished_unix = Some(unix_timestamp());
    job.note = format!(
        "Cloud agent finished; PR opened at {}. {}",
        opened.url,
        teardown_note(launcher, &receipt)
    );
    if !store.save_unless_canceled(job)? {
        // A cancel landed while the PR was opening. The PR may well exist —
        // keep its URL and say exactly that rather than claiming success or
        // silently dropping the receipt.
        let mut canceled = store.load(&job.id)?;
        canceled.pr_url = job.pr_url.clone();
        canceled.agent_summary = job.agent_summary.clone();
        canceled.finished_unix = Some(canceled.finished_unix.unwrap_or_else(unix_timestamp));
        canceled.note = format!(
            "Canceled as the PR was opening; it may still have landed at {}. {}",
            opened.url,
            teardown_note(launcher, &receipt)
        );
        *job = canceled.clone();
        return store.save(&canceled).map(|_| ());
    }
    Ok(())
}

/// True when the user canceled the job mid-run.
fn cancel_requested(store: &CloudJobStore, job: &CloudJob) -> Result<bool> {
    Ok(store.load(&job.id)?.status == CloudJobStatus::Canceled)
}

fn finish_canceled(
    store: &CloudJobStore,
    job: &mut CloudJob,
    launcher: &dyn DaytonaLauncher,
    receipt: &SandboxReceipt,
) -> Result<()> {
    let mut canceled = store.load(&job.id)?;
    canceled.agent_summary = job.agent_summary.clone();
    canceled.finished_unix = Some(canceled.finished_unix.unwrap_or_else(unix_timestamp));
    canceled.note = format!("Canceled mid-run. {}", teardown_note(launcher, receipt));
    *job = canceled.clone();
    store.save(&canceled)
}

fn teardown_best_effort(launcher: &dyn DaytonaLauncher, job: &CloudJob) {
    if let Some(sandbox_id) = job.sandbox_id.clone() {
        let receipt = SandboxReceipt {
            sandbox_id,
            toolbox_url: None,
        };
        let _ = launcher.teardown(&receipt);
    }
}

fn teardown_note(launcher: &dyn DaytonaLauncher, receipt: &SandboxReceipt) -> String {
    match launcher.teardown(receipt) {
        Ok(()) => "The sandbox was torn down.".to_string(),
        Err(error) => format!(
            "Sandbox teardown failed and may need a retry: {}",
            sanitize_error(&error.to_string())
        ),
    }
}

/// The exact harness invocation the sandbox runs: the one-shot
/// `codewhale exec --auto` entry — the same single-`Engine::run_turn` path
/// local non-interactive callers use, never a second engine.
pub fn harness_command(job: &CloudJob) -> HarnessCommand {
    HarnessCommand {
        argv: vec![
            "codewhale".to_string(),
            "exec".to_string(),
            "--auto".to_string(),
            job.prompt.clone(),
        ],
        cwd: SANDBOX_WORKSPACE.to_string(),
        timeout_secs: HARNESS_TIMEOUT_SECS,
    }
}

/// First non-empty line of harness output, bounded for notes and PR bodies.
pub fn summary_line(output: &str) -> String {
    // Redacted first: the sandbox env carries the account machine token, and
    // harness output must not be able to echo it into the job record.
    crate::cloud_dispatch::redact_machine_tokens(output)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
        .unwrap_or_else(|| "cloud agent turn completed".to_string())
}

/// Owner/repo slug for a forge remote URL, or `None` when the URL does not
/// match the job's forge. Both https and `git@host:owner/repo.git` shapes
/// are accepted; a trailing `.git` is stripped.
pub fn forge_slug(forge: Forge, remote_url: &str) -> Option<String> {
    if cloud_dispatch::classify_url(remote_url) != Some(forge) {
        return None;
    }
    // `https://host/owner/repo.git` or `git@host:owner/repo.git`
    let repo_path = if let Some((_, rest)) = remote_url.split_once("://") {
        // Strip the host: the slug is the two path segments after it.
        rest.split_once('/')?.1
    } else {
        remote_url.split_once(':')?.1
    };
    let segments: Vec<&str> = repo_path.split('/').collect();
    if segments.len() < 2 {
        return None;
    }
    let repo = segments[segments.len() - 1].trim_end_matches(".git");
    let owner = segments[segments.len() - 2];
    if owner.is_empty() || repo.is_empty() || repo == owner {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Truthful PR title: names the agent and its own summary.
pub fn compose_pr_title(job: &CloudJob, patch: &PatchReceipt) -> String {
    let summary = if patch.summary.trim().is_empty() {
        one_line(&job.prompt, 72)
    } else {
        one_line(&patch.summary, 72)
    };
    one_line(&format!("codewhale cloud: {summary}"), MAX_TITLE_CHARS)
}

/// Truthful PR body: what the agent did, the receipts Codewhale has, and an
/// explicit No-Issue line (no tracked issue; the cloud job is the record).
/// Sandbox-provider names never appear — the operator is Codewhale.
pub fn compose_pr_body(job: &CloudJob, patch: &PatchReceipt) -> String {
    compose_pr_body_for_head(job, patch, &patch.head_sha)
}

/// PR body whose `Head:` is the sha actually applied/pushed.
pub fn compose_pr_body_for_head(job: &CloudJob, patch: &PatchReceipt, head_sha: &str) -> String {
    let summary = if patch.summary.trim().is_empty() {
        "(the agent's commit list is the record)".to_string()
    } else {
        patch.summary.trim().to_string()
    };
    let body = format!(
        "Automated change by a Codewhale cloud agent.\n\n\
         ## What the agent did\n{summary}\n\n\
         ## Task\n{}\n\n\
         ## Receipts\n\
         - Cloud job: {}\n\
         - Sandbox: {}\n\
         - Branch: `{}` (base `{}`)\n\
         - Head: `{}`\n\n\
         No-Issue: cloud dispatch {} (no tracked issue; receipts above)",
        job.prompt,
        job.id,
        job.sandbox_id.as_deref().unwrap_or("(pending)"),
        job.branch,
        patch.base_branch,
        head_sha,
        job.id,
    );
    one_line(&body, MAX_BODY_CHARS)
}

/// `gh pr create` argv for the GitHub path. The body rides in a file so the
/// prompt text never appears in `ps` output.
pub fn gh_pr_create_argv(
    slug: &str,
    base: &str,
    head: &str,
    title: &str,
    body_file: &str,
) -> Vec<String> {
    vec![
        "pr".to_string(),
        "create".to_string(),
        "--repo".to_string(),
        slug.to_string(),
        "--base".to_string(),
        base.to_string(),
        "--head".to_string(),
        head.to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body-file".to_string(),
        body_file.to_string(),
    ]
}

/// Gitee v5 pull-request endpoint for a slug.
pub fn gitee_pr_url(slug: &str) -> String {
    format!("https://gitee.com/api/v5/repos/{slug}/pulls")
}

/// CNB pull-request endpoint for a slug.
pub fn cnb_pr_url(slug: &str) -> String {
    format!("https://api.cnb.cool/{slug}/-/pulls")
}

/// Production forge opener: local git for the branch push, then the forge's
/// own API surface for the PR (gh for GitHub; REST for CNB and Gitee with
/// service-slot tokens). Fails closed on missing tooling or tokens — the
/// branch push is not rolled back, and no PR URL is ever invented.
pub struct LiveForgePr;

impl ForgePr for LiveForgePr {
    fn open(&self, job: &CloudJob, patch: &PatchReceipt) -> Result<PrOpened> {
        let slug = forge_slug(job.forge, &job.remote_url).ok_or_else(|| {
            anyhow!(
                "the {} remote {} does not resolve to an owner/repo slug",
                job.forge.as_str(),
                job.remote_url
            )
        })?;
        let title = compose_pr_title(job, patch);
        let dir = tempfile::tempdir().context("could not stage the cloud agent branch")?;
        let head_sha = prepare_branch(job, patch, dir.path())?;
        push_branch(&dir.path().join("repo"), &job.remote_url, &job.branch)?;
        let body = compose_pr_body_for_head(job, patch, &head_sha);
        let url = match job.forge {
            Forge::Github => open_pr_github(&slug, job, patch, &title, &body)?,
            Forge::Gitee => open_pr_gitee(&slug, job, patch, &title, &body)?,
            Forge::Cnb => open_pr_cnb(&slug, job, patch, &title, &body)?,
        };
        Ok(PrOpened { url, head_sha })
    }
}

/// Shallow-clone the target repository, apply the agent's patch on a branch,
/// and return the head sha. Plain git subprocess work; never forceful.
fn prepare_branch(job: &CloudJob, patch: &PatchReceipt, dir: &Path) -> Result<String> {
    let remote_url = cloud_dispatch::validate_git_remote_url(&job.remote_url)?;
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--depth",
            "50",
            "--",
            &remote_url,
            &dir.join("repo").to_string_lossy(),
        ],
    )
    .context("could not clone the target repository for the cloud agent branch")?;
    let repo = dir.join("repo");
    let patch_path = dir.join("agent.patch");
    std::fs::write(&patch_path, &patch.patch).context("could not stage the agent patch")?;
    git(
        Some(&repo),
        &["config", "user.name", "Codewhale Cloud Agent"],
    )
    .context("could not set the agent identity")?;
    git(
        Some(&repo),
        &["config", "user.email", "cloud-agent@codewhale.invalid"],
    )
    .context("could not set the agent identity")?;
    git(Some(&repo), &["checkout", "--quiet", "-b", &job.branch])
        .context("could not create the cloud agent branch")?;
    git(
        Some(&repo),
        &["am", "--quiet", "--3way", &patch_path.to_string_lossy()],
    )
    .context("the agent patch did not apply cleanly onto the target branch")?;
    git(Some(&repo), &["rev-parse", "HEAD"]).map(|out| out.trim().to_string())
}

/// Push the prepared branch. Plain push only — `--force` is never passed, so
/// an existing branch that is not a fast-forward fails closed instead of
/// rewriting the target's history.
fn push_branch(repo: &Path, remote_url: &str, branch: &str) -> Result<()> {
    let remote_url = cloud_dispatch::validate_git_remote_url(remote_url)?;
    if cloud_dispatch::is_forge_default_branch(branch) {
        bail!("refusing to push onto the forge default branch {branch}");
    }
    if looks_like_network_remote(&remote_url) && cloud_dispatch::classify_url(&remote_url).is_none()
    {
        bail!("refusing to push to a non-forge remote");
    }
    if remote_branch_exists(&remote_url, branch)? {
        bail!("refusing to update existing remote branch {branch}");
    }
    git(
        Some(repo),
        &[
            "push",
            "--quiet",
            "--",
            &remote_url,
            &format!("HEAD:refs/heads/{branch}"),
        ],
    )
    .map(|_| ())
    .context(
        "could not push the cloud agent branch (it may need credentials or the branch may have moved)",
    )
}

fn looks_like_network_remote(url: &str) -> bool {
    url.contains("://") || url.contains('@')
}

fn remote_branch_exists(remote_url: &str, branch: &str) -> Result<bool> {
    let listing =
        git(None, &["ls-remote", "--heads", "--", remote_url, branch]).unwrap_or_default();
    Ok(listing
        .lines()
        .any(|line| line.contains(&format!("refs/heads/{branch}"))))
}

fn git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .args(args)
        .output()
        .context("failed to start git for the cloud agent branch")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            sanitize_error(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn open_pr_github(
    slug: &str,
    job: &CloudJob,
    patch: &PatchReceipt,
    title: &str,
    body: &str,
) -> Result<String> {
    let body_dir = tempfile::tempdir().context("could not stage the PR body")?;
    let body_file = body_dir.path().join("body.md");
    std::fs::write(&body_file, body).context("could not write the PR body")?;
    let mut command = crate::dependencies::Gh::command()
        .ok_or_else(|| anyhow!("the GitHub pull request needs the gh CLI on PATH"))?;
    command.args(gh_pr_create_argv(
        slug,
        &patch.base_branch,
        &job.branch,
        title,
        &body_file.to_string_lossy(),
    ));
    let output = command
        .output()
        .context("failed to start gh for the pull request")?;
    if !output.status.success() {
        bail!(
            "gh pr create failed: {}",
            sanitize_error(&String::from_utf8_lossy(&output.stderr))
        );
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !url.starts_with("https://") {
        bail!("gh did not report a pull request URL; refusing to invent one.");
    }
    Ok(url)
}

fn open_pr_gitee(
    slug: &str,
    job: &CloudJob,
    patch: &PatchReceipt,
    title: &str,
    body: &str,
) -> Result<String> {
    let token = read_service_token("gitee").ok_or_else(|| {
        anyhow!("a Gitee access token is not configured in the Codewhale service slot; the branch was pushed but no pull request was opened")
    })?;
    let url = validate_outbound_origin(&gitee_pr_url(slug))?;
    let response = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("could not initialize the Gitee client")?
        .post(url)
        .form(&[
            ("access_token", token.as_str()),
            ("title", title),
            ("head", job.branch.as_str()),
            ("base", patch.base_branch.as_str()),
            ("body", body),
        ])
        .send()
        .context("could not reach Gitee")?;
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        bail!("Gitee pull request create failed (HTTP {status}).");
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&text).context("Gitee returned invalid JSON")?;
    parsed
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| url.starts_with("https://"))
        .map(|url| url.to_string())
        .ok_or_else(|| anyhow!("Gitee did not report a pull request URL; refusing to invent one."))
}

fn open_pr_cnb(
    slug: &str,
    job: &CloudJob,
    patch: &PatchReceipt,
    title: &str,
    body: &str,
) -> Result<String> {
    let token = read_service_token("cnb").ok_or_else(|| {
        anyhow!("a CNB access token is not configured in the Codewhale service slot; the branch was pushed but no pull request was opened")
    })?;
    let url = validate_outbound_origin(&cnb_pr_url(slug))?;
    let response = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("could not initialize the CNB client")?
        .post(url)
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "title": title,
            "head": job.branch,
            "base": patch.base_branch,
            "body": body,
        }))
        .send()
        .context("could not reach CNB")?;
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        bail!("CNB pull request create failed (HTTP {status}).");
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&text).context("CNB returned invalid JSON")?;
    let number = parsed
        .get("number")
        .and_then(serde_json::Value::as_i64)
        .filter(|number| *number > 0)
        .ok_or_else(|| {
            anyhow!("CNB did not report a pull request number; refusing to invent a URL.")
        })?;
    Ok(format!("https://cnb.cool/{slug}/-/pulls/{number}"))
}

/// Read a forge token from the Codewhale service slot. Never logged.
fn read_service_token(slot: &str) -> Option<String> {
    codewhale_secrets::Secrets::auto_detect()
        .get(slot)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn one_line(value: &str, max: usize) -> String {
    let flat: String = value
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' {
                ' '
            } else {
                ch
            }
        })
        .collect();
    if flat.chars().count() <= max {
        flat
    } else {
        let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn status_word(status: CloudJobStatus) -> &'static str {
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

/// Callback fired at each launcher phase boundary (used by tests to
/// simulate mid-run cancellation).
pub type PhaseHook = Box<dyn Fn(&str) + Send + Sync>;

/// Recording launcher for offline tests. Records every phase by name and
/// replays canned results; `hook` fires at each phase boundary so tests can
/// simulate mid-run cancellation.
pub struct RecordingLauncher {
    sandbox_id: String,
    patch: PatchReceipt,
    calls: Mutex<Vec<String>>,
    pub hook: Option<PhaseHook>,
    /// Sandboxes reported by `list_job_sandboxes` (the reconciler seam).
    pub listed: Mutex<Vec<crate::cloud_dispatch::LabeledSandbox>>,
}

impl RecordingLauncher {
    pub fn new(sandbox_id: &str, patch: PatchReceipt) -> Self {
        Self {
            sandbox_id: sandbox_id.to_string(),
            patch,
            calls: Mutex::new(Vec::new()),
            hook: None,
            listed: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    fn record(&self, phase: &str) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(phase.to_string());
        }
        if let Some(hook) = self.hook.as_ref() {
            hook(phase);
        }
    }
}

impl DaytonaLauncher for RecordingLauncher {
    fn create_sandbox(&self, _job: &CloudJob) -> Result<SandboxReceipt> {
        self.record("create");
        Ok(SandboxReceipt {
            sandbox_id: self.sandbox_id.clone(),
            toolbox_url: Some("https://toolbox.example.test".to_string()),
        })
    }

    fn wait_ready(&self, _receipt: &SandboxReceipt) -> Result<()> {
        self.record("wait_ready");
        Ok(())
    }

    fn clone_repository(&self, _receipt: &SandboxReceipt, url: &str, path: &str) -> Result<()> {
        self.record("clone");
        assert_eq!(path, SANDBOX_WORKSPACE);
        assert!(
            url.starts_with("https://"),
            "live clone must be an https URL"
        );
        Ok(())
    }

    fn run_harness(&self, _receipt: &SandboxReceipt, _command: &HarnessCommand) -> Result<String> {
        self.record("harness");
        Ok("Fixed the flaky test and re-ran the suite.\nall green".to_string())
    }

    fn collect_patch(&self, _receipt: &SandboxReceipt) -> Result<PatchReceipt> {
        self.record("collect");
        Ok(self.patch.clone())
    }

    fn teardown(&self, _receipt: &SandboxReceipt) -> Result<()> {
        self.record("teardown");
        Ok(())
    }

    fn list_job_sandboxes(&self) -> Result<Vec<crate::cloud_dispatch::LabeledSandbox>> {
        self.record("list");
        Ok(self
            .listed
            .lock()
            .map(|listed| listed.clone())
            .unwrap_or_default())
    }
}

/// Recording forge opener for offline tests.
pub struct RecordingForgePr {
    pub url: String,
    opened: Mutex<Vec<String>>,
}

impl RecordingForgePr {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            opened: Mutex::new(Vec::new()),
        }
    }

    pub fn opened(&self) -> Vec<String> {
        self.opened
            .lock()
            .map(|opened| opened.clone())
            .unwrap_or_default()
    }
}

impl ForgePr for RecordingForgePr {
    fn open(&self, job: &CloudJob, patch: &PatchReceipt) -> Result<PrOpened> {
        if let Ok(mut opened) = self.opened.lock() {
            opened.push(format!("{}:{}", job.id, patch.head_sha));
        }
        Ok(PrOpened {
            url: self.url.clone(),
            head_sha: patch.head_sha.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_dispatch::{
        CloudJobStatus, CredentialSource, CredentialState, DispatchOutcome, GitRemote,
        MachineTokenState, execute_dispatch, plan_dispatch,
    };
    use std::sync::Arc;

    fn fixture_patch() -> PatchReceipt {
        PatchReceipt {
            base_branch: "main".to_string(),
            head_sha: "abc123def4567".to_string(),
            summary: "Fix the flaky dispatch test".to_string(),
            patch: "From abc123 Mon Sep 17 00:00:00 2001\nSubject: [PATCH] Fix the flake\n"
                .to_string(),
        }
    }

    fn confirmed_job(store: &CloudJobStore) -> CloudJob {
        let plan = plan_dispatch(
            &[GitRemote {
                name: "github".to_string(),
                url: "https://github.com/org/repo.git".to_string(),
            }],
            "open a PR that fixes the flake",
            Some(Forge::Github),
            Some("codewhale/cloud-runner-test"),
        )
        .unwrap();
        match execute_dispatch(
            store,
            plan,
            true,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
            &MachineTokenState::Present,
        )
        .unwrap()
        {
            DispatchOutcome::Accepted(job) => job,
            other => panic!("expected accept, got {other:?}"),
        }
    }

    #[test]
    fn recording_lifecycle_reaches_done_with_receipts_and_teardown() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let job = confirmed_job(&store);
        let launcher = RecordingLauncher::new("sandbox_runner_1", fixture_patch());
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/9");
        let finished = run_confirmed_job(&store, &job.id, &launcher, &forge).unwrap();

        assert_eq!(finished.status, CloudJobStatus::Done);
        assert_eq!(
            finished.pr_url.as_deref(),
            Some("https://github.com/org/repo/pull/9")
        );
        assert_eq!(finished.sandbox_id.as_deref(), Some("sandbox_runner_1"));
        assert_eq!(finished.base_branch.as_deref(), Some("main"));
        assert_eq!(finished.head_sha.as_deref(), Some("abc123def4567"));
        assert_eq!(
            finished.agent_summary.as_deref(),
            Some("Fixed the flaky test and re-ran the suite.")
        );
        assert!(finished.finished_unix.is_some());
        assert!(finished.note.contains("PR opened at"));
        assert!(finished.note.contains("torn down"));
        // Full protocol order, teardown last.
        assert_eq!(
            launcher.calls(),
            vec![
                "create",
                "wait_ready",
                "clone",
                "harness",
                "collect",
                "teardown"
            ]
        );
        assert_eq!(forge.opened(), vec![format!("{}:abc123def4567", job.id)]);
        // The persisted record streams the same truth.
        assert_eq!(store.load(&job.id).unwrap().status, CloudJobStatus::Done);
    }

    #[test]
    fn cancel_during_running_tears_down_and_never_opens_the_pr() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let store = CloudJobStore::from_path(root.clone());
        let job = confirmed_job(&store);
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/9");
        // Cancel lands while the harness turn is in flight.
        let cancel_root = root.clone();
        let cancel_id = job.id.clone();
        let canceled_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = canceled_seen.clone();
        let mut launcher = RecordingLauncher::new("sandbox_runner_2", fixture_patch());
        launcher.hook = Some(Box::new(move |phase| {
            if phase == "harness" && !seen.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let store = CloudJobStore::from_path(cancel_root.clone());
                let mut current = store.load(&cancel_id).unwrap();
                current.status = CloudJobStatus::Canceled;
                store.save(&current).unwrap();
            }
        }));
        let finished = run_confirmed_job(&store, &job.id, &launcher, &forge).unwrap();

        assert_eq!(finished.status, CloudJobStatus::Canceled);
        assert!(finished.pr_url.is_none());
        assert!(finished.note.contains("Canceled mid-run"));
        assert!(finished.note.contains("torn down"));
        // The run stopped before collect and the forge never fired; teardown ran.
        assert_eq!(
            launcher.calls(),
            vec!["create", "wait_ready", "clone", "harness", "teardown"]
        );
        assert!(forge.opened().is_empty());
        assert!(canceled_seen.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn cancel_between_clone_and_harness_never_starts_the_turn() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let store = CloudJobStore::from_path(root.clone());
        let job = confirmed_job(&store);
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/9");
        let mut launcher = RecordingLauncher::new("sandbox_runner_clone_cancel", fixture_patch());
        launcher.hook = Some(Box::new({
            let cancel = raw_cancel_from_hook(root, job.id.clone());
            move |phase| {
                if phase == "clone" {
                    cancel(phase);
                }
            }
        }));
        let finished = run_confirmed_job(&store, &job.id, &launcher, &forge).unwrap();
        assert_eq!(finished.status, CloudJobStatus::Canceled);
        assert!(finished.pr_url.is_none());
        let calls = launcher.calls();
        assert!(
            calls.contains(&"clone".to_string()),
            "clone must have run: {calls:?}"
        );
        assert!(
            !calls.contains(&"harness".to_string()),
            "harness must not run after a post-clone cancel: {calls:?}"
        );
        assert!(forge.opened().is_empty());
    }

    /// Cancels the job from inside a phase hook with a raw status flip (no)
    /// `finished_unix`), mirroring the reviewer's reproduction: cancel_job
    /// saves `canceled` while a phase is in flight.
    fn raw_cancel_from_hook(root: std::path::PathBuf, id: String) -> impl Fn(&str) + Send + Sync {
        move |_phase: &str| {
            let store = CloudJobStore::from_path(root.clone());
            let mut current = store.load(&id).unwrap();
            current.status = CloudJobStatus::Canceled;
            store.save(&current).unwrap();
        }
    }

    #[test]
    fn cancel_during_create_wins_over_the_post_create_save() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let store = CloudJobStore::from_path(root.clone());
        let job = confirmed_job(&store);
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/9");
        // The cancel lands while the create POST is in flight — before the
        // runner ever holds a receipt. The post-create phase save must
        // refuse to resurrect the run.
        let mut launcher = RecordingLauncher::new("sandbox_cancel_1", fixture_patch());
        launcher.hook = Some(Box::new({
            let cancel = raw_cancel_from_hook(root.clone(), job.id.clone());
            move |phase| {
                if phase == "create" {
                    cancel(phase);
                }
            }
        }));
        let finished = run_confirmed_job(&store, &job.id, &launcher, &forge).unwrap();

        assert_eq!(finished.status, CloudJobStatus::Canceled);
        assert!(finished.pr_url.is_none());
        assert!(finished.finished_unix.is_some());
        assert!(finished.note.contains("Canceled mid-run"));
        assert!(finished.note.contains("torn down"));
        // The run never reached readiness, the forge never fired, teardown ran.
        assert_eq!(launcher.calls(), vec!["create", "teardown"]);
        assert!(forge.opened().is_empty());
        let persisted = store.load(&job.id).unwrap();
        assert_eq!(persisted.status, CloudJobStatus::Canceled);
        assert!(persisted.finished_unix.is_some());
    }

    #[test]
    fn cancel_during_collect_blocks_the_branch_raise_and_the_pr() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let store = CloudJobStore::from_path(root.clone());
        let job = confirmed_job(&store);
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/9");
        // The cancel lands while the patch is being collected; the
        // OpeningPr phase save must refuse, so the branch is never raised
        // and the PR never opened.
        let mut launcher = RecordingLauncher::new("sandbox_cancel_2", fixture_patch());
        launcher.hook = Some(Box::new({
            let cancel = raw_cancel_from_hook(root.clone(), job.id.clone());
            move |phase| {
                if phase == "collect" {
                    cancel(phase);
                }
            }
        }));
        let finished = run_confirmed_job(&store, &job.id, &launcher, &forge).unwrap();

        assert_eq!(finished.status, CloudJobStatus::Canceled);
        assert!(finished.pr_url.is_none());
        assert!(finished.base_branch.is_none());
        assert!(finished.finished_unix.is_some());
        assert!(finished.note.contains("Canceled mid-run"));
        assert_eq!(
            launcher.calls(),
            vec![
                "create",
                "wait_ready",
                "clone",
                "harness",
                "collect",
                "teardown"
            ]
        );
        assert!(forge.opened().is_empty());
    }

    /// Launcher whose harness step fails; earlier phases record normally.
    struct HarnessFailsLauncher {
        calls: Mutex<Vec<String>>,
        hook: Option<PhaseHook>,
    }

    impl HarnessFailsLauncher {
        fn note(&self, phase: &str) {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(phase.to_string());
            }
            if let Some(hook) = self.hook.as_ref() {
                hook(phase);
            }
        }
    }

    impl DaytonaLauncher for HarnessFailsLauncher {
        fn create_sandbox(&self, _job: &CloudJob) -> Result<SandboxReceipt> {
            self.note("create");
            Ok(SandboxReceipt {
                sandbox_id: "sandbox_err_1".to_string(),
                toolbox_url: None,
            })
        }
        fn wait_ready(&self, _receipt: &SandboxReceipt) -> Result<()> {
            self.note("wait_ready");
            Ok(())
        }
        fn clone_repository(&self, _receipt: &SandboxReceipt, url: &str, path: &str) -> Result<()> {
            self.note("clone");
            assert_eq!(path, SANDBOX_WORKSPACE);
            assert!(url.starts_with("https://"));
            Ok(())
        }
        fn run_harness(
            &self,
            _receipt: &SandboxReceipt,
            _command: &HarnessCommand,
        ) -> Result<String> {
            self.note("harness");
            bail!("harness exploded")
        }
        fn teardown(&self, _receipt: &SandboxReceipt) -> Result<()> {
            self.note("teardown");
            Ok(())
        }
    }

    #[test]
    fn run_error_after_cancel_keeps_the_canceled_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let store = CloudJobStore::from_path(root.clone());
        let job = confirmed_job(&store);
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/9");
        // The cancel lands as the harness explodes: the failure arm must not
        // overwrite the user's `canceled` with `failed` — the error rides
        // along in the note.
        let mut launcher = HarnessFailsLauncher {
            calls: Mutex::new(Vec::new()),
            hook: None,
        };
        launcher.hook = Some(Box::new({
            let cancel = raw_cancel_from_hook(root.clone(), job.id.clone());
            move |phase| {
                if phase == "harness" {
                    cancel(phase);
                }
            }
        }));
        let error = run_confirmed_job(&store, &job.id, &launcher, &forge)
            .unwrap_err()
            .to_string();
        assert!(error.contains("harness exploded"), "{error}");

        let persisted = store.load(&job.id).unwrap();
        assert_eq!(
            persisted.status,
            CloudJobStatus::Canceled,
            "a user cancel survives a later run error"
        );
        assert!(persisted.note.contains("Run error after cancel"));
        assert!(persisted.note.contains("harness exploded"));
        assert!(persisted.refusal.is_none());
        assert!(persisted.pr_url.is_none());
        assert!(persisted.finished_unix.is_some());
        assert!(forge.opened().is_empty());
        let calls = launcher.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec!["create", "wait_ready", "clone", "harness", "teardown"]
        );
    }

    #[test]
    fn the_declared_harness_budget_fits_the_harness_client_budget() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let job = confirmed_job(&store);
        let command = harness_command(&job);
        assert_eq!(command.timeout_secs, HARNESS_TIMEOUT_SECS);
        assert!(
            cloud_dispatch::LiveDaytonaLauncher::harness_client_budget_secs(&command)
                >= u64::from(HARNESS_TIMEOUT_SECS),
            "the client that carries the harness turn must cover the declared hour"
        );
    }

    #[test]
    fn sandbox_intent_is_persisted_before_the_create_post() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let store = CloudJobStore::from_path(root.clone());
        let job = confirmed_job(&store);
        // Observed from inside the create phase: the intent must already be
        // on disk, so a create whose response never arrives is still
        // reconcilable by label.
        let intent_root = root.clone();
        let intent_id = job.id.clone();
        let seen_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = seen_pending.clone();
        let mut launcher = RecordingLauncher::new("sandbox_intent_1", fixture_patch());
        launcher.hook = Some(Box::new(move |phase| {
            if phase == "create" {
                let store = CloudJobStore::from_path(intent_root.clone());
                let current = store.load(&intent_id).unwrap();
                assert!(current.sandbox_pending, "intent must precede the POST");
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }));
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/11");
        let finished = run_confirmed_job(&store, &job.id, &launcher, &forge).unwrap();

        assert!(seen_pending.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(finished.status, CloudJobStatus::Done);
        assert!(!finished.sandbox_pending, "intent clears once the id lands");
        let persisted = store.load(&job.id).unwrap();
        assert!(!persisted.sandbox_pending);
        assert_eq!(persisted.sandbox_id.as_deref(), Some("sandbox_intent_1"));
    }

    #[test]
    fn startup_reconcile_sweeps_stale_jobs_and_deletes_orphan_sandboxes() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let job = confirmed_job(&store);
        // Age the record past the stale threshold and park it mid-run, the
        // state a quit/crash would leave behind.
        let mut stale = store.load(&job.id).unwrap();
        stale.status = CloudJobStatus::Running;
        stale.sandbox_id = Some("sandbox_orphan".to_string());
        stale.created_unix = stale
            .created_unix
            .saturating_sub(crate::cloud_dispatch::STALE_ACTIVE_JOB_SECS + 120);
        store.save(&stale).unwrap();
        // A sandbox labeled for a job that is not in the store at all.
        let launcher = RecordingLauncher::new("unused", fixture_patch());
        *launcher.listed.lock().unwrap() = vec![crate::cloud_dispatch::LabeledSandbox {
            sandbox_id: "sandbox_ghost".to_string(),
            job_id: Some("cloud_0000000000000bad".to_string()),
        }];

        let receipt = startup_reconcile(&store, &launcher);
        assert!(
            receipt.contains(&job.id),
            "receipt names the swept job: {receipt}"
        );
        assert!(
            receipt.contains("sandbox_ghost"),
            "receipt names deletions: {receipt}"
        );
        assert_eq!(store.load(&job.id).unwrap().status, CloudJobStatus::Failed);
        assert!(launcher.calls().contains(&"teardown".to_string()));
        assert!(launcher.calls().contains(&"list".to_string()));
    }

    #[test]
    fn only_confirmed_jobs_can_run() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let plan = plan_dispatch(
            &[GitRemote {
                name: "github".to_string(),
                url: "https://github.com/org/repo.git".to_string(),
            }],
            "unconfirmed work",
            Some(Forge::Github),
            Some("codewhale/cloud-unconfirmed"),
        )
        .unwrap();
        match execute_dispatch(
            &store,
            plan,
            false,
            &CredentialState::Present {
                source: CredentialSource::Env,
            },
            &MachineTokenState::Present,
        )
        .unwrap()
        {
            DispatchOutcome::Proposal(job) => {
                let launcher = RecordingLauncher::new("never", fixture_patch());
                let forge = RecordingForgePr::new("https://github.com/org/repo/pull/1");
                let error = run_confirmed_job(&store, &job.id, &launcher, &forge)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("confirm it first"), "{error}");
                assert!(launcher.calls().is_empty());
            }
            other => panic!("expected proposal, got {other:?}"),
        }
    }

    #[test]
    fn launch_failure_fails_closed_with_sanitized_note() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let job = confirmed_job(&store);
        struct FailingLauncher;
        impl DaytonaLauncher for FailingLauncher {
            fn create_sandbox(&self, _job: &CloudJob) -> Result<SandboxReceipt> {
                bail!("create exploded\u{1}")
            }
        }
        let forge = RecordingForgePr::new("https://github.com/org/repo/pull/2");
        let error = run_confirmed_job(&store, &job.id, &FailingLauncher, &forge).unwrap_err();
        assert!(error.to_string().contains("create exploded"));
        let failed = store.load(&job.id).unwrap();
        assert_eq!(failed.status, CloudJobStatus::Failed);
        assert!(failed.pr_url.is_none());
        assert!(failed.note.contains("failed closed"));
        assert!(!failed.note.contains('\u{1}'));
        assert!(forge.opened().is_empty());
    }

    #[test]
    fn pr_title_and_body_are_truthful_unbranded_and_carry_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let job = confirmed_job(&store);
        let patch = fixture_patch();
        let title = compose_pr_title(&job, &patch);
        assert_eq!(title, "codewhale cloud: Fix the flaky dispatch test");
        let body = compose_pr_body(&job, &patch);
        assert!(body.contains("Codewhale cloud agent"));
        assert!(body.contains("What the agent did"));
        assert!(body.contains("Fix the flaky dispatch test"));
        assert!(body.contains("open a PR that fixes the flake"));
        assert!(body.contains(&job.id));
        assert!(body.contains("(pending)"));
        assert!(body.contains("codewhale/cloud-runner-test"));
        assert!(body.contains("`main`"));
        assert!(body.contains("abc123def4567"));
        assert!(body.contains("No-Issue: cloud dispatch"));
        for banned in ["Daytona", "daytona"] {
            assert!(
                !body.contains(banned),
                "body must not brand the sandbox: {banned}"
            );
        }
    }

    #[test]
    fn harness_command_is_the_standard_one_shot_entry() {
        let temp = tempfile::tempdir().unwrap();
        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let job = confirmed_job(&store);
        let command = harness_command(&job);
        assert_eq!(
            command.argv,
            vec![
                "codewhale".to_string(),
                "exec".to_string(),
                "--auto".to_string(),
                job.prompt.clone(),
            ]
        );
        assert_eq!(command.cwd, SANDBOX_WORKSPACE);
    }

    #[test]
    fn gh_argv_and_forge_endpoints_pin_the_pr_shapes() {
        let argv = gh_pr_create_argv(
            "org/repo",
            "main",
            "codewhale/cloud-1",
            "title",
            "/tmp/body.md",
        );
        assert_eq!(
            argv,
            vec![
                "pr",
                "create",
                "--repo",
                "org/repo",
                "--base",
                "main",
                "--head",
                "codewhale/cloud-1",
                "--title",
                "title",
                "--body-file",
                "/tmp/body.md",
            ]
        );
        assert_eq!(
            gitee_pr_url("org/repo"),
            "https://gitee.com/api/v5/repos/org/repo/pulls"
        );
        assert_eq!(
            cnb_pr_url("org/repo"),
            "https://api.cnb.cool/org/repo/-/pulls"
        );
    }

    #[test]
    fn forge_slug_parses_https_and_ssh_and_rejects_foreign_hosts() {
        assert_eq!(
            forge_slug(Forge::Github, "https://github.com/Hmbown/CodeWhale.git"),
            Some("Hmbown/CodeWhale".to_string())
        );
        assert_eq!(
            forge_slug(Forge::Github, "git@github.com:Hmbown/CodeWhale.git"),
            Some("Hmbown/CodeWhale".to_string())
        );
        assert_eq!(
            forge_slug(Forge::Cnb, "https://cnb.cool/codewhale.net/codewhale.git"),
            Some("codewhale.net/codewhale".to_string())
        );
        assert_eq!(
            forge_slug(Forge::Gitee, "https://gitee.com/org/repo.git"),
            Some("org/repo".to_string())
        );
        assert_eq!(
            forge_slug(Forge::Github, "https://gitee.com/org/repo.git"),
            None
        );
        assert_eq!(
            forge_slug(Forge::Cnb, "https://example.test/org/repo.git"),
            None
        );
        assert_eq!(
            forge_slug(Forge::Github, "https://github.com/only-repo"),
            None
        );
    }

    #[test]
    fn push_branch_refuses_non_forge_remotes() {
        let error =
            push_branch(Path::new("."), "https://example.test/org/repo.git", "b").unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("non-forge") || text.contains("not a supported forge"),
            "{text}"
        );
    }

    #[test]
    fn push_branch_refuses_forge_default_branch_names() {
        let error =
            push_branch(Path::new("."), "https://github.com/org/repo.git", "main").unwrap_err();
        assert!(error.to_string().contains("default branch"), "{}", error);
    }

    #[test]
    fn prepare_branch_rejects_leading_dash_remote_before_clone() {
        let temp = tempfile::tempdir().unwrap();
        let mut job = confirmed_job(&CloudJobStore::from_path(temp.path().join("jobs")));
        job.remote_url = "--upload-pack=evil".to_string();
        let error = prepare_branch(&job, &fixture_patch(), temp.path()).unwrap_err();
        assert!(
            error.to_string().contains("must not start with '-'"),
            "{error}"
        );
    }

    #[test]
    fn compose_pr_body_head_is_the_pushed_sha_not_the_sandbox_sha() {
        let job = CloudJob {
            id: "cloud_00000000000000cc".to_string(),
            kind: "cloud".to_string(),
            status: CloudJobStatus::OpeningPr,
            prompt: "fix".to_string(),
            forge: Forge::Github,
            remote_name: "github".to_string(),
            remote_url: "https://github.com/org/repo.git".to_string(),
            branch: "codewhale/cloud-b".to_string(),
            confirmed: true,
            sandbox_id: Some("sandbox".to_string()),
            pr_url: None,
            refusal: None,
            note: "n".to_string(),
            created_unix: 1,
            base_branch: None,
            head_sha: None,
            agent_summary: None,
            finished_unix: None,
            sandbox_pending: false,
        };
        let patch = fixture_patch();
        let body = compose_pr_body_for_head(&job, &patch, "pushedsha0000000000000000000000000001");
        assert!(body.contains("Head: `pushedsha0000000000000000000000000001`"));
        assert!(
            !body.contains(&format!("Head: `{}`", patch.head_sha)),
            "sandbox sha must not be the receipt Head"
        );
    }

    /// Local-fixture integration: real git, no network — branch preparation
    /// applies a real format-patch, and a diverged push fails without force.
    #[test]
    fn prepare_branch_applies_a_real_patch_locally() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin.git");
        git(
            None,
            &[
                "init",
                "--bare",
                "--quiet",
                "--initial-branch=main",
                &origin.to_string_lossy(),
            ],
        )
        .unwrap();
        let seed = temp.path().join("seed");
        git(
            None,
            &[
                "clone",
                "--quiet",
                &origin.to_string_lossy(),
                &seed.to_string_lossy(),
            ],
        )
        .unwrap();
        git(Some(&seed), &["config", "user.name", "Seeder"]).unwrap();
        git(Some(&seed), &["config", "user.email", "seed@example.test"]).unwrap();
        std::fs::write(seed.join("file.txt"), "base\n").unwrap();
        git(Some(&seed), &["add", "."]).unwrap();
        git(Some(&seed), &["commit", "--quiet", "-m", "base"]).unwrap();
        git(
            Some(&seed),
            &[
                "push",
                "--quiet",
                &origin.to_string_lossy(),
                "HEAD:refs/heads/main",
            ],
        )
        .unwrap();

        // The agent's work product: a real patch.
        std::fs::write(seed.join("file.txt"), "base\nagent change\n").unwrap();
        git(Some(&seed), &["add", "."]).unwrap();
        git(Some(&seed), &["commit", "--quiet", "-m", "agent work"]).unwrap();
        let patch_text = git(Some(&seed), &["format-patch", "HEAD~1", "--stdout"]).unwrap();

        let store = CloudJobStore::from_path(temp.path().join("jobs"));
        let mut job = confirmed_job(&store);
        // Point at the local fixture so preparation is offline.
        job.remote_url = origin.to_string_lossy().to_string();
        let patch = PatchReceipt {
            base_branch: "main".to_string(),
            head_sha: "fixture".to_string(),
            summary: "agent work".to_string(),
            patch: patch_text,
        };
        let dir = temp.path().join("agent");
        std::fs::create_dir_all(&dir).unwrap();
        let head = prepare_branch(&job, &patch, &dir).unwrap();
        assert_eq!(
            head.len(),
            40,
            "prepare_branch returns the applied head sha"
        );

        // A diverged push onto the same branch must fail: no force, ever.
        let repo = dir.join("repo");
        std::fs::write(repo.join("other.txt"), "y\n").unwrap();
        git(Some(&repo), &["add", "."]).unwrap();
        git(Some(&repo), &["commit", "--quiet", "-m", "diverges"]).unwrap();
        // Seed the remote branch at the "diverges" commit.
        git(
            Some(&repo),
            &[
                "push",
                "--quiet",
                &origin.to_string_lossy(),
                "HEAD:refs/heads/codewhale/cloud-runner-test",
            ],
        )
        .unwrap();
        // Rewind and build a sibling commit: same parent, different content.
        git(Some(&repo), &["reset", "--quiet", "--hard", "HEAD~1"]).unwrap();
        std::fs::write(repo.join("other2.txt"), "z\n").unwrap();
        git(Some(&repo), &["add", "."]).unwrap();
        git(
            Some(&repo),
            &["commit", "--quiet", "-m", "diverges differently"],
        )
        .unwrap();
        let diverged = git(
            Some(&repo),
            &[
                "push",
                "--quiet",
                &origin.to_string_lossy(),
                "HEAD:refs/heads/codewhale/cloud-runner-test",
            ],
        );
        assert!(diverged.is_err(), "a diverged push must fail without force");
    }

    #[test]
    fn compose_pr_body_bounds_oversized_prompts() {
        let job = CloudJob {
            id: "cloud_00000000000000bb".to_string(),
            kind: "cloud".to_string(),
            status: CloudJobStatus::OpeningPr,
            prompt: "p".repeat(9_000),
            forge: Forge::Github,
            remote_name: "github".to_string(),
            remote_url: "https://github.com/org/repo.git".to_string(),
            branch: "codewhale/cloud-b".to_string(),
            confirmed: true,
            sandbox_id: Some("sandbox".to_string()),
            pr_url: None,
            refusal: None,
            note: "n".to_string(),
            created_unix: 1,
            base_branch: None,
            head_sha: None,
            agent_summary: None,
            finished_unix: None,
            sandbox_pending: false,
        };
        let body = compose_pr_body(&job, &fixture_patch());
        assert!(body.chars().count() <= MAX_BODY_CHARS);
    }

    #[test]
    fn one_line_flattens_control_characters() {
        assert_eq!(one_line("a\nb", 10), "a\nb");
        assert_eq!(one_line("a\u{1}b", 10), "a b");
        assert_eq!(one_line("abcdef", 3), "ab…");
    }
}
