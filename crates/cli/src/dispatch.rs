//! First-class Daytona cloud-agent offload: `codewhale dispatch`.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use codewhale_tui::cloud_dispatch::{
    CloudJobStore, DispatchOutcome, Forge, LiveDaytonaLauncher, cancel_job, confirm_job,
    discover_credentials, discover_machine_token, discover_remotes, execute_dispatch, format_job,
    format_job_list, format_status, plan_dispatch,
};
use codewhale_tui::dispatch_runner::spawn_confirmed_runner;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ForgeArg {
    Github,
    Cnb,
    Gitee,
}

impl From<ForgeArg> for Forge {
    fn from(value: ForgeArg) -> Self {
        match value {
            ForgeArg::Github => Forge::Github,
            ForgeArg::Cnb => Forge::Cnb,
            ForgeArg::Gitee => Forge::Gitee,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct DispatchArgs {
    /// Task for the remote agent. Required unless listing or inspecting a job.
    #[arg(value_name = "PROMPT")]
    prompt: Vec<String>,
    /// Forge that should receive the branch and PR: github, cnb, or gitee.
    #[arg(long, value_enum)]
    remote: Option<ForgeArg>,
    /// Branch the remote agent will raise (default: codewhale/cloud-<unix>).
    #[arg(long)]
    branch: Option<String>,
    /// Required to create Codewhale cloud-agent spend or push. Without this, only a proposal is written.
    #[arg(long)]
    confirm: bool,
    /// Show remotes and whether Codewhale cloud-agent credentials are present (never prints secrets).
    #[arg(long)]
    status: bool,
    /// List first-class cloud jobs (same kind shown by `/jobs`).
    #[arg(long)]
    list: bool,
    /// Inspect one cloud job.
    #[arg(long, value_name = "ID")]
    show: Option<String>,
    /// Cancel one cloud job.
    #[arg(long, value_name = "ID")]
    cancel: Option<String>,
    /// Workspace whose git remotes are classified (default: current directory).
    #[arg(long)]
    cwd: Option<PathBuf>,
}

pub(crate) fn run(args: DispatchArgs) -> Result<()> {
    let mut out = io::stdout().lock();
    run_with(args, &mut out)
}

fn run_with<W: Write>(args: DispatchArgs, out: &mut W) -> Result<()> {
    if [
        args.status,
        args.list,
        args.show.is_some(),
        args.cancel.is_some(),
        !args.prompt.is_empty(),
    ]
    .iter()
    .filter(|flag| **flag)
    .count()
        > 1
    {
        bail!("Use one of: a prompt, --status, --list, --show <id>, or --cancel <id>.");
    }

    let workspace = args
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let remotes = discover_remotes(&workspace);
    let credentials = discover_credentials();
    let store = CloudJobStore::from_env()?;

    if args.status
        || (args.prompt.is_empty() && args.show.is_none() && args.cancel.is_none() && !args.list)
    {
        writeln!(
            out,
            "{}",
            format_status(&remotes, &credentials, &recent_jobs(&store))
        )?;
        return Ok(());
    }
    if args.list {
        writeln!(out, "{}", format_job_list(&store.list()?))?;
        return Ok(());
    }
    if let Some(id) = args.show.as_deref() {
        writeln!(out, "{}", format_job(&store.load(id)?))?;
        return Ok(());
    }
    if let Some(id) = args.cancel.as_deref() {
        writeln!(
            out,
            "{}",
            format_job(&cancel_job(&store, id, &LiveDaytonaLauncher)?)
        )?;
        return Ok(());
    }

    let prompt = args.prompt.join(" ");
    if prompt.starts_with("cloud_") && args.confirm && prompt.split_whitespace().count() == 1 {
        let outcome = confirm_job(
            &store,
            prompt.trim(),
            &credentials,
            &discover_machine_token(),
        )?;
        let runner = spawn_accepted(&store, &outcome);
        write_outcome(out, outcome)?;
        return join_runner(out, &store, prompt.trim(), runner);
    }

    let plan = plan_dispatch(
        &remotes,
        &prompt,
        args.remote.map(Forge::from),
        args.branch.as_deref(),
    )?;
    let outcome = execute_dispatch(
        &store,
        plan,
        args.confirm,
        &credentials,
        &discover_machine_token(),
    )?;
    let runner = spawn_accepted(&store, &outcome);
    let job_id = outcome_job_id(&outcome).unwrap_or_default();
    write_outcome(out, outcome)?;
    join_runner(out, &store, &job_id, runner)
}

/// The CLI stays attached to a confirmed run: the card prints immediately,
/// then the process waits for the runner so a paid sandbox is never
/// orphaned by an early exit. Ctrl-C exits the wait; the job stays recorded
/// and `--cancel` tears the sandbox down.
fn join_runner<W: Write>(
    out: &mut W,
    store: &CloudJobStore,
    id: &str,
    runner: Option<std::thread::JoinHandle<()>>,
) -> Result<()> {
    if let Some(runner) = runner {
        runner
            .join()
            .map_err(|_| anyhow::anyhow!("the cloud agent runner panicked"))?;
        if !id.is_empty()
            && let Ok(job) = store.load(id)
        {
            writeln!(out, "{}", format_job(&job))?;
        }
    }
    Ok(())
}

fn outcome_job_id(outcome: &DispatchOutcome) -> Option<String> {
    match outcome {
        DispatchOutcome::Proposal(job)
        | DispatchOutcome::Refused(job)
        | DispatchOutcome::Accepted(job) => Some(job.id.clone()),
    }
}

/// Newest jobs for the status card's receipts section (best effort — an
/// unreadable store must not hide the card).
fn recent_jobs(store: &CloudJobStore) -> Vec<codewhale_tui::cloud_dispatch::CloudJob> {
    store
        .list()
        .unwrap_or_default()
        .into_iter()
        .take(5)
        .collect()
}

/// Start the background runner for a just-accepted confirm. The sandbox,
/// harness turn, branch push, PR open, and teardown all happen there.
fn spawn_accepted(
    store: &CloudJobStore,
    outcome: &DispatchOutcome,
) -> Option<std::thread::JoinHandle<()>> {
    match outcome {
        DispatchOutcome::Accepted(job) => spawn_confirmed_runner(store.clone(), job.id.clone()),
        _ => None,
    }
}

fn write_outcome<W: Write>(out: &mut W, outcome: DispatchOutcome) -> Result<()> {
    match outcome {
        DispatchOutcome::Proposal(job) | DispatchOutcome::Accepted(job) => {
            writeln!(out, "{}", format_job(&job))?;
            Ok(())
        }
        DispatchOutcome::Refused(job) => {
            writeln!(out, "{}", format_job(&job))?;
            bail!("{}", job.note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Commands};
    use clap::Parser;

    fn args(argv: &[&str]) -> DispatchArgs {
        let cli = Cli::try_parse_from(argv).unwrap();
        let Some(Commands::Dispatch(args)) = cli.command else {
            panic!("expected dispatch command");
        };
        args
    }

    #[test]
    fn parses_the_obvious_dispatch_command() {
        let parsed = args(&[
            "codewhale",
            "dispatch",
            "fix",
            "the",
            "flake",
            "--remote",
            "github",
        ]);
        assert_eq!(parsed.prompt, ["fix", "the", "flake"]);
        assert_eq!(parsed.remote, Some(ForgeArg::Github));
        assert!(!parsed.confirm);
        assert!(args(&["codewhale", "cloud-agent", "--status"]).status);
        assert!(Cli::try_parse_from(["codewhale", "dispatch", "--remote", "gitlab"]).is_err());
    }

    #[test]
    fn refused_confirmation_is_a_nonzero_error() {
        use codewhale_tui::cloud_dispatch::{CloudJob, CloudJobStatus};
        let job = CloudJob {
            id: "cloud_00000000000000dd".to_string(),
            kind: "cloud".to_string(),
            status: CloudJobStatus::Refused,
            prompt: "fix".to_string(),
            forge: Forge::Github,
            remote_name: "github".to_string(),
            remote_url: "https://github.com/org/repo.git".to_string(),
            branch: "codewhale/cloud-x".to_string(),
            confirmed: true,
            sandbox_id: None,
            pr_url: None,
            refusal: Some("no credentials".to_string()),
            note: "Refused: cloud agents are not available.".to_string(),
            created_unix: 1,
            base_branch: None,
            head_sha: None,
            agent_summary: None,
            finished_unix: Some(1),
            sandbox_pending: false,
        };
        let error = write_outcome(&mut Vec::new(), DispatchOutcome::Refused(job)).unwrap_err();
        assert!(
            error.to_string().contains("Refused"),
            "refused confirmations must not exit 0: {error}"
        );
    }

    #[test]
    fn status_is_fail_closed_and_never_prints_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let mut output = Vec::new();
        run_with(
            DispatchArgs {
                prompt: Vec::new(),
                remote: None,
                branch: None,
                confirm: false,
                status: true,
                list: false,
                show: None,
                cancel: None,
                cwd: Some(temp.path().to_path_buf()),
            },
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Codewhale cloud dispatch"));
        assert!(!text.contains("Daytona"));
        assert!(!text.contains("sk-"));
        assert!(!text.contains("Bearer"));
    }

    #[test]
    fn rendered_help_carries_no_provider_brand() {
        use clap::CommandFactory;
        let help = Cli::command()
            .find_subcommand_mut("dispatch")
            .expect("dispatch subcommand exists")
            .render_help()
            .to_string();
        // The reworded cloud-agent copy must actually land in --help…
        assert!(help.contains("cloud-agent"), "{help}");
        assert!(help.contains("--confirm"), "{help}");
        // …and no provider brand may leak into it.
        for banned in ["Daytona", "daytona"] {
            assert!(
                !help.contains(banned),
                "--help must not brand the operator: {banned}"
            );
        }
    }
}
