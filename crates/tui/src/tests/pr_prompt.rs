use super::*;

fn sample_pr() -> GhPullRequest {
    GhPullRequest {
        title: "Add cool feature".to_string(),
        body: "Closes #99.\n\nAlso:\n- bullet a\n- bullet b".to_string(),
        base: "main".to_string(),
        head: "feat/cool".to_string(),
        url: "https://github.com/example/repo/pull/123".to_string(),
        head_sha: "abc123def456abc123def456abc123def456abc1".to_string(),
    }
}

#[test]
fn format_pr_prompt_includes_title_url_branches_body_and_diff() {
    let prompt = format_pr_prompt(123, &sample_pr(), "diff --git a/x b/x\n+y");
    assert!(prompt.contains("Review PR #123 — Add cool feature"));
    assert!(prompt.contains("URL: https://github.com/example/repo/pull/123"));
    assert!(prompt.contains("Branches: main ← feat/cool"));
    assert!(prompt.contains("Closes #99."));
    assert!(prompt.contains("- bullet a"));
    assert!(prompt.contains("```diff"));
    assert!(prompt.contains("diff --git a/x b/x"));
}

#[test]
fn format_pr_prompt_handles_empty_body_and_unknown_branches() {
    let pr = GhPullRequest {
        title: String::new(),
        body: "   ".to_string(),
        base: String::new(),
        head: String::new(),
        url: String::new(),
        head_sha: String::new(),
    };
    let prompt = format_pr_prompt(7, &pr, "(diff body)");
    assert!(prompt.contains("(PR #7)"));
    assert!(prompt.contains("(no description)"));
    assert!(prompt.contains("Branches: (unknown)"));
    assert!(prompt.contains("URL: (unavailable)"));
}

#[test]
fn format_pr_prompt_truncates_oversize_diff_at_a_codepoint_boundary() {
    let mut diff = "X".repeat(190 * 1024);
    diff.push_str(&"🚀".repeat(5_000));
    let prompt = format_pr_prompt(1, &sample_pr(), &diff);
    assert!(prompt.contains("[…diff truncated"));
    assert!(prompt.contains("at 200 KiB"));
    assert!(prompt.is_ascii() || prompt.contains('🚀'));
}

#[test]
fn is_command_available_detects_present_and_absent_binaries() {
    #[cfg(unix)]
    assert!(is_command_available("sh"), "POSIX `sh` should be on PATH");

    assert!(
        !is_command_available("this-command-cannot-exist-codewhale-tui-test-ENOENT-marker"),
        "missing command should return false, not panic"
    );
}
