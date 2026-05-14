//! Output truncation and summarization helpers for shell tools.

/// Maximum output size before truncation (30KB like Claude Code).
const MAX_OUTPUT_SIZE: usize = 30_000;
/// Head bytes preserved for large shell/test output. The matching tail budget
/// keeps final errors and test summaries visible without a second command.
const TRUNCATED_HEAD_BYTES: usize = 22_000;
const TRUNCATED_TAIL_BYTES: usize = MAX_OUTPUT_SIZE - TRUNCATED_HEAD_BYTES;
/// Limits for summary strings in tool metadata.
const SUMMARY_MAX_LINES: usize = 3;
const SUMMARY_MAX_CHARS: usize = 240;
/// Maximum number of preserved high-signal lines extracted from the tail
/// when output is truncated (#242). Bounded so the preserved summary
/// itself can never blow up the context window.
const MAX_PRESERVED_SUMMARY_LINES: usize = 80;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TruncationMeta {
    pub(crate) original_len: usize,
    pub(crate) omitted: usize,
    pub(crate) truncated: bool,
}

pub(crate) fn truncate_with_meta(output: &str) -> (String, TruncationMeta) {
    let original_len = output.len();
    if original_len <= MAX_OUTPUT_SIZE {
        return (
            output.to_string(),
            TruncationMeta {
                original_len,
                omitted: 0,
                truncated: false,
            },
        );
    }

    let head_end = char_boundary_at_or_before(output, TRUNCATED_HEAD_BYTES);
    let tail_start =
        char_boundary_at_or_after(output, original_len.saturating_sub(TRUNCATED_TAIL_BYTES));
    let head = &output[..head_end];
    let omitted_middle = &output[head_end..tail_start];
    let tail = &output[tail_start..];
    let omitted = omitted_middle.len();
    let note = format!(
        "...\n\n[Output truncated: showing first {head_bytes} bytes and last {tail_bytes} bytes. {omitted} bytes omitted.]",
        head_bytes = head.len(),
        tail_bytes = tail.len(),
    );

    // Preserve high-signal summary lines from the omitted middle (cargo test
    // results, rustc errors, panics, completion markers). The raw tail is
    // already included below; these snippets keep earlier failures visible
    // without re-running `cargo test | tail` repeatedly (#242/#1450).
    let mut combined = format!("{head}{note}");
    let preserved = collect_summary_lines(omitted_middle);
    if !preserved.is_empty() {
        combined.push_str("\n\n[Preserved summary lines from omitted middle]\n");
        combined.push_str(&preserved.join("\n"));
    }
    combined.push_str("\n\n[Output tail]\n");
    combined.push_str(tail);

    (
        combined,
        TruncationMeta {
            original_len,
            omitted,
            truncated: true,
        },
    )
}

/// Extract high-signal summary lines from a chunk of output that would
/// otherwise be discarded by truncation. Recognises Cargo/rustc output,
/// generic test framework summaries, panic markers, exit-status lines,
/// and `Finished`/`running ...` markers. Returns at most
/// `MAX_PRESERVED_SUMMARY_LINES` lines, oldest-first within each match
/// class so the most actionable signal is at the end.
pub(crate) fn collect_summary_lines(text: &str) -> Vec<String> {
    let mut preserved: Vec<String> = Vec::new();
    for line in text.lines() {
        if preserved.len() >= MAX_PRESERVED_SUMMARY_LINES {
            break;
        }
        if is_summary_line(line) {
            preserved.push(line.to_string());
        }
    }
    preserved
}

/// Heuristics for "this line is worth preserving even when most of the
/// output is dropped." Tuned for Cargo/rustc and generic test runner
/// vocabulary. Intentionally conservative: false positives only cost a
/// handful of bytes; false negatives force the agent to re-run gates.
fn is_summary_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    // Cargo / rustc canonical markers. Note `trim_start` already stripped
    // any leading whitespace, so match the bare word — the indentation
    // Cargo prints (e.g. "    Finished") would never reach this point.
    if trimmed.starts_with("test result:")
        || trimmed.starts_with("failures:")
        || trimmed.starts_with("FAILED")
        || trimmed.starts_with("error[")
        || trimmed.starts_with("error:")
        || trimmed.starts_with("warning:")
        || trimmed.starts_with("panicked at")
        || trimmed.starts_with("note:")
        || trimmed.starts_with("help:")
        || trimmed.starts_with("Finished")
        || trimmed.starts_with("Compiling")
        || trimmed.starts_with("Building")
        || trimmed.starts_with("Running")
        || trimmed.starts_with("running ")
        || trimmed.starts_with("Doc-tests")
        || trimmed.starts_with("---- ")
    {
        return true;
    }
    // Generic test runner vocabulary.
    if trimmed.contains("PASS") || trimmed.contains("FAIL") || trimmed.contains("ASSERT") {
        return true;
    }
    // Process-level signal lines.
    if trimmed.starts_with("Killed")
        || trimmed.starts_with("Aborted")
        || trimmed.starts_with("Segmentation fault")
        || trimmed.starts_with("Error:")
        || trimmed.starts_with("exit status")
        || trimmed.starts_with("exit code")
    {
        return true;
    }
    // `test some::name ... ok|FAILED|ignored` is the per-test result line in
    // libtest. Cheap to match and useful for pinpointing the failing case.
    if trimmed.starts_with("test ") && (trimmed.ends_with("FAILED") || trimmed.ends_with("ignored"))
    {
        return true;
    }
    false
}

fn char_boundary_at_or_before(text: &str, max_bytes: usize) -> usize {
    if max_bytes >= text.len() {
        return text.len();
    }

    let mut last_end = 0usize;
    for (idx, ch) in text.char_indices() {
        let end = idx.saturating_add(ch.len_utf8());
        if end > max_bytes {
            break;
        }
        last_end = end;
    }

    last_end.min(text.len())
}

fn char_boundary_at_or_after(text: &str, min_bytes: usize) -> usize {
    if min_bytes >= text.len() {
        return text.len();
    }
    if text.is_char_boundary(min_bytes) {
        return min_bytes;
    }
    text.char_indices()
        .map(|(idx, _)| idx)
        .find(|&idx| idx > min_bytes)
        .unwrap_or(text.len())
}

fn strip_truncation_note(text: &str) -> &str {
    text.split_once("\n\n[Output truncated")
        .map_or(text, |(prefix, _)| prefix)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut end = text.len();
    for (count, (idx, _)) in text.char_indices().enumerate() {
        if count == max_chars {
            end = idx;
            break;
        }
    }

    format!("{}...", &text[..end])
}

pub(crate) fn summarize_output(text: &str) -> String {
    let stripped = strip_truncation_note(text);
    let summary = stripped
        .lines()
        .take(SUMMARY_MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if summary.is_empty() {
        String::new()
    } else {
        truncate_chars(&summary, SUMMARY_MAX_CHARS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_preserves_cargo_test_summary_lines_from_tail() {
        let mut head = String::with_capacity(MAX_OUTPUT_SIZE + 4_000);
        head.push_str("running 5 tests\n");
        for i in 0..3_000 {
            head.push_str(&format!("test test::case_{i} ... ok\n"));
        }
        // Pad to force tail truncation
        while head.len() < MAX_OUTPUT_SIZE {
            head.push_str("...padding line below threshold...\n");
        }
        head.push_str("\ntest result: ok. 1687 passed; 0 failed; 2 ignored\n");
        head.push_str("    Finished `dev` profile target(s) in 4.87s\n");

        let (truncated, meta) = truncate_with_meta(&head);
        assert!(meta.truncated, "expected truncation");
        assert!(
            truncated.contains("test result: ok. 1687 passed"),
            "summary line must be preserved\nGot: {}",
            &truncated[truncated.len().saturating_sub(400)..]
        );
        assert!(
            truncated.contains("Finished"),
            "Finished marker must be preserved"
        );
    }

    #[test]
    fn truncation_preserves_failure_lines_from_tail() {
        let mut head = String::with_capacity(MAX_OUTPUT_SIZE + 1_000);
        for _ in 0..MAX_OUTPUT_SIZE {
            head.push('a');
        }
        head.push_str("\nfailures:\n  test::flaky_thing FAILED\n");
        head.push_str("test result: FAILED. 0 passed; 1 failed\n");

        let (truncated, _meta) = truncate_with_meta(&head);
        assert!(truncated.contains("failures:"), "must preserve failures:");
        assert!(truncated.contains("FAILED"), "must preserve FAILED");
    }

    #[test]
    fn truncation_includes_raw_tail_for_shell_output() {
        let mut output = String::new();
        output.push_str("head-marker\n");
        output.push_str(&"middle noise\n".repeat(3_000));
        output.push_str("tail-marker: final compiler error\n");

        let (truncated, meta) = truncate_with_meta(&output);

        assert!(meta.truncated, "expected truncation");
        assert!(truncated.contains("head-marker"));
        assert!(
            truncated.contains("[Output tail]"),
            "tail section should be explicit: {truncated}"
        );
        assert!(
            truncated.contains("tail-marker: final compiler error"),
            "raw tail must remain visible"
        );
    }

    #[test]
    fn collect_summary_lines_skips_noise() {
        let body = "\nblah blah\nrandom line\nokay\n\n";
        assert!(collect_summary_lines(body).is_empty());
    }

    #[test]
    fn collect_summary_lines_picks_rustc_errors() {
        let body = "\
some preamble
error[E0277]: the trait `Foo` is not implemented for `Bar`
  --> src/lib.rs:42:9
warning: unused variable
note: see help
";
        let preserved = collect_summary_lines(body);
        assert!(preserved.iter().any(|line| line.contains("error[E0277]")));
        assert!(preserved.iter().any(|line| line.contains("warning:")));
    }
}
