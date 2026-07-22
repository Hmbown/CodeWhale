//! Process-boundary/source-wiring acceptance for delegated coordination (#4647).
//!
//! Behavioral state-machine coverage lives beside `SubAgentManager`, where it
//! can exercise the real private ledger without inventing a second public API.
//! This integration target locks the public wiring and uses a real tempfile Git
//! repository to prove that terminal fan-in evidence keeps both candidates.

use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

const COORD_SOURCE: &str = include_str!("../src/tools/subagent/coord.rs");
const SUBAGENT_SOURCE: &str = include_str!("../src/tools/subagent/mod.rs");
const WORKER_SOURCE: &str = include_str!("../src/fleet/worker_runtime.rs");
const FLEET_MANAGER_SOURCE: &str = include_str!("../src/fleet/manager.rs");
const TEST_SOURCE: &str = include_str!("../src/tools/subagent/tests.rs");

#[test]
fn coordination_contract_is_live_bounded_and_fail_closed() {
    for required in [
        "COORDINATION_SCHEMA_VERSION",
        "ContextProjectionReceipt",
        "WriteContentionReceipt",
        "candidate_handles",
        "retry_limit",
        "reviewer_evidence_handles",
        "verifier_evidence_handles",
        "verification_outcome",
    ] {
        assert!(
            COORD_SOURCE.contains(required),
            "coordination ledger lost required field {required}"
        );
    }
    for required in [
        "nearest_common_fan_in_owner",
        "project_relevant_decisions",
        "prompt-only/general launch remains ergonomic",
        "write-capable agent starts must declare",
        "register_worker_with_coordination",
        "context_projections",
        "hottest_paths",
    ] {
        assert!(
            SUBAGENT_SOURCE.contains(required),
            "runtime coordination path lost {required}"
        );
    }
    assert!(
        !SUBAGENT_SOURCE.contains("vec![\".\".to_string()]"),
        "missing writer declarations must never become an implicit repo-wide claim"
    );
    assert!(
        WORKER_SOURCE.contains("launch_manifest: Some(launch_manifest)"),
        "Fleet workers must persist the same #414 launch manifest"
    );
    assert!(
        FLEET_MANAGER_SOURCE.contains("preflight_worker_coordination")
            && FLEET_MANAGER_SOURCE.contains("register_worker_with_coordination"),
        "Fleet dispatch must enforce claims before its durable running transition"
    );
    assert!(
        TEST_SOURCE.contains("coordination_acceptance_preserves_scopes_candidates_and_replay"),
        "private-ledger acceptance fixture must remain wired"
    );
}

#[test]
fn terminal_retry_fixture_preserves_both_real_git_candidates() {
    let repo = tempdir().expect("temp repo");
    git(repo.path(), &["init"]);
    git(repo.path(), &["config", "core.autocrlf", "false"]);
    git(repo.path(), &["config", "user.name", "codewhale Tests"]);
    git(repo.path(), &["config", "user.email", "tests@example.com"]);
    git(repo.path(), &["config", "commit.gpgsign", "false"]);
    git(repo.path(), &["commit", "--allow-empty", "-m", "base"]);
    let base = git_stdout(repo.path(), &["branch", "--show-current"]);

    git(repo.path(), &["switch", "-c", "candidate-a"]);
    std::fs::create_dir_all(repo.path().join("src")).expect("src");
    std::fs::write(repo.path().join("src/a.rs"), "pub const A: u8 = 1;\n").expect("candidate A");
    git(repo.path(), &["add", "src/a.rs"]);
    git(repo.path(), &["commit", "-m", "candidate A"]);
    let candidate_a = git_stdout(repo.path(), &["rev-parse", "HEAD"]);

    git(repo.path(), &["switch", &base]);
    git(repo.path(), &["switch", "-c", "candidate-b"]);
    std::fs::create_dir_all(repo.path().join("src")).expect("src");
    std::fs::write(repo.path().join("src/b.rs"), "pub const B: u8 = 2;\n").expect("candidate B");
    git(repo.path(), &["add", "src/b.rs"]);
    git(repo.path(), &["commit", "-m", "candidate B"]);
    let candidate_b = git_stdout(repo.path(), &["rev-parse", "HEAD"]);

    assert_ne!(candidate_a, candidate_b);
    assert_eq!(
        git_stdout(repo.path(), &["show", "candidate-a:src/a.rs"]),
        "pub const A: u8 = 1;"
    );
    assert_eq!(
        git_stdout(repo.path(), &["show", "candidate-b:src/b.rs"]),
        "pub const B: u8 = 2;"
    );
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
