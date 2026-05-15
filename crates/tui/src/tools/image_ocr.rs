//! `image_ocr` tool — extract text from an image via the local
//! `tesseract` OCR engine.
//!
//! Tesseract is the open-source workhorse for "convert this image
//! to text" — covers screenshots, scanned PDFs that arrived as
//! image-only blobs, handwriting-free documents in 100+ languages,
//! receipts, whiteboard photos, etc. Surfacing it as a
//! model-callable tool means the model can OCR an asset the user
//! drops into the workspace without bouncing through `exec_shell`.
//!
//! Registration is gated by [`crate::dependencies::resolve_tesseract`]
//! (see [`crate::tools::registry::ToolRegistryBuilder::with_image_ocr_tools`]).
//! When tesseract isn't installed the tool simply doesn't appear in
//! the catalog, so the model never sees a binary it can't actually
//! use.

use std::process::{Command, Stdio};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str};

/// Tool implementing `image_ocr`. Spawns `tesseract <image> -` and
/// returns the extracted text on success.
pub struct ImageOcrTool;

#[async_trait]
impl ToolSpec for ImageOcrTool {
    fn name(&self) -> &'static str {
        "image_ocr"
    }

    fn description(&self) -> &'static str {
        "Extract text from an image (PNG, JPEG, or TIFF) via local tesseract OCR. Use this for screenshots, scanned receipts/whiteboards, image-only PDFs, or any visual that contains text the model needs to read. Returns the extracted text inline; no file is written. Use `exec_shell` only when you need a non-default OCR language pack or PSM mode."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the image file (relative to workspace or absolute). PNG / JPEG / TIFF supported."
                }
            },
            "required": ["path"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path_str = required_str(&input, "path")?;
        let image_path = context.resolve_path(path_str)?;
        if !image_path.exists() {
            return Err(ToolError::execution_failed(format!(
                "image_ocr: source path does not exist: {}",
                image_path.display()
            )));
        }

        // Late-resolve tesseract too. Registration gated on
        // resolve_tesseract(), but a concurrent uninstall between
        // catalog build and the model's call should surface a clear
        // error rather than the raw spawn failure.
        let tesseract = crate::dependencies::resolve_tesseract().ok_or_else(|| {
            ToolError::execution_failed(
                "image_ocr: tesseract binary not found on PATH. \
                 Install tesseract (macOS: `brew install tesseract`; \
                 Debian/Ubuntu: `apt install tesseract-ocr`; \
                 Windows: `winget install UB-Mannheim.TesseractOCR`) \
                 and restart deepseek-tui.",
            )
        })?;

        // `tesseract <image> -` writes the recognised text to stdout.
        // The trailing `-` is documented and produces text mode by
        // default (no `.txt` file written to disk).
        let mut cmd = Command::new(&tesseract);
        cmd.arg(&image_path);
        cmd.arg("-");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .map_err(|e| ToolError::execution_failed(format!("failed to launch tesseract: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ToolError::execution_failed(format!(
                "tesseract failed (exit {:?}): {stderr}",
                output.status.code()
            )));
        }

        // Tesseract appends a trailing form-feed on some platforms;
        // trim trailing whitespace so the result reads cleanly inline.
        let text = String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string();
        Ok(ToolResult::success(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Tesseract availability — happy-path tests skip when missing so
    /// CI environments without OCR still pass the suite.
    fn tesseract_present() -> bool {
        crate::dependencies::resolve_tesseract().is_some()
    }

    /// Resolve the checked-in OCR fixture path. The image lives at
    /// `crates/tui/tests/fixtures/ocr_hello.png` (300x100 grayscale,
    /// "HELLO OCR" rendered in Helvetica) and is committed for the
    /// happy-path round-trip below.
    fn ocr_fixture_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr_hello.png")
    }

    #[test]
    fn tool_metadata_marks_image_ocr_read_only_and_parallel() {
        let tool = ImageOcrTool;
        assert_eq!(tool.name(), "image_ocr");
        assert!(tool.supports_parallel());
        let caps = tool.capabilities();
        assert!(caps.contains(&ToolCapability::ReadOnly));
        assert!(!caps.contains(&ToolCapability::WritesFiles));
    }

    #[tokio::test]
    async fn image_ocr_rejects_missing_path() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let err = ImageOcrTool
            .execute(json!({"path": "definitely-not-here.png"}), &ctx)
            .await
            .expect_err("nonexistent path must reject before tesseract spawn");
        let msg = err.to_string();
        assert!(
            msg.contains("does not exist"),
            "error must call out missing path; got {msg}"
        );
    }

    #[tokio::test]
    async fn image_ocr_recovers_hello_from_fixture_image() {
        if !tesseract_present() {
            // Tool wouldn't be registered without tesseract — mirror
            // that here so the suite stays green on CI images that
            // intentionally omit OCR tooling.
            return;
        }
        let fixture = ocr_fixture_path();
        if !fixture.exists() {
            // Fixture not committed (sparse / shallow checkout). Skip
            // silently rather than failing the suite.
            return;
        }
        let tmp = tempdir().expect("tempdir");
        // Stage the fixture under the workspace so the path resolver
        // accepts the relative input — keeps the test independent of
        // the workspace boundary check inside `resolve_path`.
        let staged = tmp.path().join("ocr_hello.png");
        fs::copy(&fixture, &staged).unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let result = ImageOcrTool
            .execute(json!({"path": "ocr_hello.png"}), &ctx)
            .await
            .expect("execute");
        assert!(result.success);
        // Tesseract reliably recovers "HELLO OCR" from the rendered
        // PNG; allow either spacing variant.
        let normalised = result.content.to_uppercase();
        assert!(
            normalised.contains("HELLO") && normalised.contains("OCR"),
            "expected OCR to recover HELLO OCR; got {:?}",
            result.content
        );
    }
}
