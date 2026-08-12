use super::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn missing_pdf_path_precedes_unavailable_helper() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let input = temporary.path().join("missing.pdf");
    let missing = temporary.path().join("definitely-not-pdftotext");
    let error = read_pdf_if_detected(
        &input,
        None,
        super::super::pdf::PdfTextCommand::test(missing.as_os_str(), Duration::from_secs(1), None),
    )
    .await
    .expect_err("missing path must fail before the missing helper is launched");

    match error {
        ToolError::ExecutionFailed { message } => {
            assert!(message.contains("Failed to read"), "{message}");
            assert!(message.contains("missing.pdf"), "{message}");
        }
        other => panic!("expected ordinary read failure, got {other:?}"),
    }
}

#[tokio::test]
async fn read_file_missing_pdftotext_is_a_failed_typed_outcome() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let missing = temporary.path().join("definitely-not-pdftotext");
    let input = temporary.path().join("input.pdf");
    std::fs::write(&input, b"%PDF-1.7\n%%EOF").expect("fixture");

    let error = read_pdf_with_command(
        &input,
        None,
        super::super::pdf::PdfTextCommand::test(missing.as_os_str(), Duration::from_secs(1), None),
    )
    .await
    .expect_err("missing helper must fail the tool call");
    let payload = match &error {
        ToolError::NotAvailable { message } => {
            serde_json::from_str::<Value>(message).expect("structured unavailable payload")
        }
        other => panic!("unexpected error: {other:?}"),
    };
    assert_eq!(payload["type"], "binary_unavailable");
    assert_eq!(
        crate::tools::spec::ToolExecutionOutcome::from_legacy(Err(error)).status,
        crate::tools::spec::ToolTerminalStatus::Failed
    );
}

#[test]
fn contract_read_exact_line_limit_with_terminal_newline_is_not_truncated() {
    let content = (0..READ_MAX_LINES)
        .map(|index| format!("line-{index}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let window = contract_read_window(&content);
    assert!(!window.truncated_by_lines);
    assert!(!window.truncated_by_bytes);
    assert_eq!(window.shown_lines, READ_MAX_LINES);
    assert_eq!(window.content, content);
}

#[test]
fn contract_read_byte_limit_keeps_only_complete_utf8_lines() {
    let first = "é".repeat(20_000);
    let second = "z".repeat(20_000);
    let window = contract_read_window(&format!("{first}\n{second}\n"));
    assert!(window.truncated_by_bytes);
    assert_eq!(window.shown_lines, 1);
    assert_eq!(window.content, first);
    assert!(std::str::from_utf8(window.content.as_bytes()).is_ok());
}

#[tokio::test]
async fn contract_read_reports_huge_first_line_with_exact_bash_fallback() {
    let temporary = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        temporary.path().join("huge.txt"),
        "x".repeat(READ_MAX_BYTES + 1),
    )
    .expect("fixture");
    let context = ToolContext::new(temporary.path());
    let result = ReadFileTool::execute_contract_read(json!({"path": "huge.txt"}), &context)
        .await
        .expect("read result");
    assert_eq!(
        result.content,
        "[Line 1 is 50.0KB, exceeds 50.0KB limit. Use bash: sed -n '1p' huge.txt | head -c 51200]"
    );
}

#[tokio::test]
async fn pi_read_offset_oob_and_limit_continuation_match_contract() {
    let temporary = tempfile::tempdir().expect("tempdir");
    std::fs::write(temporary.path().join("lines.txt"), "one\ntwo\nthree").expect("fixture");
    let context = ToolContext::new(temporary.path());

    let limited = ReadFileTool::execute_contract_read(
        json!({"path": "lines.txt", "offset": 2, "limit": 1}),
        &context,
    )
    .await
    .expect("limited read");
    assert_eq!(
        limited.content,
        "two\n\n[1 more lines in file. Use offset=3 to continue.]"
    );

    let error =
        ReadFileTool::execute_contract_read(json!({"path": "lines.txt", "offset": 4}), &context)
            .await
            .expect_err("offset beyond EOF");
    assert_eq!(
        error.to_string(),
        "Failed to execute tool: Offset 4 is beyond end of file (3 lines total)"
    );
}

#[tokio::test]
async fn pi_read_uses_magic_not_extension_for_images() {
    let temporary = tempfile::tempdir().expect("tempdir");
    std::fs::write(temporary.path().join("plain.png"), "ordinary text").expect("text fixture");
    std::fs::write(
        temporary.path().join("renamed.data"),
        [b"\x89PNG\r\n\x1a\n".as_slice(), b"\0\0\0\rIHDR".as_slice()].concat(),
    )
    .expect("image fixture");
    let context = ToolContext::new(temporary.path());

    let text = ReadFileTool::execute_contract_read(json!({"path": "plain.png"}), &context)
        .await
        .expect("fake extension remains text");
    assert_eq!(text.content, "ordinary text");
    let image = ReadFileTool::execute_contract_read(json!({"path": "renamed.data"}), &context)
        .await
        .expect("real image uses typed transport");
    assert_eq!(image.content_blocks.len(), 1);
    assert!(matches!(
        &image.content_blocks[0],
        codewhale_tools::ToolResultContentBlock::Image { mime_type, .. }
            if mime_type == "image/png"
    ));
}

#[test]
fn pi_edit_preparation_accepts_string_and_legacy_recovery_forms() {
    let encoded = prepare_pi_edit_input(json!({
        "path": "doc.txt",
        "edits": "[{\"oldText\":\"a\",\"newText\":\"b\"}]"
    }))
    .expect("encoded edits");
    assert_eq!(encoded["edits"][0], json!({"oldText": "a", "newText": "b"}));

    let recovered = prepare_pi_edit_input(json!({
        "path": "doc.txt",
        "edits": {"malformed": true},
        "oldText": "a",
        "newText": "b"
    }))
    .expect("legacy recovery");
    assert_eq!(
        recovered["edits"],
        json!([{"oldText": "a", "newText": "b"}])
    );
    assert!(recovered.get("oldText").is_none());
    assert!(recovered.get("newText").is_none());
}

#[test]
fn pi_edit_fuzzy_normalization_preserves_untouched_lines() {
    let original = "untouched line  \nShe said “hello”—today.   \ntail  \n";
    let updated = apply_pi_edits(
        original,
        &[ContractEdit {
            index: 0,
            old_text: "She said \"hello\"-today.".to_string(),
            new_text: "She said hello.".to_string(),
        }],
        "doc.txt",
    )
    .expect("fuzzy edit");
    assert_eq!(updated, "untouched line  \nShe said hello.\ntail  \n");
}

#[tokio::test]
async fn pi_edit_preserves_bom_and_crlf_without_prior_read() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let path = temporary.path().join("doc.txt");
    std::fs::write(&path, "\u{FEFF}alpha\r\nbeta\r\n").expect("fixture");
    let context = ToolContext::new(temporary.path());
    let result = EditFileTool::execute_pi_edits(
        json!({
            "path": "doc.txt",
            "edits": [{"oldText": "alpha\nbeta", "newText": "one\ntwo"}]
        }),
        &context,
    )
    .await
    .expect("edit");
    assert_eq!(
        result.content,
        "Successfully replaced 1 block(s) in doc.txt."
    );
    assert_eq!(
        std::fs::read(&path).expect("updated"),
        "\u{FEFF}one\r\ntwo\r\n".as_bytes()
    );
}

#[tokio::test]
async fn queued_parallel_pi_edits_preserve_both_changes() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let path = temporary.path().join("doc.txt");
    std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("fixture");
    let context = ToolContext::new(temporary.path());
    let first_context = context.clone();
    let second_context = context.clone();

    let first = tokio::spawn(async move {
        EditFileTool::execute_pi_edits(
            json!({"path": "doc.txt", "edits": [{"oldText": "alpha", "newText": "A"}]}),
            &first_context,
        )
        .await
    });
    let second = tokio::spawn(async move {
        EditFileTool::execute_pi_edits(
            json!({"path": "doc.txt", "edits": [{"oldText": "gamma", "newText": "G"}]}),
            &second_context,
        )
        .await
    });
    first.await.expect("first task").expect("first edit");
    second.await.expect("second task").expect("second edit");
    assert_eq!(
        std::fs::read_to_string(path).expect("updated"),
        "A\nbeta\nG\n"
    );
}

#[tokio::test]
async fn cancelled_queued_pi_write_never_starts() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let context = ToolContext::new(temporary.path());
    let path = context.resolve_path("queued.txt").expect("resolved path");
    let held = file_mutation_lock(&path).expect("queue").lock_owned().await;
    let cancellation = CancellationToken::new();
    let queued_context = context.clone().with_cancel_token(cancellation.clone());
    let queued = tokio::spawn(async move {
        WriteFileTool::execute_contract_write(
            json!({"path": "queued.txt", "content": "must-not-land"}),
            &queued_context,
        )
        .await
    });
    tokio::task::yield_now().await;
    cancellation.cancel();
    let error = queued
        .await
        .expect("queued task")
        .expect_err("queued write must cancel");
    assert!(matches!(error, ToolError::Cancelled { .. }));
    assert!(!path.exists());
    drop(held);
}

#[cfg(unix)]
#[tokio::test]
async fn pi_edit_rejects_read_only_target_before_atomic_replace() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("tempdir");
    let path = temporary.path().join("readonly.txt");
    std::fs::write(&path, "alpha\n").expect("fixture");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).expect("readonly");
    let context = ToolContext::new(temporary.path());
    let result = EditFileTool::execute_pi_edits(
        json!({"path": "readonly.txt", "edits": [{"oldText": "alpha", "newText": "beta"}]}),
        &context,
    )
    .await;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("restore permissions");
    let error = result.expect_err("read-only target must fail");
    assert!(error.to_string().contains("readable and writable"));
    assert_eq!(std::fs::read_to_string(path).expect("unchanged"), "alpha\n");
}
