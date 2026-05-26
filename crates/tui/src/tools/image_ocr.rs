//! `image_ocr` tool — extract text from an image via local OCR.
//!
//! Tesseract is the cross-platform workhorse for "convert this image
//! to text". On macOS we also use the built-in Vision framework, so
//! screenshots keep working on a clean machine without making the
//! user install a separate OCR binary first.
//!
//! Surfacing OCR as a model-callable tool means the model can read an
//! asset the user drops into the workspace without bouncing through
//! `exec_shell`.

use std::path::Path;
use std::process::{Command, Stdio};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str};

/// Tool implementing `image_ocr`. Runs a local OCR backend and returns the
/// extracted text on success.
pub struct ImageOcrTool;

#[async_trait]
impl ToolSpec for ImageOcrTool {
    fn name(&self) -> &'static str {
        "image_ocr"
    }

    fn description(&self) -> &'static str {
        "Extract text from an image (PNG, JPEG, or TIFF) via local OCR. On macOS this uses the built-in Vision framework; otherwise it uses local tesseract when available. Use this for screenshots, scanned receipts/whiteboards, image-only PDFs, or any visual that contains text the model needs to read. Returns the extracted text inline; no file is written."
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

        let text = ocr_image_path(&image_path)?;
        Ok(ToolResult::success(text))
    }
}

pub(crate) fn ocr_available() -> bool {
    crate::dependencies::resolve_tesseract().is_some() || native_ocr_available()
}

pub(crate) fn ocr_image_path(image_path: &Path) -> Result<String, ToolError> {
    if let Some(text) = try_native_ocr(image_path)? {
        return Ok(text);
    }

    if let Some(tesseract) = crate::dependencies::resolve_tesseract() {
        return ocr_with_tesseract(&tesseract, image_path);
    }

    Err(ToolError::execution_failed(
        "image_ocr: no local OCR backend is available. On macOS, update to a version with the Vision framework; on Linux/Windows install tesseract and restart codewhale.",
    ))
}

fn ocr_with_tesseract(tesseract: &str, image_path: &Path) -> Result<String, ToolError> {
    // `tesseract <image> -` writes the recognised text to stdout. The trailing
    // `-` is documented and produces text mode by default (no `.txt` file).
    let mut cmd = Command::new(tesseract);
    cmd.arg(image_path);
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

    // Tesseract appends a trailing form-feed on some platforms; trim trailing
    // whitespace so the result reads cleanly inline.
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

#[cfg(target_os = "macos")]
fn native_ocr_available() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn native_ocr_available() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn try_native_ocr(_image_path: &Path) -> Result<Option<String>, ToolError> {
    Ok(None)
}

#[cfg(target_os = "macos")]
#[link(name = "Vision", kind = "framework")]
unsafe extern "C" {}

#[cfg(target_os = "macos")]
fn try_native_ocr(image_path: &Path) -> Result<Option<String>, ToolError> {
    macos_vision::recognize_text(image_path).map(Some)
}

#[cfg(target_os = "macos")]
mod macos_vision {
    use super::*;
    use objc2::msg_send;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSArray, NSDictionary, NSError, NSString, NSURL};
    use std::ptr;

    pub(super) fn recognize_text(image_path: &Path) -> Result<String, ToolError> {
        autoreleasepool(|_| recognize_text_inner(image_path))
    }

    fn recognize_text_inner(image_path: &Path) -> Result<String, ToolError> {
        let url = NSURL::from_file_path(image_path).ok_or_else(|| {
            ToolError::execution_failed(format!(
                "image_ocr: failed to build file URL for {}",
                image_path.display()
            ))
        })?;

        let request_class = AnyClass::get(c"VNRecognizeTextRequest").ok_or_else(|| {
            ToolError::execution_failed("image_ocr: macOS Vision text request is unavailable")
        })?;
        let handler_class = AnyClass::get(c"VNImageRequestHandler").ok_or_else(|| {
            ToolError::execution_failed("image_ocr: macOS Vision image handler is unavailable")
        })?;

        let request = new_object(request_class, "VNRecognizeTextRequest")?;
        // VNRequestTextRecognitionLevelAccurate is 0. Use accurate mode for
        // screenshots and receipts; the tool is user-facing, not latency-critical.
        unsafe {
            let _: () = msg_send![&*request, setRecognitionLevel: 0usize];
            let _: () = msg_send![&*request, setUsesLanguageCorrection: true];
        }

        let requests = NSArray::from_slice(&[&*request]);
        let options: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();

        let handler_alloc = alloc_object(handler_class, "VNImageRequestHandler")?;
        let handler_raw: *mut AnyObject =
            unsafe { msg_send![handler_alloc, initWithURL: &*url, options: &*options] };
        let handler = unsafe { Retained::from_raw(handler_raw) }.ok_or_else(|| {
            ToolError::execution_failed("image_ocr: failed to initialize Vision image handler")
        })?;

        let mut error: *mut NSError = ptr::null_mut();
        let ok: bool =
            unsafe { msg_send![&*handler, performRequests: &*requests, error: &mut error] };
        if !ok {
            return Err(ToolError::execution_failed(format!(
                "image_ocr: macOS Vision failed{}",
                vision_error_suffix(error)
            )));
        }

        collect_recognized_text(&request)
    }

    fn new_object(class: &AnyClass, label: &str) -> Result<Retained<AnyObject>, ToolError> {
        let raw: *mut AnyObject = unsafe { msg_send![class, new] };
        unsafe { Retained::from_raw(raw) }.ok_or_else(|| {
            ToolError::execution_failed(format!("image_ocr: failed to create {label}"))
        })
    }

    fn alloc_object(class: &AnyClass, label: &str) -> Result<*mut AnyObject, ToolError> {
        let raw: *mut AnyObject = unsafe { msg_send![class, alloc] };
        if raw.is_null() {
            Err(ToolError::execution_failed(format!(
                "image_ocr: failed to allocate {label}"
            )))
        } else {
            Ok(raw)
        }
    }

    fn collect_recognized_text(request: &AnyObject) -> Result<String, ToolError> {
        let results: *mut AnyObject = unsafe { msg_send![request, results] };
        if results.is_null() {
            return Ok(String::new());
        }

        let count: usize = unsafe { msg_send![results, count] };
        let mut lines = Vec::new();
        for idx in 0..count {
            let observation: *mut AnyObject = unsafe { msg_send![results, objectAtIndex: idx] };
            if observation.is_null() {
                continue;
            }
            let candidates: *mut AnyObject =
                unsafe { msg_send![observation, topCandidates: 1usize] };
            if candidates.is_null() {
                continue;
            }
            let candidate_count: usize = unsafe { msg_send![candidates, count] };
            if candidate_count == 0 {
                continue;
            }
            let candidate: *mut AnyObject = unsafe { msg_send![candidates, objectAtIndex: 0usize] };
            if candidate.is_null() {
                continue;
            }
            let text: *mut NSString = unsafe { msg_send![candidate, string] };
            if text.is_null() {
                continue;
            }
            let line = unsafe { &*text }.to_string();
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }

        Ok(lines.join("\n"))
    }

    fn vision_error_suffix(error: *mut NSError) -> String {
        if error.is_null() {
            return String::new();
        }
        let description: *mut NSString = unsafe { msg_send![error, localizedDescription] };
        if description.is_null() {
            String::new()
        } else {
            format!(": {}", unsafe { &*description })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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
        if !ocr_available() {
            // Tool wouldn't be registered without a local OCR backend — mirror
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
