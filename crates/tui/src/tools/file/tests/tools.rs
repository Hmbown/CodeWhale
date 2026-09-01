use super::*;
use tempfile::tempdir;

async fn read_before_edit(ctx: &ToolContext, path: &str) {
    ReadFileTool
        .execute(json!({"path": path}), ctx)
        .await
        .expect("read before edit");
}

#[tokio::test]
async fn test_read_file_tool() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Create a test file
    let test_file = tmp.path().join("test.txt");
    fs::write(&test_file, "hello world").expect("write");

    let tool = ReadFileTool;
    let result = tool
        .execute(json!({"path": "test.txt"}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    // #3979: a small-file read now leads with the snapshot hash the edit
    // guard verifies against, then the contents verbatim.
    assert_eq!(
        result.content,
        format!(
            "content_hash=\"{}\"\nhello world",
            super::content_hash(b"hello world")
        )
    );
}

// This test deliberately serializes process-global environment changes
// while awaiting the tool path.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn read_file_denies_codewhale_config_backups_and_secret_store() {
    let _env_lock = crate::test_support::lock_test_env();
    let tmp = tempdir().expect("tempdir");
    let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
    let _config_path = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
    let _legacy_config_path = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

    fs::write(tmp.path().join("config.toml"), "api_key = \"secret\"\n").expect("write config");
    fs::write(
        tmp.path().join("config.toml.bak"),
        "api_key = \"old-secret\"\n",
    )
    .expect("write config backup");
    fs::create_dir_all(tmp.path().join("secrets")).expect("create secrets dir");
    fs::write(
        tmp.path().join("secrets").join("secrets.json"),
        r#"{"provider":"secret"}"#,
    )
    .expect("write file keyring");
    fs::write(tmp.path().join("notes.txt"), "ordinary workspace data")
        .expect("write ordinary file");

    let ctx = ToolContext::new(tmp.path().to_path_buf());
    for path in ["config.toml", "config.toml.bak", "secrets/secrets.json"] {
        let err = ReadFileTool
            .execute(json!({"path": path}), &ctx)
            .await
            .expect_err("credential-bearing CodeWhale file must be denied");
        let message = err.to_string();
        assert!(message.contains("cannot expose Codewhale"), "{message}");
        assert!(message.contains("codewhale config list"), "{message}");
    }

    let ordinary = ReadFileTool
        .execute(json!({"path": "notes.txt"}), &ctx)
        .await
        .expect("ordinary workspace file should remain readable");
    assert!(
        ordinary.content.ends_with("ordinary workspace data"),
        "{}",
        ordinary.content
    );
}

#[tokio::test]
async fn read_file_ocr_extracts_text_from_image_when_backend_exists() {
    if !crate::tools::image_ocr::ocr_available() {
        return;
    }
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr_hello.png");
    if !fixture.exists() {
        return;
    }
    let tmp = tempdir().expect("tempdir");
    fs::copy(&fixture, tmp.path().join("ocr_hello.png")).expect("copy fixture");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let result = match ReadFileTool
        .execute(json!({"path": "ocr_hello.png"}), &ctx)
        .await
    {
        Ok(result) => result,
        Err(err) => {
            // Name is when_backend_exists — skip if live OCR fails after
            // the availability probe (restricted Vision, etc.).
            let msg = err.to_string();
            let _skip_reason = format!("OCR backend probe passed but read_file OCR failed: {msg}");
            let _ = &_skip_reason;
            return;
        }
    };

    assert!(result.success);
    assert!(result.content.contains("<image_ocr"));
    let normalized = result.content.to_uppercase();
    assert!(
        normalized.contains("HELLO") && normalized.contains("OCR"),
        "expected OCR text in read_file result, got {:?}",
        result.content
    );
}

#[test]
fn parse_pages_arg_accepts_single_page() {
    assert_eq!(parse_pages_arg("3"), Some((3, 3)));
    assert_eq!(parse_pages_arg("  7  "), Some((7, 7)));
}

#[test]
fn parse_pages_arg_accepts_range() {
    assert_eq!(parse_pages_arg("1-5"), Some((1, 5)));
    assert_eq!(parse_pages_arg("10-20"), Some((10, 20)));
    // Whitespace around either side of the dash is tolerated so
    // hand-typed `pages: "1 - 5"` still works.
    assert_eq!(parse_pages_arg(" 1 - 5 "), Some((1, 5)));
}

#[test]
fn parse_pages_arg_rejects_invalid_ranges() {
    // Caller would otherwise feed `pdftotext -f 5 -l 1`, which
    // prints nothing — fail loudly so the model can re-issue.
    assert!(parse_pages_arg("5-1").is_none(), "end < start must reject");
    // 0-indexed pages aren't a thing in pdftotext; reject so the
    // caller doesn't get a confusing "no output" silent fail.
    assert!(
        parse_pages_arg("0").is_none(),
        "zero single-page must reject"
    );
    assert!(parse_pages_arg("0-3").is_none(), "zero start must reject");
    // Empty / whitespace-only / non-numeric inputs must reject.
    assert!(parse_pages_arg("").is_none());
    assert!(parse_pages_arg("   ").is_none());
    assert!(parse_pages_arg("abc").is_none());
    assert!(parse_pages_arg("3.5").is_none(), "floats must reject");
}

#[test]
fn parse_pages_arg_rejects_half_open_ranges() {
    // Half-open ranges like `1-` or `-5` are almost certainly a
    // typo for `1-N`/`N` rather than intentional input. Reject
    // them rather than silently extending to u32::MAX or 0.
    assert!(parse_pages_arg("1-").is_none());
    assert!(parse_pages_arg("-5").is_none());
    assert!(parse_pages_arg("-").is_none());
}

#[test]
fn parse_pages_arg_rejects_negative_numbers() {
    // u32::parse on a negative literal returns Err, so the
    // function reports `None` rather than wrapping into a giant
    // positive number — defensive but worth pinning.
    assert!(parse_pages_arg("-3-5").is_none());
}

#[tokio::test]
async fn test_read_file_not_found() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let tool = ReadFileTool;
    let result = tool.execute(json!({"path": "nonexistent.txt"}), &ctx).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn read_file_small_file_returns_unwrapped_contents() {
    // Small files (≤ 200 lines AND ≤ 16KB, no explicit range) keep
    // the historical "return contents unchanged" behavior so
    // existing prompts don't suddenly see <file> tags appear.
    // Harvested from #1451 — pin the fast-path contract.
    //
    // #3979 added one `content_hash="…"` header line ahead of the contents:
    // the guard is useless if the common read path cannot report a hash, and
    // this branch has no `<file>` envelope to carry it as an attribute. The
    // contents themselves are still verbatim and still unwrapped.
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("small.txt");
    fs::write(&file, "line 1\nline 2\nline 3\n").expect("write");
    let tool = ReadFileTool;
    let result = tool
        .execute(json!({ "path": "small.txt" }), &ctx)
        .await
        .expect("execute");
    assert!(result.success);
    assert_eq!(
        result.content,
        format!(
            "content_hash=\"{}\"\nline 1\nline 2\nline 3\n",
            super::content_hash(b"line 1\nline 2\nline 3\n")
        )
    );
    assert!(
        !result.content.contains("<file"),
        "small-file fast path must not wrap output"
    );
}

#[tokio::test]
async fn read_file_explicit_range_wraps_in_file_tag_with_one_based_lines() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("ranged.txt");
    let body: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    fs::write(&file, &body).expect("write");
    let tool = ReadFileTool;
    let result = tool
        .execute(
            json!({ "path": "ranged.txt", "start_line": 3, "max_lines": 4 }),
            &ctx,
        )
        .await
        .expect("execute");
    assert!(result.success);
    assert!(
        result.content.contains("shown_lines=\"3-6\""),
        "1-based inclusive range must be reflected in shown_lines: {}",
        result.content
    );
    assert!(
        result.content.contains("next_start_line=\"7\""),
        "next_start_line must point one past the last shown line: {}",
        result.content
    );
    assert!(
        result.content.contains("     3│ line 3"),
        "rendered lines must start at the requested line number"
    );
    assert!(
        result.content.contains("     6│ line 6"),
        "rendered lines must end at the last in-range line"
    );
    assert!(
        !result.content.contains("     7│ line 7"),
        "lines past max_lines must be excluded"
    );
    assert!(result.content.contains("truncated=\"true\""));
}

#[tokio::test]
async fn read_file_range_beyond_total_returns_no_content_sentinel() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("short.txt");
    fs::write(&file, "only\nthree\nlines\n").expect("write");
    let tool = ReadFileTool;
    let result = tool
        .execute(json!({ "path": "short.txt", "start_line": 99 }), &ctx)
        .await
        .expect("execute");
    assert!(
        result.success,
        "out-of-range must not raise — it's a sentinel"
    );
    assert!(result.content.contains("[NO CONTENT]"));
    assert!(result.content.contains("shown_lines=\"none\""));
    assert!(result.content.contains("truncated=\"false\""));
}

/// 2026-08-04 review: a `start_line:"1200"` string (or any wrong type) used
/// to fall back SILENTLY to the defaults, returning lines 1-500 — the head
/// of the file dressed up as the window the model asked for. Wrong types
/// are errors, matching the shared `optional_u64` contract.
#[tokio::test]
async fn read_file_refuses_wrongly_typed_range_params_instead_of_defaulting() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    fs::write(tmp.path().join("any.txt"), "x\ny\nz\n").expect("write");
    let tool = ReadFileTool;
    for bad in [json!("1200"), json!(-5), json!(2.5), json!([1200])] {
        let err = tool
            .execute(json!({ "path": "any.txt", "start_line": bad }), &ctx)
            .await
            .expect_err("wrongly typed start_line must error, never default");
        assert!(
            err.to_string().contains("start_line"),
            "error names the field: {err}"
        );
        let err = tool
            .execute(json!({ "path": "any.txt", "max_lines": bad }), &ctx)
            .await
            .expect_err("wrongly typed max_lines must error, never default");
        assert!(
            err.to_string().contains("max_lines"),
            "error names the field: {err}"
        );
    }
    // Null still reads as absent, consistent with the strictness lane.
    let ok = tool
        .execute(json!({ "path": "any.txt", "start_line": null }), &ctx)
        .await
        .expect("null is absence, not a type error");
    assert!(ok.success);
}

#[tokio::test]
async fn read_file_rejects_zero_start_line_and_zero_max_lines() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    fs::write(tmp.path().join("any.txt"), "x\n").expect("write");
    let tool = ReadFileTool;
    let zero_start = tool
        .execute(json!({ "path": "any.txt", "start_line": 0 }), &ctx)
        .await;
    assert!(zero_start.is_err(), "start_line=0 must error (1-based)");
    let zero_max = tool
        .execute(json!({ "path": "any.txt", "max_lines": 0 }), &ctx)
        .await;
    assert!(zero_max.is_err(), "max_lines=0 must error");
}

#[tokio::test]
async fn read_file_byte_truncation_keeps_head_and_tail() {
    // Long lines force the 16 KiB bound before the line cap. The model must
    // see both ends of the window (qwen-style head = budget/5 + tail) and the
    // recovery note must name the original path for a re-read.
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("wide.txt");
    let body: String = (1..=40)
        .map(|n| format!("LINE{n}_START {} LINE{n}_END\n", "x".repeat(600)))
        .collect();
    assert!(body.len() > 16 * 1024, "fixture must exceed 16KB");
    fs::write(&file, &body).expect("write");

    let tool = ReadFileTool;
    let result = tool
        .execute(
            json!({ "path": "wide.txt", "start_line": 1, "max_lines": 40 }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("truncated=\"true\""));
    assert!(
        result.content.contains("LINE1_START"),
        "head of the window must survive: {}",
        &result.content[..result.content.len().min(400)]
    );
    assert!(
        result.content.contains("LINE40_END") || result.content.contains("LINE40_START"),
        "tail of the window must survive: {}",
        &result.content[result.content.len().saturating_sub(400)..]
    );
    assert!(
        result.content.contains("[CONTENT TRUNCATED]"),
        "head/tail separator missing: {}",
        result.content
    );
    assert!(
        result.content.contains("path=\"wide.txt\""),
        "recovery path must name the file: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains("Re-read narrower windows to see the middle"),
        "byte-truncation recovery note must give actionable advice: {}",
        result.content
    );
    assert!(
        result.content.contains("offset=1 limit=20"),
        "the note names a concrete narrower window: {}",
        result.content
    );
    // Middle of the window should be the part omitted under a head+tail budget.
    assert!(
        !result.content.contains("LINE20_START") || result.content.contains("[CONTENT TRUNCATED]"),
        "expected truncation of the middle: {}",
        result.content
    );
}

#[tokio::test]
async fn read_file_clamps_max_lines_to_hard_cap() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("bigish.txt");
    let body: String = (1..=600).map(|n| format!("L{n}\n")).collect();
    fs::write(&file, &body).expect("write");
    let tool = ReadFileTool;
    let result = tool
        .execute(json!({ "path": "bigish.txt", "max_lines": 5000 }), &ctx)
        .await
        .expect("execute");
    // Hard cap is 500 lines; line 500 must appear, line 501 must not.
    assert!(
        result.content.contains("   500│ L500"),
        "line 500 should be in the window (max_lines clamped to 500)"
    );
    assert!(
        !result.content.contains("   501│ L501"),
        "line 501 must be outside the clamped window"
    );
    assert!(result.content.contains("next_start_line=\"501\""));
    assert!(result.content.contains("truncated=\"true\""));
}

#[tokio::test]
async fn read_file_large_file_without_range_uses_default_window() {
    // A file over 200 lines / 16KB with no explicit range still
    // gets the default window, not the unbounded raw content —
    // this is the entire point of the patch (token-budget control).
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("big.txt");
    let body: String = (1..=250).map(|n| format!("row {n}\n")).collect();
    fs::write(&file, &body).expect("write");
    let tool = ReadFileTool;
    let result = tool
        .execute(json!({ "path": "big.txt" }), &ctx)
        .await
        .expect("execute");
    // 250 rows is ~1.7 KB — far inside the 16 KB byte budget — so it reads in
    // ONE call. The old 200-line default truncated here and charged a second
    // round trip to fetch 50 lines, which is what this change removes.
    // No `<file …>` envelope: at 250 lines / ~1.7 KB it now takes the
    // whole-file path and comes back as plain text, which is the point.
    assert!(result.content.contains("row 1"));
    assert!(result.content.contains("row 250"));
    assert!(
        !result.content.contains("next_start_line"),
        "a 250-line, ~1.7 KB file must not window: {}",
        result.content
    );

    // Past the line cap it still windows, because the cap is a real guard for
    // pathologically short lines.
    let many = tmp.path().join("many.txt");
    let body: String = (1..=600).map(|n| format!("row {n}\n")).collect();
    fs::write(&many, &body).expect("write");
    let windowed = tool
        .execute(json!({ "path": "many.txt" }), &ctx)
        .await
        .expect("execute");
    assert!(windowed.content.contains("shown_lines=\"1-500\""));
    assert!(windowed.content.contains("next_start_line=\"501\""));
}

#[tokio::test]
async fn read_file_streamed_range_on_large_file_matches_windowed_contract() {
    // Over 16KB forces the streamed BufRead path even without an
    // explicit range; assert the ranged output stays byte-compatible
    // with the historical full-read implementation.
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("large.txt");
    let body: String = (1..=2000)
        .map(|n| format!("line {n} {}\n", "x".repeat(20)))
        .collect();
    assert!(body.len() > 16 * 1024, "fixture must exceed 16KB");
    fs::write(&file, &body).expect("write");

    let tool = ReadFileTool;
    let result = tool
        .execute(
            json!({ "path": "large.txt", "start_line": 1500, "max_lines": 10 }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("total_lines=\"2000\""));
    assert!(result.content.contains("shown_lines=\"1500-1509\""));
    assert!(result.content.contains("next_start_line=\"1510\""));
    assert!(result.content.contains("  1500│ line 1500"));
    assert!(result.content.contains("  1509│ line 1509"));
    assert!(!result.content.contains("  1510│"));
    assert!(result.content.contains(
        "[TRUNCATED] Showing lines 1500-1509 of 2000. To continue, call read with path=\"large.txt\" offset=1510 limit=10"
    ));
    assert!(!result.content.contains("read_file"), "{}", result.content);

    // Default window (no range) on the same large file starts at line 1.
    let default_window = tool
        .execute(json!({ "path": "large.txt" }), &ctx)
        .await
        .expect("execute");
    assert!(default_window.content.contains("shown_lines=\"1-500\""));
    assert!(default_window.content.contains("next_start_line=\"501\""));
    assert!(default_window.content.contains("     1│ line 1"));

    // Paging past EOF returns the no-content sentinel, not an error.
    let past_end = tool
        .execute(json!({ "path": "large.txt", "start_line": 5000 }), &ctx)
        .await
        .expect("execute");
    assert!(past_end.content.contains("[NO CONTENT]"));
    assert!(past_end.content.contains("shown_lines=\"none\""));
}

#[tokio::test]
async fn read_file_streamed_range_rejects_invalid_utf8_like_full_read() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let file = tmp.path().join("mixed.bin");
    // Valid first lines, invalid bytes later: the streamed path must
    // still fail the whole read like read_to_string did.
    let mut bytes = b"good line\n".repeat(5);
    bytes.extend_from_slice(&[0xFF, 0xFE, b'\n']);
    fs::write(&file, &bytes).expect("write");

    let err = ReadFileTool
        .execute(
            json!({ "path": "mixed.bin", "start_line": 1, "max_lines": 2 }),
            &ctx,
        )
        .await
        .expect_err("invalid UTF-8 must error");
    let message = err.to_string();
    assert!(message.contains("Failed to read"), "{message}");
    assert!(message.contains("valid UTF-8"), "{message}");
}

#[tokio::test]
async fn test_read_file_missing_path() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let tool = ReadFileTool;
    let result = tool.execute(json!({}), &ctx).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string()
            .contains("Failed to validate input: missing required field 'path'")
    );
}

#[test]
fn pdf_detected_by_extension() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("paper.PDF");
    fs::write(&path, b"not really a pdf, but extension says yes").unwrap();
    assert!(is_pdf(&path).unwrap());
}

#[test]
fn pdf_detected_by_magic_bytes_without_extension() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("blob");
    fs::write(&path, b"%PDF-1.7\nrest of bytes").unwrap();
    assert!(is_pdf(&path).unwrap());
}

#[test]
fn non_pdf_not_detected() {
    let tmp = tempdir().expect("tempdir");
    let path = tmp.path().join("notes.txt");
    fs::write(&path, "hello").unwrap();
    assert!(!is_pdf(&path).unwrap());
}

#[test]
fn pages_arg_parses_single_and_range() {
    assert_eq!(parse_pages_arg("5"), Some((5, 5)));
    assert_eq!(parse_pages_arg("1-10"), Some((1, 10)));
    assert_eq!(parse_pages_arg(" 3 - 7 "), Some((3, 7)));
    assert_eq!(parse_pages_arg("0"), None);
    assert_eq!(parse_pages_arg("10-3"), None);
    assert_eq!(parse_pages_arg(""), None);
    assert_eq!(parse_pages_arg("abc"), None);
}

/// Sample PDF shipped with the repo for parity tests against the
/// pure-Rust extractor. 38 pages, born-digital LaTeX (arXiv 2512.24601).
/// Path is workspace-root-relative because the fixture lives outside
/// the tui crate.
const SAMPLE_PDF_PATH: &str = "../../docs/2512.24601v2.pdf";

fn sample_pdf_present() -> bool {
    std::path::Path::new(SAMPLE_PDF_PATH).exists()
}

#[test]
fn clean_pdf_text_collapses_consecutive_blank_lines() {
    let raw = "line1\n\n\n\n\nline2\n\n\nline3";
    let cleaned = super::clean_pdf_text(raw);
    assert_eq!(cleaned, "line1\n\nline2\n\nline3");
}

#[test]
fn clean_pdf_text_replaces_nul_bytes_with_replacement_char() {
    let raw = "hello\0world";
    let cleaned = super::clean_pdf_text(raw);
    assert!(!cleaned.contains('\0'));
    assert!(cleaned.contains('\u{FFFD}'));
}

#[test]
fn clean_pdf_text_replaces_non_breaking_spaces() {
    let raw = "hello\u{A0}world";
    let cleaned = super::clean_pdf_text(raw);
    assert!(!cleaned.contains('\u{A0}'));
    assert_eq!(cleaned, "hello world");
}

#[test]
fn clean_pdf_text_trims_trailing_whitespace() {
    let raw = "hello   ";
    let cleaned = super::clean_pdf_text(raw);
    assert_eq!(cleaned, "hello");
}

#[test]
fn clean_pdf_text_preserves_leading_indentation() {
    let raw = "   indented line\nregular line";
    let cleaned = super::clean_pdf_text(raw);
    assert_eq!(cleaned, "   indented line\nregular line");
}

#[tokio::test]
async fn read_file_pdf_path_uses_optional_pdftotext_adapter() {
    if !sample_pdf_present() || crate::dependencies::resolve_pdftotext().is_none() {
        return;
    }
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
    let ctx = ToolContext::new(workspace);
    let result = ReadFileTool
        .execute(json!({"path": "docs/2512.24601v2.pdf", "pages": "1"}), &ctx)
        .await
        .expect("execute");
    assert!(result.success);
    assert!(
        result.content.contains("Recursive Language Models"),
        "page-1 extraction must surface the title"
    );
}

#[tokio::test]
async fn test_write_file_tool() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let tool = WriteFileTool;
    let result = tool
        .execute(
            json!({"path": "output.txt", "content": "test content"}),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);
    // New file → "Created …" summary; the unified diff above the summary
    // primes the TUI's diff-aware renderer (#505).
    assert!(result.content.contains("Created"), "{}", result.content);
    assert!(result.content.contains("--- a/"), "{}", result.content);
    assert!(
        result.content.contains("+test content"),
        "{}",
        result.content
    );
    let mutation = &result.metadata.as_ref().expect("metadata")["mutation"];
    assert_eq!(
        mutation["files"],
        json!([{ "path": "output.txt", "outcome": "created" }])
    );
    assert!(
        mutation["diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("--- a/output.txt")),
        "{mutation}"
    );
    assert!(
        !mutation["diff"]
            .as_str()
            .unwrap_or_default()
            .contains(&tmp.path().display().to_string()),
        "receipt headers must not expose the resolved host path: {mutation}"
    );

    // Verify file was written
    let written = fs::read_to_string(tmp.path().join("output.txt")).expect("read");
    assert_eq!(written, "test content");
}

#[tokio::test]
async fn test_write_file_creates_dirs() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let tool = WriteFileTool;
    let result = tool
        .execute(
            json!({"path": "subdir/nested/file.txt", "content": "nested content"}),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);

    // Verify nested file was created
    let written = fs::read_to_string(tmp.path().join("subdir/nested/file.txt")).expect("read");
    assert_eq!(written, "nested content");
}

#[cfg(unix)]
#[tokio::test]
async fn write_file_tool_new_file_matches_standard_creation_mode() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let control = tmp.path().join("control.txt");
    fs::write(&control, b"control").expect("write control");

    WriteFileTool
        .execute(
            json!({"path": "created.txt", "content": "from write_file"}),
            &ctx,
        )
        .await
        .expect("execute");

    let control_mode = fs::metadata(&control)
        .expect("control metadata")
        .permissions()
        .mode()
        & 0o777;
    let created_mode = fs::metadata(tmp.path().join("created.txt"))
        .expect("created metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(created_mode, control_mode);
}

#[cfg(unix)]
#[tokio::test]
async fn write_file_tool_preserves_existing_mode() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("shared.txt");
    fs::write(&path, b"before").expect("initial write");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o664)).expect("set shared permissions");

    WriteFileTool
        .execute(json!({"path": "shared.txt", "content": "after"}), &ctx)
        .await
        .expect("execute");

    let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o664);
    assert_eq!(fs::read_to_string(&path).expect("read"), "after");
}

#[cfg(unix)]
#[tokio::test]
async fn edit_file_tool_preserves_executable_bits() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("script.sh");
    fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("initial write");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .expect("set executable permissions");
    read_before_edit(&ctx, "script.sh").await;

    EditFileTool
        .execute(
            json!({
                "path": "script.sh",
                "search": "exit 0",
                "replace": "exit 1"
            }),
            &ctx,
        )
        .await
        .expect("execute");

    let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "#!/bin/sh\nexit 1\n"
    );
}

#[tokio::test]
async fn edit_file_refuses_brace_collapsed_match_arm_payload() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("arm.rs");
    let original = r#"match outcome {
            SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,
            SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Completed,
                ..
            } => {}
        }
"#;
    fs::write(&path, original).expect("write");
    read_before_edit(&ctx, "arm.rs").await;

    let search = r#"SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,"#;
    // Corrupted host payload: brace block collapsed to empty brackets.
    let replace = "[

            ] => {},";
    let err = EditFileTool
        .execute(
            json!({
                "path": "arm.rs",
                "search": search,
                "replace": replace,
            }),
            &ctx,
        )
        .await
        .expect_err("corrupted brace collapse must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("corrupted") || msg.contains("collapsed") || msg.contains("unbalanced"),
        "unexpected error: {msg}"
    );
    assert_eq!(fs::read_to_string(&path).expect("read"), original);
}

#[tokio::test]
async fn edit_file_preserves_rust_match_arm_braces() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("arm.rs");
    let original = r#"match outcome {
            SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,
            other => {}
        }
"#;
    fs::write(&path, original).expect("write");
    read_before_edit(&ctx, "arm.rs").await;

    let search = r#"SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,"#;
    let replace = r#"SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => {
                // stay active
                let _ = self.tx_event.send(Event::status("ok".into())).await;
            }"#;
    EditFileTool
        .execute(
            json!({
                "path": "arm.rs",
                "search": search,
                "replace": replace,
            }),
            &ctx,
        )
        .await
        .expect("brace-heavy replace must apply");
    let updated = fs::read_to_string(&path).expect("read");
    assert!(updated.contains("stay active"), "{updated}");
    assert!(
        updated.contains("SendMessageOutcome::Finished"),
        "{updated}"
    );
    assert!(
        !updated.contains("pause_goal_after_interruption"),
        "{updated}"
    );
}

#[tokio::test]
async fn test_edit_file_tool() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Create a file to edit
    let test_file = tmp.path().join("edit_me.txt");
    fs::write(&test_file, "hello world").expect("write");
    read_before_edit(&ctx, "edit_me.txt").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({"path": "edit_me.txt", "search": "hello", "replace": "hi"}),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("Replaced 1 occurrence"));
    // Inline diff (#505) — the unified diff lands above the summary
    // line so the TUI's diff-aware renderer kicks in.
    assert!(result.content.contains("--- a/"), "{}", result.content);
    assert!(
        result.content.contains("-hello world"),
        "{}",
        result.content
    );
    assert!(result.content.contains("+hi world"), "{}", result.content);
    let mutation = &result.metadata.as_ref().expect("metadata")["mutation"];
    assert_eq!(
        mutation["files"],
        json!([{ "path": "edit_me.txt", "outcome": "updated" }])
    );
    let receipt_diff = mutation["diff"].as_str().expect("receipt diff");
    assert!(receipt_diff.contains("--- a/edit_me.txt"), "{receipt_diff}");
    assert!(receipt_diff.contains("-hello world"), "{receipt_diff}");
    assert!(receipt_diff.contains("+hi world"), "{receipt_diff}");
    assert!(
        !receipt_diff.contains(&tmp.path().display().to_string()),
        "receipt headers must not expose the resolved host path: {receipt_diff}"
    );

    // Verify edit was applied
    let edited = fs::read_to_string(&test_file).expect("read");
    assert_eq!(edited, "hi world");
}

#[tokio::test]
async fn edit_file_matches_lf_search_in_crlf_file_and_preserves_crlf() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("crlf.py");
    fs::write(
        &test_file,
        b"def greet(name):\r\n    print(name)\r\n\r\ndef add(a, b):\r\n    return a + b\r\n",
    )
    .expect("write");
    read_before_edit(&ctx, "crlf.py").await;

    let result = EditFileTool
        .execute(
            json!({
                "path": "crlf.py",
                "search": "def add(a, b):\n    return a + b",
                "replace": "def add(a, b):\n    return a * b",
            }),
            &ctx,
        )
        .await
        .expect("LF model input should edit a CRLF file");

    assert!(result.success, "{}", result.content);
    assert_eq!(
        fs::read(&test_file).expect("read"),
        b"def greet(name):\r\n    print(name)\r\n\r\ndef add(a, b):\r\n    return a * b\r\n",
    );
}

#[test]
fn edit_file_sparse_crlf_positions_map_utf8_range_through_eof() {
    let original = "前\r\n尾";
    let (normalized, crlf_positions) = normalize_crlf_with_positions(original);

    assert_eq!(normalized, "前\n尾");
    assert_eq!(crlf_positions.as_deref(), Some(&[3][..]));
    assert_eq!(
        map_normalized_range((0, normalized.len()), crlf_positions.as_deref()),
        (0, original.len()),
    );
}

#[tokio::test]
async fn edit_file_maps_utf8_crlf_match_ending_at_eof() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("utf8-eof-crlf.txt");
    fs::write(&test_file, "前\r\n尾").expect("write");
    read_before_edit(&ctx, "utf8-eof-crlf.txt").await;

    EditFileTool
        .execute(
            json!({
                "path": "utf8-eof-crlf.txt",
                "search": "前\n尾",
                "replace": "始\n终",
            }),
            &ctx,
        )
        .await
        .expect("UTF-8 CRLF match should map through EOF");

    assert_eq!(fs::read(&test_file).expect("read"), "始\r\n终".as_bytes(),);
}

#[tokio::test]
async fn edit_file_normalizes_multiline_replacement_for_single_line_crlf_match() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("single-line-crlf.txt");
    fs::write(&test_file, b"alpha\r\nomega\r\n").expect("write");
    read_before_edit(&ctx, "single-line-crlf.txt").await;

    EditFileTool
        .execute(
            json!({
                "path": "single-line-crlf.txt",
                "search": "omega",
                "replace": "beta\ngamma",
            }),
            &ctx,
        )
        .await
        .expect("replacement should follow the file's CRLF style");

    assert_eq!(
        fs::read(&test_file).expect("read"),
        b"alpha\r\nbeta\r\ngamma\r\n",
    );
}

#[tokio::test]
async fn edit_file_normalizes_crlf_and_mixed_replacement_for_lf_file() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("lf.txt");
    fs::write(&test_file, b"alpha\nomega\n").expect("write");
    read_before_edit(&ctx, "lf.txt").await;

    EditFileTool
        .execute(
            json!({
                "path": "lf.txt",
                "search": "omega",
                "replace": "beta\r\ngamma\nfinal",
            }),
            &ctx,
        )
        .await
        .expect("replacement should follow the file's LF style");

    assert_eq!(
        fs::read(&test_file).expect("read"),
        b"alpha\nbeta\ngamma\nfinal\n",
    );
}

#[tokio::test]
async fn edit_file_rejects_logical_duplicate_across_lf_and_crlf() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("mixed.txt");
    let original = b"same\nblock\r\nsame\r\nblock\r\n";
    fs::write(&test_file, original).expect("write");
    read_before_edit(&ctx, "mixed.txt").await;

    let error = EditFileTool
        .execute(
            json!({
                "path": "mixed.txt",
                "search": "same\nblock",
                "replace": "changed",
            }),
            &ctx,
        )
        .await
        .expect_err("logical duplicates must remain non-unique");

    assert!(error.to_string().contains("matched 2"), "{error}");
    assert_eq!(fs::read(&test_file).expect("read"), original);
}

#[tokio::test]
async fn edit_file_combines_crlf_and_indentation_fuzzy_matching() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("fuzzy-crlf.txt");
    fs::write(&test_file, "前言\r\n    数据 = 1\r\n").expect("write");
    read_before_edit(&ctx, "fuzzy-crlf.txt").await;

    let result = EditFileTool
        .execute(
            json!({
                "path": "fuzzy-crlf.txt",
                "search": "前言\n        数据 = 1",
                "replace": "前言\n    数据 = 2",
            }),
            &ctx,
        )
        .await
        .expect("indentation fallback should compose with CRLF normalization");

    assert!(
        result.content.contains("fuzzy indentation match"),
        "{}",
        result.content
    );
    assert_eq!(
        fs::read(&test_file).expect("read"),
        "前言\r\n    数据 = 2\r\n".as_bytes(),
    );
}

#[tokio::test]
async fn edit_file_combines_crlf_and_punctuation_fuzzy_matching() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("punctuation-crlf.txt");
    fs::write(&test_file, "前言\r\n数据 \"x\"\r\n").expect("write");
    read_before_edit(&ctx, "punctuation-crlf.txt").await;

    let result = EditFileTool
        .execute(
            json!({
                "path": "punctuation-crlf.txt",
                "search": "前言\n数据 \u{201C}x\u{201D}",
                "replace": "前言\r\n数据 y\n下一行",
            }),
            &ctx,
        )
        .await
        .expect("punctuation fallback should compose with CRLF normalization");

    assert!(
        result.content.contains("fuzzy punctuation match"),
        "{}",
        result.content
    );
    assert_eq!(
        fs::read(&test_file).expect("read"),
        "前言\r\n数据 y\r\n下一行\r\n".as_bytes(),
    );
}

#[tokio::test]
async fn edit_file_rejects_line_ending_normalized_noop() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("noop-crlf.txt");
    let original = b"alpha\r\nbeta\r\n";
    fs::write(&test_file, original).expect("write");
    read_before_edit(&ctx, "noop-crlf.txt").await;

    let error = EditFileTool
        .execute(
            json!({
                "path": "noop-crlf.txt",
                "search": "alpha\nbeta",
                "replace": "alpha\r\nbeta",
            }),
            &ctx,
        )
        .await
        .expect_err("normalized no-op should be rejected");

    assert!(error.to_string().contains("no change intended"), "{error}");
    assert_eq!(fs::read(&test_file).expect("read"), original);
}

#[tokio::test]
async fn edit_file_requires_prior_read() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("blind.txt");
    fs::write(&test_file, "hello world").expect("write");

    let err = EditFileTool
        .execute(
            json!({"path": "blind.txt", "search": "hello", "replace": "hi"}),
            &ctx,
        )
        .await
        .expect_err("edit without read should fail");
    let message = err.to_string();
    assert!(message.contains("not been read"), "{message}");
    // The recovery has to be spelled as a call the model can make: `read_file`
    // was retired in v0.9.3 and the registry has no fuzzy resolve step.
    assert!(message.contains(r#"File with action="read""#), "{message}");
    assert!(!message.contains("read_file"), "{message}");

    let unchanged = fs::read_to_string(&test_file).expect("read");
    assert_eq!(unchanged, "hello world");
}

#[tokio::test]
async fn edit_file_rejects_stale_prior_read() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("stale.txt");
    fs::write(&test_file, "alpha beta").expect("write");
    read_before_edit(&ctx, "stale.txt").await;
    fs::write(&test_file, "alpha beta gamma").expect("external write");

    let err = EditFileTool
        .execute(
            json!({"path": "stale.txt", "search": "alpha", "replace": "omega"}),
            &ctx,
        )
        .await
        .expect_err("stale read should fail");
    let message = err.to_string();
    assert!(message.contains("changed since"), "{message}");
    assert!(message.contains(r#"File with action="read""#), "{message}");
    assert!(!message.contains("read_file"), "{message}");

    let unchanged = fs::read_to_string(&test_file).expect("read");
    assert_eq!(unchanged, "alpha beta gamma");
}

#[tokio::test]
async fn edit_file_rejects_non_unique_exact_match() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("multi.txt");
    fs::write(&test_file, "hello world hello").expect("write");
    read_before_edit(&ctx, "multi.txt").await;

    let err = EditFileTool
        .execute(
            json!({"path": "multi.txt", "search": "hello", "replace": "hi"}),
            &ctx,
        )
        .await
        .expect_err("non-unique exact match should fail");
    let message = err.to_string();
    assert!(message.contains("non-unique"), "{message}");
    assert!(message.contains("matched 2"), "{message}");
    // Recovery text must name the live surface. `read_file` is retired and
    // cannot dispatch (crates/tui/src/tools/registry.rs:2067).
    assert!(
        message.contains("call File with action=\"read\""),
        "{message}"
    );
    assert!(!message.contains("read_file"), "{message}");

    let unchanged = fs::read_to_string(&test_file).expect("read");
    assert_eq!(unchanged, "hello world hello");
}

/// `fuzz` on `edit` was an advertised parameter with no implementation: it
/// was parsed into `let _fuzz` and thrown away, and a live model read the
/// schema as offering "an optional fuzzy-matching flag for the search". The
/// advertisement is gone, so the name now means nothing to `edit` and is
/// refused like any other name with no known meaning — the fuzzy fallbacks it
/// appeared to control run unconditionally either way.
#[tokio::test]
async fn edit_file_refuses_the_retired_fuzz_parameter() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let test_file = tmp.path().join("fuzz_retired.txt");
    fs::write(&test_file, "hello world").expect("write");
    read_before_edit(&ctx, "fuzz_retired.txt").await;

    let err = EditFileTool
        .execute(
            json!({
                "path": "fuzz_retired.txt",
                "search": "hello",
                "replace": "hi",
                "fuzz": true,
            }),
            &ctx,
        )
        .await
        .expect_err("a parameter edit does not implement must be refused");
    let msg = err.to_string();
    assert!(msg.contains("fuzz"), "must name the parameter: {msg}");
    assert!(
        msg.contains("was not performed"),
        "must deny having edited: {msg}"
    );
    assert_eq!(
        fs::read_to_string(&test_file).expect("read"),
        "hello world",
        "a refused edit must not touch the file"
    );
}

#[tokio::test]
async fn test_edit_file_single_match_has_no_multi_match_warning() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("single.txt");
    fs::write(&test_file, "hello world").expect("write");
    read_before_edit(&ctx, "single.txt").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({"path": "single.txt", "search": "hello", "replace": "hi"}),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("Replaced 1 occurrence"));
    assert!(!result.content.contains("multiple matches were replaced"));
}

#[tokio::test]
async fn test_edit_file_fuzz_tolerates_leading_whitespace() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("fuzzy.txt");
    fs::write(
        &test_file,
        "fn main() {\n    if true {\n        let value = 1;\n    }\n}\n",
    )
    .expect("write");
    read_before_edit(&ctx, "fuzzy.txt").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({
                "path": "fuzzy.txt",
                "search": "if true {\n    let value = 1;\n}",
                "replace": "    if true {\n        let value = 2;\n    }"
            }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("fuzzy indentation match"));
    let edited = fs::read_to_string(&test_file).expect("read");
    assert_eq!(
        edited,
        "fn main() {\n    if true {\n        let value = 2;\n    }\n}\n"
    );
}

#[tokio::test]
async fn test_edit_file_fuzz_tolerates_leading_whitespace_after_multibyte_start() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("fuzzy_cjk.txt");
    fs::write(&test_file, "数据\n").expect("write");
    read_before_edit(&ctx, "fuzzy_cjk.txt").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({
                "path": "fuzzy_cjk.txt",
                "search": "    数据",
                "replace": "记录"
            }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("fuzzy indentation match"));
    let edited = fs::read_to_string(&test_file).expect("read");
    assert_eq!(edited, "记录\n");
}

#[tokio::test]
async fn test_edit_file_fuzz_tolerates_smart_quote_substitution() {
    // The file on disk has ASCII quotes. The search comes from a
    // browser paste with curly quotes. Exact match fails; the
    // punctuation-normalized fallback should still land the edit.
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("smart.rs");
    fs::write(&test_file, "let s = \"hello world\";\n").expect("write");
    read_before_edit(&ctx, "smart.rs").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({
                "path": "smart.rs",
                // \u{201C} \u{201D} are the curly double-quote pair.
                "search": "let s = \u{201C}hello world\u{201D};",
                "replace": "let s = \"hello universe\";"
            }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success, "fuzzy punctuation edit should succeed");
    assert!(
        result.content.contains("fuzzy punctuation match"),
        "expected punctuation-fuzz note, got: {}",
        result.content
    );
    let edited = fs::read_to_string(&test_file).expect("read");
    assert_eq!(edited, "let s = \"hello universe\";\n");
}

#[tokio::test]
async fn test_edit_file_fuzz_tolerates_smart_quote_after_multibyte_start() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("smart_cjk.md");
    fs::write(&test_file, "数据 \"x\"\n").expect("write");
    read_before_edit(&ctx, "smart_cjk.md").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({
                "path": "smart_cjk.md",
                "search": "数据 \u{201C}x\u{201D}",
                "replace": "数据 y"
            }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("fuzzy punctuation match"));
    let edited = fs::read_to_string(&test_file).expect("read");
    assert_eq!(edited, "数据 y\n");
}

#[tokio::test]
async fn test_edit_file_fuzz_tolerates_em_dash_and_nbsp() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("dash.md");
    // File has an ASCII hyphen and ASCII space.
    fs::write(&test_file, "alpha - beta\n").expect("write");
    read_before_edit(&ctx, "dash.md").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({
                "path": "dash.md",
                // Search uses em-dash + NBSP, common after a copy-paste
                // from a styled document.
                "search": "alpha\u{00A0}\u{2014}\u{00A0}beta",
                "replace": "alpha - gamma"
            }),
            &ctx,
        )
        .await
        .expect("execute");

    assert!(result.success);
    let edited = fs::read_to_string(&test_file).expect("read");
    assert_eq!(edited, "alpha - gamma\n");
}

#[tokio::test]
async fn test_edit_file_not_found() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Create a file without the search string
    let test_file = tmp.path().join("no_match.txt");
    fs::write(&test_file, "foo bar baz").expect("write");
    read_before_edit(&ctx, "no_match.txt").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({"path": "no_match.txt", "search": "hello", "replace": "hi"}),
            &ctx,
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not found"));
    assert!(err.to_string().contains("call File with action=\"read\""));
    assert!(!err.to_string().contains("read_file"));
}

#[tokio::test]
async fn test_edit_file_rejects_identical_search_and_replace() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("same.txt");
    fs::write(&test_file, "a := \"foo\"").expect("write");

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({
                "path": "same.txt",
                "search": "a := \"foo\"",
                "replace": "a := \"foo\""
            }),
            &ctx,
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("search and replace are identical"),
        "error must explain the no-op input: {err}"
    );
    // #5003 - the diagnostic must help the model self-correct: it should
    // size the payload and point at the root cause instead of a bare
    // "no change intended".
    assert!(
        err.contains("10 chars"),
        "error should size the payload: {err}"
    );
    assert!(
        err.contains("Recovery"),
        "error should offer recovery: {err}"
    );
    let unchanged = fs::read_to_string(&test_file).expect("read");
    assert_eq!(unchanged, "a := \"foo\"");
}

#[test]
fn test_c_preprocessor_rejects_missing_close() {
    let before = "#if FEATURE\nold code\n#endif\n";
    let after = "#if FEATURE\nnew code\n";
    assert_eq!(
        invalid_preprocessor_edit(Path::new("source.c"), before, after),
        Some(PREPROCESSOR_CONDITIONAL_ERROR)
    );
}

#[test]
fn test_c_preprocessor_rejects_extra_close() {
    let before = "#if FEATURE\nold code\n#endif\n";
    let after = "#if FEATURE\nnew code\n#endif\n#endif\n";
    assert_eq!(
        invalid_preprocessor_edit(Path::new("source.hpp"), before, after),
        Some(PREPROCESSOR_CONDITIONAL_ERROR)
    );
}

#[test]
fn test_c_preprocessor_allows_balanced_block_removal_and_insertion() {
    let block = "#ifdef FEATURE\nfeature();\n#endif\n";
    assert!(invalid_preprocessor_edit(Path::new("source.cc"), block, "").is_none());
    assert!(invalid_preprocessor_edit(Path::new("source.cc"), "", block).is_none());
}

#[test]
fn test_c_preprocessor_allows_in_block_edit() {
    let before = "#if FEATURE\nold_call();\n#endif\n";
    let after = "#if FEATURE\nnew_call();\n#endif\n";
    assert!(invalid_preprocessor_edit(Path::new("source.cxx"), before, after).is_none());
}

#[test]
fn test_non_c_directive_prose_is_not_validated() {
    let before = "#if this example is enabled\nexplanation\n#endif\n";
    let after = "#if this example is enabled\nupdated explanation\n";
    assert!(invalid_preprocessor_edit(Path::new("guide.md"), before, after).is_none());
}

#[test]
fn test_preview_search_for_error_truncates() {
    let long_line = "x".repeat(200);
    let search = format!("{long_line}\nsecond line\nthird line\nfourth line\n");
    let preview = preview_search_for_error(&search);
    assert!(preview.lines().count() <= 3);
    assert!(preview.contains("..."));
    assert!(!preview.contains("fourth line"));
}

#[tokio::test]
async fn test_edit_file_not_found_shows_search_preview() {
    // #5003 - when search misses, the error should preview the search text
    // so the model can compare what it searched for against the file.
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("preview.txt");
    fs::write(&test_file, "foo bar baz").expect("write");
    read_before_edit(&ctx, "preview.txt").await;

    let tool = EditFileTool;
    let result = tool
        .execute(
            json!({
                "path": "preview.txt",
                "search": "first line\nsecond line\n",
                "replace": "changed"
            }),
            &ctx,
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Search string not found"));
    assert!(
        err.contains("first line"),
        "error should preview search text: {err}"
    );
}

/// #157 / #5209 — `replacement` is an unambiguous synonym for `replace`, so
/// the edit the model asked for is the edit that lands. The #5209 guarantee
/// being protected is that the file and the receipt agree: a reported
/// replacement must correspond to a real one.
#[tokio::test]
async fn edit_file_accepts_replacement_alias_and_applies_the_edit() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    let test_file = tmp.path().join("test.txt");
    fs::write(&test_file, "hello world").expect("write");
    read_before_edit(&ctx, "test.txt").await;

    let result = EditFileTool
        .execute(
            json!({"path": "test.txt", "search": "hello", "replacement": "hi"}),
            &ctx,
        )
        .await
        .expect("replacement alias must be honored");

    assert!(result.success);
    assert_eq!(
        fs::read_to_string(&test_file).expect("read"),
        "hi world",
        "the receipt claimed an edit, so the file must actually carry it"
    );
}

/// Every cross-harness spelling of the two edit arguments resolves to the
/// same applied edit. A model that guesses from a different harness's prior
/// gets its work done instead of a rejection and a wasted turn (#5209).
#[tokio::test]
async fn edit_file_accepts_every_cross_harness_edit_alias() {
    for (search_key, replace_key) in [
        ("old_string", "new_string"),
        ("old_str", "new_str"),
        ("oldText", "newText"),
        ("old_text", "new_text"),
    ] {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let path = tmp.path().join("doc.md");
        fs::write(&path, "old text line\n").expect("write");
        read_before_edit(&ctx, "doc.md").await;

        let result = EditFileTool
            .execute(
                json!({
                    "path": "doc.md",
                    search_key: "old text line",
                    replace_key: "new text line",
                }),
                &ctx,
            )
            .await
            .unwrap_or_else(|err| panic!("{search_key}/{replace_key} must apply: {err}"));

        assert!(result.success, "{search_key}/{replace_key}");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "new text line\n",
            "{search_key}/{replace_key} must reach the file"
        );
    }
}

/// The unified `File` tool takes the same alias path as the inner tool, so
/// the model-facing surface and the dispatch target cannot disagree.
#[tokio::test]
async fn file_tool_action_edit_accepts_new_str_alias() {
    use crate::tools::file_tool::FileTool;

    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.md");
    fs::write(&path, "old text line\n").expect("write");
    read_before_edit(&ctx, "doc.md").await;

    let result = FileTool::with_patch("File")
        .execute(
            json!({
                "action": "edit",
                "path": "doc.md",
                "search": "old text line",
                "new_str": "new text line",
            }),
            &ctx,
        )
        .await
        .expect("File action=edit with new_str must apply");

    assert!(result.success);
    assert_eq!(fs::read_to_string(&path).expect("read"), "new text line\n");
}

/// An alias that contradicts an explicitly supplied canonical value is
/// ambiguous. Picking one would be the guess this whole path exists to
/// avoid, so it fails and changes nothing.
#[tokio::test]
async fn edit_file_rejects_alias_conflicting_with_canonical_name() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.md");
    fs::write(&path, "old text line\n").expect("write");
    read_before_edit(&ctx, "doc.md").await;

    let err = EditFileTool
        .execute(
            json!({
                "path": "doc.md",
                "search": "old text line",
                "replace": "one thing",
                "new_string": "a different thing",
            }),
            &ctx,
        )
        .await
        .expect_err("conflicting alias must not be silently resolved");

    let msg = err.to_string();
    assert!(
        msg.contains("`replace`") && msg.contains("`new_string`"),
        "must name both spellings: {msg}"
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "old text line\n",
        "nothing may change on an ambiguous call"
    );
}

/// An alias that merely repeats the canonical value is a harmless
/// duplicate, not a conflict.
#[tokio::test]
async fn edit_file_accepts_alias_agreeing_with_canonical_name() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.md");
    fs::write(&path, "old text line\n").expect("write");
    read_before_edit(&ctx, "doc.md").await;

    EditFileTool
        .execute(
            json!({
                "path": "doc.md",
                "search": "old text line",
                "replace": "new text line",
                "new_string": "new text line",
            }),
            &ctx,
        )
        .await
        .expect("agreeing duplicate must be accepted");

    assert_eq!(fs::read_to_string(&path).expect("read"), "new text line\n");
}

/// `file_path` is the other widespread spelling of `path` and is accepted on
/// every file action.
#[tokio::test]
async fn file_actions_accept_file_path_alias() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    WriteFileTool
        .execute(json!({"file_path": "note.txt", "content": "first\n"}), &ctx)
        .await
        .expect("write must accept file_path");
    assert_eq!(
        fs::read_to_string(tmp.path().join("note.txt")).expect("read"),
        "first\n"
    );

    let read = ReadFileTool
        .execute(json!({"file_path": "note.txt"}), &ctx)
        .await
        .expect("read must accept file_path");
    assert!(read.content.contains("first"));

    EditFileTool
        .execute(
            json!({"file_path": "note.txt", "search": "first", "replace": "second"}),
            &ctx,
        )
        .await
        .expect("edit must accept file_path");
    assert_eq!(
        fs::read_to_string(tmp.path().join("note.txt")).expect("read"),
        "second\n"
    );
}

/// `offset`/`limit` name the same read window as `start_line`/`max_lines`.
/// Before they were translated, a wrong guess was dropped and the model
/// silently got the head of the file instead of the window it asked for.
#[tokio::test]
async fn read_file_accepts_offset_and_limit_aliases() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let body: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    fs::write(tmp.path().join("many.txt"), &body).expect("write");

    let aliased = ReadFileTool
        .execute(json!({"path": "many.txt", "offset": 5, "limit": 3}), &ctx)
        .await
        .expect("offset/limit must be honored");
    let canonical = ReadFileTool
        .execute(
            json!({"path": "many.txt", "start_line": 5, "max_lines": 3}),
            &ctx,
        )
        .await
        .expect("canonical read");

    assert_eq!(
        aliased.content, canonical.content,
        "aliases must select the same window as the canonical names"
    );
    assert!(
        aliased.content.contains("line 5") && !aliased.content.contains("line 1\n"),
        "must start at the requested offset: {}",
        aliased.content
    );
}

/// #5209 — unknown keys on edit hard-error even when required fields are present.
#[tokio::test]
async fn edit_file_rejects_unexpected_parameter_names() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.md");
    fs::write(&path, "hello\n").expect("write");
    read_before_edit(&ctx, "doc.md").await;

    let err = EditFileTool
        .execute(
            json!({
                "path": "doc.md",
                "search": "hello",
                "replace": "hi",
                "mystery": true,
            }),
            &ctx,
        )
        .await
        .expect_err("unexpected params must hard-error");
    let msg = err.to_string();
    assert!(
        msg.contains("unexpected") && msg.contains("mystery"),
        "must name unexpected key: {msg}"
    );
    assert_eq!(fs::read_to_string(&path).expect("read"), "hello\n");
}

#[test]
fn edit_payload_allows_same_brace_delta_unbalanced_fragment() {
    // Same-delta unbalanced fragment: both sides open one more brace than
    // they close (typical mid-block edit).
    let search = "    handler({\n        a: 1,\n";
    let replace = "    handler({\n        a: 1,\n        b: 2,\n";
    assert!(
        edit_payload_looks_corrupted(search, replace).is_none(),
        "same brace delta unbalanced fragment must be allowed"
    );

    // Unbalanced-to-unbalanced with the same closing delta (e.g. near `});`).
    let search = "    done();\n    });\n";
    let replace = "    done();\n    cleanup();\n    });\n";
    assert!(
        edit_payload_looks_corrupted(search, replace).is_none(),
        "unbalanced-to-unbalanced with same delta (e.g. around `}});`) must be allowed"
    );
}

#[test]
fn edit_payload_rejects_divergent_brace_delta() {
    let search = "fn f() {\n    body\n}\n";
    let replace = "fn f() {\n    body\n"; // lost closing brace
    let reason =
        edit_payload_looks_corrupted(search, replace).expect("divergent brace delta must reject");
    assert!(
        reason.contains("brace balance") || reason.contains("unbalanced"),
        "reason should mention brace balance: {reason}"
    );
}

#[test]
fn edit_payload_still_rejects_empty_bracket_collapse() {
    let search = r#"SendMessageOutcome::Finished {
                status: TurnOutcomeStatus::Interrupted,
                ..
            } => self.pause_goal_after_interruption().await,"#;
    let replace = "[

            ] => {},";
    assert!(
        edit_payload_looks_corrupted(search, replace).is_some(),
        "empty bracket collapse must still fail closed"
    );
}

#[test]
fn edit_payload_still_rejects_extreme_shrinkage() {
    // Many nested braces in search, collapsed to a tiny stub that lost opens.
    let search = "fn long_match_arm() {\n".to_string()
        + &"    if cond { statement(); }\n".repeat(20)
        + "}\n";
    let replace = "fn long_match_arm() {}\n";
    assert!(
        search.len() >= 80,
        "fixture must be long enough for shrinkage guard"
    );
    assert!(
        edit_payload_looks_corrupted(&search, replace).is_some(),
        "extreme shrinkage with lost braces must still fail closed"
    );
}

#[tokio::test]
async fn test_list_dir_tool() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Create some files and directories
    fs::write(tmp.path().join("file1.txt"), "").expect("write");
    fs::write(tmp.path().join("file2.txt"), "").expect("write");
    fs::create_dir(tmp.path().join("subdir")).expect("mkdir");

    let tool = ListDirTool;
    let result = tool.execute(json!({}), &ctx).await.expect("execute");

    assert!(result.success);
    assert!(result.content.contains("file1.txt"));
    assert!(result.content.contains("file2.txt"));
    assert!(result.content.contains("subdir"));
    let entries: Value = serde_json::from_str(&result.content).expect("list_dir json");
    assert!(entries.as_array().expect("entries").iter().any(|entry| {
        entry.get("name").and_then(Value::as_str) == Some("subdir")
            && entry.get("is_dir").and_then(Value::as_bool) == Some(true)
    }));
}

#[tokio::test]
async fn test_list_dir_with_path() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // Create a subdirectory with files
    let subdir = tmp.path().join("mydir");
    fs::create_dir(&subdir).expect("mkdir");
    fs::write(subdir.join("nested.txt"), "").expect("write");

    let tool = ListDirTool;
    let result = tool
        .execute(json!({"path": "mydir"}), &ctx)
        .await
        .expect("execute");

    assert!(result.success);
    assert!(result.content.contains("nested.txt"));
}

#[tokio::test]
async fn test_list_dir_small_dir_keeps_plain_array_response() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    fs::write(tmp.path().join("only.txt"), "").expect("write");

    let tool = ListDirTool;
    let result = tool.execute(json!({}), &ctx).await.expect("execute");

    let parsed: Value = serde_json::from_str(&result.content).expect("json");
    assert!(
        parsed.is_array(),
        "small dirs must keep the historical array shape: {parsed}"
    );
    assert_eq!(parsed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_list_dir_caps_entries_with_truncation_metadata() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let extra = 7;
    for i in 0..LIST_DIR_MAX_ENTRIES + extra {
        fs::write(tmp.path().join(format!("f{i:04}.txt")), "").expect("write");
    }

    let tool = ListDirTool;
    let result = tool.execute(json!({}), &ctx).await.expect("execute");

    let parsed: Value = serde_json::from_str(&result.content).expect("json");
    assert!(parsed.is_object(), "oversized dirs return an object");
    assert_eq!(parsed["truncated"], json!(true));
    assert_eq!(
        parsed["listed_entries"].as_u64().unwrap() as usize,
        LIST_DIR_MAX_ENTRIES
    );
    assert_eq!(
        parsed["total_entries"].as_u64().unwrap() as usize,
        LIST_DIR_MAX_ENTRIES + extra
    );
    assert_eq!(
        parsed["entries"].as_array().unwrap().len(),
        LIST_DIR_MAX_ENTRIES
    );
}

#[tokio::test]
async fn test_list_dir_respects_cancel_token() {
    let tmp = tempdir().expect("tempdir");
    fs::write(tmp.path().join("file.txt"), "").expect("write");
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();
    let ctx = ToolContext::new(tmp.path().to_path_buf()).with_cancel_token(cancel_token);

    let tool = ListDirTool;
    let err = tool
        .execute(json!({}), &ctx)
        .await
        .expect_err("cancelled list_dir should return an error");

    assert!(
        format!("{err:?}").contains("cancelled"),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_list_dir_blocking_wrapper_reports_timeout() {
    let err = run_blocking_list_dir(Duration::from_millis(1), None, || {
        std::thread::sleep(Duration::from_millis(50));
        Ok(Value::Array(Vec::new()))
    })
    .await
    .expect_err("slow list_dir worker should time out");

    assert!(
        matches!(err, ToolError::Timeout { seconds: 1 }),
        "unexpected error: {err:?}"
    );
}

#[test]
fn test_read_file_tool_properties() {
    let tool = ReadFileTool;
    assert_eq!(tool.name(), "read_file");
    assert!(tool.is_read_only());
    assert!(tool.is_sandboxable());
    assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
}

#[test]
fn test_write_file_tool_properties() {
    let tool = WriteFileTool;
    assert_eq!(tool.name(), "write_file");
    assert!(!tool.is_read_only());
    assert!(tool.is_sandboxable());
    assert_eq!(tool.approval_requirement(), ApprovalRequirement::Suggest);
}

#[test]
fn test_edit_file_tool_properties() {
    let tool = EditFileTool;
    assert_eq!(tool.name(), "edit_file");
    assert!(!tool.is_read_only());
    assert!(tool.is_sandboxable());
    assert_eq!(tool.approval_requirement(), ApprovalRequirement::Suggest);
    assert!(tool.description().contains("exact search/replace"));
    assert!(tool.description().contains("structural"));
}

#[test]
fn test_list_dir_tool_properties() {
    let tool = ListDirTool;
    assert_eq!(tool.name(), "list_dir");
    assert!(tool.is_read_only());
    assert!(tool.is_sandboxable());
    assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
}

#[test]
fn test_parallel_support_flags() {
    let read_tool = ReadFileTool;
    let list_tool = ListDirTool;
    let write_tool = WriteFileTool;

    assert!(read_tool.supports_parallel());
    assert!(list_tool.supports_parallel());
    assert!(!write_tool.supports_parallel());
}

#[test]
fn test_input_schemas() {
    // Verify all tools have valid JSON schemas
    let read_schema = ReadFileTool.input_schema();
    assert!(read_schema.get("type").is_some());
    assert!(read_schema.get("properties").is_some());

    let write_schema = WriteFileTool.input_schema();
    let required = write_schema
        .get("required")
        .and_then(|value| value.as_array())
        .expect("write schema should include required array");
    assert!(required.iter().any(|v| v.as_str() == Some("path")));
    assert!(required.iter().any(|v| v.as_str() == Some("content")));

    let edit_schema = EditFileTool.input_schema();
    let required = edit_schema
        .get("required")
        .and_then(|value| value.as_array())
        .expect("edit schema should include required array");
    let required_fields: Vec<_> = required.iter().filter_map(|value| value.as_str()).collect();
    assert_eq!(required_fields, vec!["path", "search", "replace"]);
    assert!(!required_fields.contains(&"fuzz"));
    // `fuzz` was never read by `edit` — it was parsed into a discarded
    // binding while the schema advertised it. An unimplemented parameter has
    // no place in a schema the model is asked to trust.
    assert!(edit_schema["properties"].get("fuzz").is_none());
    let search_desc = edit_schema["properties"]["search"]["description"]
        .as_str()
        .expect("search description");
    assert!(search_desc.contains("Exact text"));
    assert!(search_desc.contains("whitespace"));

    let list_schema = ListDirTool.input_schema();
    let required = list_schema
        .get("required")
        .and_then(|value| value.as_array())
        .expect("list schema should include required array");
    assert!(required.is_empty()); // path is optional
}

// === Content-hash edit guards (#3979) ===
//
// The guard's whole value is that a stale hash stops the write *before* it
// happens, so every rejection case asserts the file is byte-for-byte
// unchanged — a clear error over a corrupted file is the point.

/// Read the hash the model would actually see, the way the model sees it:
/// parsed out of the tool result's content, never out of its metadata.
async fn reported_content_hash(ctx: &ToolContext, path: &str) -> String {
    let result = ReadFileTool
        .execute(json!({ "path": path }), ctx)
        .await
        .expect("read");
    let (_, rest) = result
        .content
        .split_once("content_hash=\"")
        .unwrap_or_else(|| panic!("read output carries no content_hash: {}", result.content));
    let (hash, _) = rest.split_once('"').expect("terminated content_hash");
    hash.to_string()
}

#[tokio::test]
async fn reported_hash_verifies_against_the_file_contents() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let body = "alpha\nbeta\ngamma\n";
    fs::write(tmp.path().join("doc.txt"), body).expect("write");

    let reported = reported_content_hash(&ctx, "doc.txt").await;
    assert_eq!(reported, super::content_hash(body.as_bytes()));
    assert!(reported.starts_with("sha256:"), "{reported}");
    assert_eq!(reported.len(), "sha256:".len() + 64, "{reported}");
}

#[tokio::test]
async fn windowed_read_reports_the_whole_file_hash_not_the_window() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let body: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    fs::write(tmp.path().join("many.txt"), &body).expect("write");

    // A partial read must still hand back a guard for the *file*, or the
    // model could only ever guard edits to files it read in full.
    let result = ReadFileTool
        .execute(
            json!({ "path": "many.txt", "start_line": 5, "max_lines": 3 }),
            &ctx,
        )
        .await
        .expect("read");
    assert!(
        result.content.contains("shown_lines=\"5-7\""),
        "{}",
        result.content
    );
    assert!(
        result.content.contains(&format!(
            "content_hash=\"{}\"",
            super::content_hash(body.as_bytes())
        )),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn edit_with_matching_expected_hash_proceeds() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.txt");
    fs::write(&path, "alpha\nbeta\n").expect("write");

    let hash = reported_content_hash(&ctx, "doc.txt").await;
    EditFileTool
        .execute(
            json!({
                "path": "doc.txt",
                "search": "alpha",
                "replace": "delta",
                "expected_hash": hash,
            }),
            &ctx,
        )
        .await
        .expect("matching hash must not block the edit");

    assert_eq!(fs::read_to_string(&path).expect("read"), "delta\nbeta\n");
}

#[tokio::test]
async fn edit_with_stale_expected_hash_rejects_without_writing() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.txt");
    fs::write(&path, "alpha\nbeta\n").expect("write");

    let stale = reported_content_hash(&ctx, "doc.txt").await;
    // Someone else edits the file between the read and the edit.
    fs::write(&path, "alpha\nbeta\ngamma\n").expect("concurrent write");
    // Re-read so the *other* staleness gate (mtime/size) cannot be what
    // rejects this — the hash must be doing the work.
    read_before_edit(&ctx, "doc.txt").await;

    let err = EditFileTool
        .execute(
            json!({
                "path": "doc.txt",
                "search": "alpha",
                "replace": "delta",
                "expected_hash": stale,
            }),
            &ctx,
        )
        .await
        .expect_err("stale hash must reject");

    let message = err.to_string();
    assert!(message.contains("changed since it was read"), "{message}");
    assert!(
        message.contains("re-read") || message.contains("action=\"read\""),
        "{message}"
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "alpha\nbeta\ngamma\n",
        "a rejected edit must not modify the file"
    );
}

#[tokio::test]
async fn edit_without_expected_hash_is_unchanged() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.txt");
    fs::write(&path, "alpha\nbeta\n").expect("write");
    read_before_edit(&ctx, "doc.txt").await;

    EditFileTool
        .execute(
            json!({ "path": "doc.txt", "search": "alpha", "replace": "delta" }),
            &ctx,
        )
        .await
        .expect("absent expected_hash keeps the pre-#3979 behavior");

    assert_eq!(fs::read_to_string(&path).expect("read"), "delta\nbeta\n");
}

#[tokio::test]
async fn write_with_stale_expected_hash_rejects_without_writing() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.txt");
    fs::write(&path, "original\n").expect("write");

    let stale = super::content_hash(b"something else entirely\n");
    let err = WriteFileTool
        .execute(
            json!({ "path": "doc.txt", "content": "clobbered\n", "expected_hash": stale }),
            &ctx,
        )
        .await
        .expect_err("stale hash must reject");

    assert!(
        err.to_string().contains("changed since it was read"),
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "original\n",
        "a rejected write must not modify the file"
    );
}

#[tokio::test]
async fn write_with_matching_expected_hash_proceeds() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());
    let path = tmp.path().join("doc.txt");
    fs::write(&path, "original\n").expect("write");

    let hash = reported_content_hash(&ctx, "doc.txt").await;
    WriteFileTool
        .execute(
            json!({ "path": "doc.txt", "content": "replaced\n", "expected_hash": hash }),
            &ctx,
        )
        .await
        .expect("matching hash must not block the write");

    assert_eq!(fs::read_to_string(&path).expect("read"), "replaced\n");
}

#[tokio::test]
async fn write_with_expected_hash_on_a_missing_file_fails_closed() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    // There is no snapshot to verify, so honoring the guard is impossible.
    // Creating the file anyway would silently give back less safety than the
    // caller asked for.
    let err = WriteFileTool
        .execute(
            json!({
                "path": "new.txt",
                "content": "x\n",
                "expected_hash": super::content_hash(b"anything"),
            }),
            &ctx,
        )
        .await
        .expect_err("guarded write to a missing file must fail closed");

    assert!(err.to_string().contains("does not exist"), "{err}");
    assert!(
        !tmp.path().join("new.txt").exists(),
        "a rejected write must not create the file"
    );
}

#[tokio::test]
async fn write_without_expected_hash_still_creates_files() {
    let tmp = tempdir().expect("tempdir");
    let ctx = ToolContext::new(tmp.path().to_path_buf());

    WriteFileTool
        .execute(json!({ "path": "new.txt", "content": "x\n" }), &ctx)
        .await
        .expect("absent expected_hash keeps the pre-#3979 behavior");

    assert_eq!(
        fs::read_to_string(tmp.path().join("new.txt")).expect("read"),
        "x\n"
    );
}

#[tokio::test]
async fn expected_hash_is_advertised_on_every_mutating_action() {
    for schema in [
        WriteFileTool.input_schema(),
        EditFileTool.input_schema(),
        crate::tools::apply_patch::ApplyPatchTool.input_schema(),
    ] {
        let description = schema["properties"]["expected_hash"]["description"]
            .as_str()
            .expect("expected_hash must be advertised");
        assert!(description.contains("content_hash"), "{description}");
    }
}

/// S1: the in-process read tools are the *only* enforcement point for
/// `read_file`/`read`/`read_media` — they call `std::fs` inside the harness
/// process, so `sandbox-exec` and `bwrap` never see them. This asserts the
/// refusal is an explicit permission error, not an empty result, and that it
/// applies to the built-in defaults with no config required.
#[test]
fn read_tools_refuse_paths_under_the_default_sandbox_read_denylist() {
    let Some(home) = dirs::home_dir() else {
        // No home directory: only machine-wide rules exist and the assertion
        // below would be vacuous. Skip rather than pretend to have evidence.
        return;
    };

    let error = enforce_read_denylist(&home.join(".ssh").join("id_ed25519"), "read_file")
        .expect_err("~/.ssh must be denied by the built-in defaults");
    assert!(
        matches!(error, ToolError::PermissionDenied { .. }),
        "a denied read must be an explicit refusal, never an empty or missing-file result: {error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("read deny-list"), "{message}");
    assert!(
        message.contains("sandbox_read_denylist_exempt"),
        "{message}"
    );

    // Ordinary source files stay readable — a coding agent must still be able
    // to read the user's tree, which is the whole point of the tool.
    let temporary = tempfile::tempdir().expect("tempdir");
    let source = temporary.path().join("main.rs");
    std::fs::write(&source, "fn main() {}\n").expect("fixture");
    assert!(enforce_read_denylist(&source, "read_file").is_ok());
}

/// A symlink whose own name is innocuous but whose target is a credential
/// store must be refused by the target. A deny-list a symlink walks around is
/// theater, and `resolve_path` deliberately *permits* a workspace symlink that
/// resolves outside the workspace.
#[cfg(unix)]
#[tokio::test]
async fn read_file_refuses_a_workspace_symlink_pointing_at_a_denied_tree() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let ssh = home.join(".ssh");
    if !ssh.is_dir() {
        // Nothing to point at; a fabricated pass here would be worse than a skip.
        return;
    }

    let workspace = tempfile::tempdir().expect("tempdir");
    let link = workspace.path().join("notes.txt");
    std::os::unix::fs::symlink(&ssh, &link).expect("symlink");

    let error = enforce_read_denylist(&link, "read_file")
        .expect_err("a symlink into ~/.ssh must be refused by its target");
    let message = error.to_string();
    assert!(message.contains("symlink"), "{message}");
    // The rule's *label* ("SSH keys (~/.ssh)") is named on purpose — the user
    // needs to know which rule to exempt. What must never appear is the
    // resolved absolute path, which is the location the caller was fishing for.
    assert!(
        !message.contains(&ssh.display().to_string()),
        "the refusal must not hand back the secret's resolved location: {message}"
    );
}

/// F1: `list_dir ~/.ssh` used to hand back the key file names — enumerating a
/// denied directory is a read of it, exactly what Seatbelt's
/// `deny file-read*` blocks at the OS layer.
#[tokio::test]
async fn list_dir_refuses_to_enumerate_a_denied_directory() {
    let ctx = ToolContext::new(std::env::temp_dir());

    // Deterministic anchor independent of the machine's home layout: the
    // `.env` filename rule denies any path whose file name is `.env`, so a
    // directory by that name is a refused listing too.
    let holder = tempfile::tempdir().expect("tempdir");
    let env_dir = holder.path().join("project");
    std::fs::create_dir_all(env_dir.join(".env")).expect("mkdir");
    let error = ListDirTool
        .execute(json!({ "path": env_dir.join(".env") }), &ctx)
        .await
        .expect_err("a directory named `.env` is denied by the filename rule");
    assert!(
        matches!(error, ToolError::PermissionDenied { .. }),
        "enumeration of a denied path must be an explicit refusal: {error:?}"
    );

    let Some(home) = dirs::home_dir() else {
        return;
    };
    let ssh = home.join(".ssh");
    if !ssh.is_dir() {
        return;
    }
    let error = ListDirTool
        .execute(json!({ "path": ssh }), &ctx)
        .await
        .expect_err("`list_dir ~/.ssh` must not return the key file names");
    assert!(
        matches!(error, ToolError::PermissionDenied { .. }),
        "expected a permission refusal, got: {error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("read deny-list"), "{message}");
}

/// F2: the refusal must name the path as the caller spelled it. When a
/// workspace symlink points into a denied tree, `resolve_path` hands the guard
/// the secret's resolved absolute location first, and a denial raised on that
/// resolved path answers the probe ("where does this link really go?") in the
/// error text. The raw-spelling check runs before resolution, so it wins.
#[cfg(unix)]
#[tokio::test]
async fn read_file_refusal_names_the_callers_spelling_not_the_symlink_target() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let ssh = home.join(".ssh");
    if !ssh.is_dir() {
        return;
    }

    let workspace = tempfile::tempdir().expect("tempdir");
    let link = workspace.path().join("notes.txt");
    std::os::unix::fs::symlink(&ssh, &link).expect("symlink");

    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let error = ReadFileTool
        .execute(json!({ "path": link }), &ctx)
        .await
        .expect_err("a symlink into ~/.ssh must be refused");
    assert!(
        matches!(error, ToolError::PermissionDenied { .. }),
        "expected a permission refusal, got: {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("notes.txt"),
        "the refusal must name the caller's spelling: {message}"
    );
    assert!(
        !message.contains(&ssh.display().to_string()),
        "the refusal must not reveal the symlink target's location: {message}"
    );
}
