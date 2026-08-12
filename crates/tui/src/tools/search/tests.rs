use std::fs;

use serde_json::{Value, json};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::tools::spec::{ApprovalRequirement, ToolContext, ToolSpec};

use super::{GrepFilesTool, matches_glob};

#[test]
fn grep_description_matches_default_exclusion_behavior() {
    let description = GrepFilesTool.description();

    assert!(description.contains("skips common non-code directories"));
    assert!(!description.contains("respects `.gitignore`"));
}

/// Representative of the ~150 shared `optional_*` call sites outside the
/// three that motivated the change: a wrong type is refused by name, an
/// absent or null field still takes its default.
#[tokio::test]
async fn grep_refuses_mistyped_optional_parameters_by_name() {
    let tmp = tempdir().expect("tempdir");
    fs::write(tmp.path().join("a.txt"), "needle\n").expect("write");
    let ctx = ToolContext::new(tmp.path());

    for (field, input) in [
        (
            "max_results",
            json!({"pattern": "needle", "max_results": "10"}),
        ),
        (
            "case_insensitive",
            json!({"pattern": "needle", "case_insensitive": "true"}),
        ),
        (
            "context_lines",
            json!({"pattern": "needle", "context_lines": 2.5}),
        ),
        ("path", json!({"pattern": "needle", "path": ["."]})),
    ] {
        let err = GrepFilesTool
            .execute(input, &ctx)
            .await
            .expect_err("a mistyped optional parameter must be refused");
        let err = err.to_string();
        assert!(err.contains(field), "error must name '{field}': {err}");
    }

    GrepFilesTool
        .execute(
            json!({"pattern": "needle", "max_results": Value::Null, "path": Value::Null}),
            &ctx,
        )
        .await
        .expect("explicit nulls read as absent");
}

#[test]
fn test_matches_glob_star() {
    assert!(matches_glob("test.rs", "*.rs"));
    assert!(matches_glob("foo.rs", "*.rs"));
    assert!(!matches_glob("test.ts", "*.rs"));
    assert!(!matches_glob("test.rs.bak", "*.rs"));
}

#[test]
fn test_matches_glob_question() {
    assert!(matches_glob("test.rs", "test.??"));
    assert!(!matches_glob("test.rs", "test.?"));
}

#[test]
fn test_matches_glob_double_star() {
    assert!(matches_glob("src/main.rs", "src/**"));
    assert!(matches_glob("src/lib/mod.rs", "src/**"));
    assert!(matches_glob("node_modules/pkg/index.js", "node_modules/*"));
}

#[test]
fn test_matches_glob_path() {
    assert!(matches_glob("src/main.rs", "src/*.rs"));
    assert!(!matches_glob("lib/main.rs", "src/*.rs"));
}

/// Regression for #249: byte-index slicing panics on multi-byte
/// characters inside filenames like `dialogue_line__冰糖.mp3`.
#[test]
fn test_matches_glob_unicode_filename() {
    let filename = "dialogue_line__冰糖.mp3";
    // The filename should match *.mp3 without panicking.
    assert!(matches_glob(filename, "*.mp3"));
    // Asterisk matching against multi-byte characters must succeed.
    assert!(matches_glob(filename, "dialogue_line__*"));
    // Literal multi-byte characters inside the pattern must match.
    assert!(matches_glob(filename, "*冰糖*"));
    // Non-matching pattern must not panic either.
    assert!(!matches_glob(filename, "nonexistent*"));
}

#[tokio::test]
async fn test_grep_files_basic() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Create test files
    fs::write(
        tmp.path().join("test.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .expect("write");
    fs::write(
        tmp.path().join("lib.rs"),
        "pub fn hello() {}\npub fn world() {}\n",
    )
    .expect("write");

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "fn"}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("main"));
    assert!(result.content.contains("hello"));
}

#[tokio::test]
async fn test_grep_files_with_context() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    fs::write(
        tmp.path().join("test.txt"),
        "line1\nline2\nMATCH\nline4\nline5\n",
    )
    .expect("write");

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "MATCH", "context_lines": 1}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("line2")); // context before
    assert!(result.content.contains("line4")); // context after

    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    let matches = parsed["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["context_before"], "line2");
    assert_eq!(matches[0]["context_after"], "line4");
    assert!(matches[0]["context_before"].is_string());
    assert!(matches[0]["context_after"].is_string());
}

#[tokio::test]
async fn test_grep_files_multi_line_context_remains_arrays() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    fs::write(tmp.path().join("test.txt"), "a\nb\nMATCH\nd\ne\n").expect("write");

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "MATCH", "context_lines": 2}), &ctx)
        .await
        .expect("execute");

    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    let matches = parsed["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["context_before"], json!(["a", "b"]));
    assert_eq!(matches[0]["context_after"], json!(["d", "e"]));
}

#[tokio::test]
async fn test_grep_files_case_insensitive() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    fs::write(
        tmp.path().join("test.txt"),
        "Hello World\nHELLO WORLD\nhello world\n",
    )
    .expect("write");

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "hello", "case_insensitive": true}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    // Should find all 3 lines
    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["total_matches"].as_u64().unwrap(), 3);
}

#[tokio::test]
async fn test_grep_files_include_filter() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    fs::write(tmp.path().join("test.rs"), "fn test() {}\n").expect("write");
    fs::write(tmp.path().join("test.js"), "function test() {}\n").expect("write");

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "test", "include": ["*.rs"]}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    // Should only match .rs file
    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    let matches = parsed["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    let file = matches[0]["file"].as_str().unwrap();
    assert!(
        file.rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
    );
}

#[tokio::test]
#[cfg(unix)]
async fn test_grep_files_does_not_follow_symlinked_files() {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().join("workspace");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&root).expect("mkdir workspace");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    let outside_file = outside.join("secret.txt");
    fs::write(&outside_file, "NEEDLE\n").expect("write outside");
    std::os::unix::fs::symlink(&outside_file, root.join("secret.txt")).expect("symlink");

    let ctx = ToolContext::new(root);
    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "NEEDLE"}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["total_matches"].as_u64().unwrap(), 0);
    assert_eq!(parsed["files_searched"].as_u64().unwrap(), 0);
}

#[tokio::test]
#[cfg(unix)]
async fn test_grep_files_default_mode_skips_symlinked_directories_but_keeps_real_files() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let real_dir = workspace.join("real");
    std::fs::create_dir_all(&real_dir).expect("mkdir workspace");
    fs::write(real_dir.join("needle.txt"), "NEEDLE\n").expect("write real file");
    std::os::unix::fs::symlink(&workspace, real_dir.join("loop")).expect("symlink loop");

    let ctx = ToolContext::new(workspace);
    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "NEEDLE"}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
    assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
    let matches = parsed["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert!(
        matches[0]["file"]
            .as_str()
            .unwrap()
            .ends_with("real/needle.txt")
    );
}

#[tokio::test]
#[cfg(unix)]
async fn test_grep_files_follow_symlinks_avoids_directory_cycles() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let real_dir = workspace.join("real");
    fs::create_dir_all(&real_dir).expect("mkdir");
    fs::write(real_dir.join("needle.txt"), "NEEDLE\n").expect("write");
    std::os::unix::fs::symlink(&workspace, real_dir.join("loop")).expect("symlink loop");

    let ctx = ToolContext::new(workspace).with_follow_symlinks(true);
    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "NEEDLE"}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
    assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
    let matches = parsed["matches"].as_array().unwrap();
    assert!(matches[0]["file"].as_str().unwrap().ends_with("needle.txt"));
}

#[tokio::test]
async fn test_grep_files_invalid_regex() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let tool = GrepFilesTool;
    let result = tool.execute(json!({"pattern": "[invalid"}), &ctx).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_grep_files_respects_cancel_token() {
    let tmp = tempdir().expect("tempdir");
    fs::write(tmp.path().join("test.txt"), "needle\n").expect("write");
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();
    let ctx = ToolContext::new(tmp.path().to_path_buf()).with_cancel_token(cancel_token);

    let tool = GrepFilesTool;
    let err = tool
        .execute(json!({"pattern": "needle"}), &ctx)
        .await
        .expect_err("cancelled grep should return an error");

    assert!(
        format!("{err:?}").contains("cancelled"),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_grep_files_streaming_stops_at_max_results() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Two files with many matches each; the walk must stop once the
    // budget is exhausted without dropping context for the last match.
    for name in ["a.txt", "b.txt"] {
        let body: String = (1..=20).map(|n| format!("needle {n}\n")).collect();
        fs::write(tmp.path().join(name), body).expect("write");
    }

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "needle", "max_results": 5}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    let matches = parsed["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 5);
    assert_eq!(parsed["total_matches"].as_u64().unwrap(), 5);
    // All five matches must come from the first file walked, in file
    // order (streaming preserves walk order).
    let first_file = matches[0]["file"].as_str().unwrap().to_string();
    for m in matches {
        assert_eq!(m["file"].as_str().unwrap(), first_file);
    }
    // The final in-budget match still gets its full after-context even
    // though the match budget was exhausted on it.
    assert_eq!(
        matches[4]["context_after"],
        json!(["needle 6", "needle 7"]),
        "last match must keep after-context lines"
    );
}

#[tokio::test]
async fn test_grep_files_ring_buffer_context_matches_full_read() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Matches at the start, middle, and end of the file exercise the
    // partial before-context (ring not yet full) and truncated
    // after-context (EOF) paths.
    fs::write(
        tmp.path().join("ctx.txt"),
        "MATCH first\nb1\nb2\nb3\nMATCH mid\na1\na2\na3\nMATCH last\n",
    )
    .expect("write");

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "MATCH", "context_lines": 2}), &ctx)
        .await
        .expect("execute");

    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    let matches = parsed["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0]["context_before"], json!([]));
    assert_eq!(matches[0]["context_after"], json!(["b1", "b2"]));
    assert_eq!(matches[1]["context_before"], json!(["b2", "b3"]));
    assert_eq!(matches[1]["context_after"], json!(["a1", "a2"]));
    assert_eq!(matches[2]["context_before"], json!(["a2", "a3"]));
    assert_eq!(matches[2]["context_after"], json!([]));
    assert_eq!(matches[2]["line_number"].as_u64().unwrap(), 9);
}

#[tokio::test]
async fn test_grep_files_streaming_skips_invalid_utf8_files() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Invalid UTF-8 after a matching line: the whole file must be
    // skipped, matching the historical read_to_string behavior.
    fs::write(
        tmp.path().join("binary.txt"),
        [b"needle\n".as_slice(), &[0xFF, 0xFE, 0x00]].concat(),
    )
    .expect("write");
    fs::write(tmp.path().join("clean.txt"), "needle\n").expect("write");

    let tool = GrepFilesTool;
    let result = tool
        .execute(json!({"pattern": "needle"}), &ctx)
        .await
        .expect("execute");

    let parsed: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(parsed["total_matches"].as_u64().unwrap(), 1);
    assert_eq!(parsed["files_searched"].as_u64().unwrap(), 1);
    let matches = parsed["matches"].as_array().unwrap();
    assert!(matches[0]["file"].as_str().unwrap().ends_with("clean.txt"));
}

#[test]
fn test_grep_files_tool_properties() {
    let tool = GrepFilesTool;
    assert_eq!(tool.name(), "grep_files");
    assert!(tool.is_read_only());
    assert!(tool.is_sandboxable());
    assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
}

#[test]
fn test_parallel_support_flags() {
    let tool = GrepFilesTool;
    assert!(tool.supports_parallel());
}
