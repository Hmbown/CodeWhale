//! `pandoc_convert` tool — universal document conversion via the
//! `pandoc` binary (<https://pandoc.org>).
//!
//! Pandoc is the de-facto Swiss Army knife for moving prose between
//! the formats writers and engineers actually use: Markdown to HTML,
//! HTML to Markdown, anything to LaTeX or DOCX, RST to Markdown,
//! ReST imports, etc. Surfacing it as a model-callable tool unblocks
//! a large class of "rewrite this report as ..." / "publish this
//! changelog as ..." workflows that previously required the user
//! to drop into a terminal between turns.
//!
//! Registration is gated by [`crate::dependencies::resolve_pandoc`]
//! (see [`crate::tools::registry::ToolRegistryBuilder::with_pandoc_tools`]).
//! When pandoc isn't installed the tool simply doesn't appear in the
//! catalog, so the model never sees a binary it can't actually use.
//!
//! ## Format whitelist
//!
//! Pandoc supports ~30 input and ~50 output formats, and exposing
//! every one of them as a free-text string would let the model
//! ask for `pdf` (which needs LaTeX installed), `epub3` (works
//! everywhere but ambiguous vs. `epub`), or typos like `markown`.
//! The whitelist below is the curated subset that a) covers ~95%
//! of real document-handling needs and b) doesn't require additional
//! system dependencies (LaTeX engines, ImageMagick) beyond pandoc
//! itself.
//!
//! Adding a format: append to [`SUPPORTED_TARGET_FORMATS`] and the
//! schema description; the dispatch logic is whitelist-driven so
//! anything in the list goes through unchanged.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, required_str,
};

/// Curated whitelist of pandoc target formats. Each entry corresponds
/// to a `--to=<format>` value pandoc accepts natively without
/// additional system tooling. Keep this list short and intentional —
/// the schema description below references it verbatim.
pub(crate) const SUPPORTED_TARGET_FORMATS: &[&str] = &[
    "markdown",   // Pandoc-flavored Markdown (the safe round-trip default)
    "gfm",        // GitHub-Flavored Markdown
    "commonmark", // strict CommonMark
    "html",       // HTML5
    "rst",        // reStructuredText
    "latex",      // LaTeX source (does not require a TeX install to *generate*)
    "docx",       // Microsoft Word .docx
    "odt",        // OpenDocument Text
    "epub",       // EPUB 2/3
    "plain",      // plain text (formatting stripped)
    "asciidoc",   // AsciiDoc
];

/// Tool implementing `pandoc_convert`. Converts a source file into
/// a target format and either writes the output to disk or returns
/// the converted text inline.
pub struct PandocConvertTool;

#[async_trait]
impl ToolSpec for PandocConvertTool {
    fn name(&self) -> &'static str {
        "pandoc_convert"
    }

    fn description(&self) -> &'static str {
        "Convert a document between formats via pandoc. Reads `source_path` (any pandoc-supported input format — pandoc autodetects from extension), converts to `target_format`, and either writes the result to `output_path` (when provided) or returns the converted text inline. Supported targets: markdown, gfm, commonmark, html, rst, latex, docx, odt, epub, plain, asciidoc. Use this instead of shelling out to pandoc via `exec_shell` — no approval prompt for output_path-less reads, structured errors, and a curated format whitelist."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source_path": {
                    "type": "string",
                    "description": "Path to the source document (relative to workspace or absolute). Pandoc autodetects the input format from the file extension."
                },
                "target_format": {
                    "type": "string",
                    "description": "One of: markdown, gfm, commonmark, html, rst, latex, docx, odt, epub, plain, asciidoc.",
                    "enum": SUPPORTED_TARGET_FORMATS,
                },
                "output_path": {
                    "type": "string",
                    "description": "Optional path to write the converted document to. When omitted, the converted text is returned inline (text formats only — binary formats like docx/odt/epub require output_path)."
                }
            },
            "required": ["source_path", "target_format"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Sandboxable,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let source_path_str = required_str(&input, "source_path")?;
        let target_format = required_str(&input, "target_format")?.trim().to_lowercase();
        let output_path_str = optional_str(&input, "output_path");

        if !SUPPORTED_TARGET_FORMATS.contains(&target_format.as_str()) {
            return Err(ToolError::invalid_input(format!(
                "unsupported target_format `{target_format}`. Pick one of: {}",
                SUPPORTED_TARGET_FORMATS.join(", ")
            )));
        }

        let source_path = context.resolve_path(source_path_str)?;
        if !source_path.exists() {
            return Err(ToolError::execution_failed(format!(
                "source_path does not exist: {}",
                source_path.display()
            )));
        }

        let resolved_output_path: Option<PathBuf> = match output_path_str {
            Some(p) => Some(context.resolve_path(p)?),
            None => None,
        };

        // Binary formats can't round-trip through stdout reliably —
        // require an output_path so the bytes survive the trip.
        if resolved_output_path.is_none() && format_is_binary(&target_format) {
            return Err(ToolError::invalid_input(format!(
                "target_format `{target_format}` is binary; provide an `output_path` to write the converted file."
            )));
        }

        // Resolve the pandoc binary at execution time too — registration
        // gated on resolve_pandoc(), but a concurrent uninstall between
        // catalog build and the model's call should produce a clear
        // error rather than the cryptic "program not found" from raw
        // Command::spawn.
        let pandoc = crate::dependencies::resolve_pandoc().ok_or_else(|| {
            ToolError::execution_failed(
                "pandoc_convert: pandoc binary not found on PATH. \
                 Install pandoc (macOS: `brew install pandoc`; \
                 Debian/Ubuntu: `apt install pandoc`; \
                 Windows: `winget install JohnMacFarlane.Pandoc`) and restart deepseek-tui.",
            )
        })?;

        let mut cmd = Command::new(&pandoc);
        cmd.arg(&source_path);
        cmd.arg("--to").arg(&target_format);
        if let Some(out) = resolved_output_path.as_ref() {
            cmd.arg("--output").arg(out);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .map_err(|e| ToolError::execution_failed(format!("failed to launch pandoc: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ToolError::execution_failed(format!(
                "pandoc failed (exit {:?}): {stderr}",
                output.status.code()
            )));
        }

        let summary = if let Some(out) = resolved_output_path {
            format!(
                "Converted {} → {} via pandoc; wrote {}",
                source_path.display(),
                target_format,
                out.display()
            )
        } else {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            return Ok(ToolResult::success(text));
        };
        Ok(ToolResult::success(summary))
    }
}

/// Whitelist of target formats whose output is binary (and therefore
/// can't be returned as inline text). `docx`, `odt`, and `epub` are
/// ZIP archives; everything else in [`SUPPORTED_TARGET_FORMATS`]
/// renders to UTF-8 text.
pub(crate) fn format_is_binary(target_format: &str) -> bool {
    matches!(target_format, "docx" | "odt" | "epub")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn pandoc_present() -> bool {
        crate::dependencies::resolve_pandoc().is_some()
    }

    #[test]
    fn supported_target_formats_match_schema_enum() {
        let tool = PandocConvertTool;
        let schema = tool.input_schema();
        let enum_vals = schema
            .get("properties")
            .and_then(|p| p.get("target_format"))
            .and_then(|t| t.get("enum"))
            .and_then(|e| e.as_array())
            .expect("target_format enum must be present in schema");
        let from_schema: Vec<&str> = enum_vals.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            from_schema, SUPPORTED_TARGET_FORMATS,
            "schema enum must mirror the SUPPORTED_TARGET_FORMATS constant exactly",
        );
    }

    #[test]
    fn binary_formats_require_output_path() {
        for fmt in ["docx", "odt", "epub"] {
            assert!(format_is_binary(fmt));
        }
        for fmt in [
            "markdown",
            "html",
            "rst",
            "latex",
            "plain",
            "gfm",
            "commonmark",
        ] {
            assert!(!format_is_binary(fmt));
        }
    }

    #[tokio::test]
    async fn pandoc_convert_rejects_unsupported_target_format() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("in.md");
        fs::write(&src, "# hi").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let err = PandocConvertTool
            .execute(
                json!({"source_path": "in.md", "target_format": "definitely-not-real"}),
                &ctx,
            )
            .await
            .expect_err("unsupported target format must reject before pandoc spawn");
        assert!(
            err.to_string().contains("unsupported target_format"),
            "error must call out the unsupported format; got {err}"
        );
    }

    #[tokio::test]
    async fn pandoc_convert_rejects_inline_request_for_binary_format() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("in.md");
        fs::write(&src, "# hi").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let err = PandocConvertTool
            .execute(
                json!({"source_path": "in.md", "target_format": "docx"}),
                &ctx,
            )
            .await
            .expect_err("missing output_path for docx must reject");
        assert!(
            err.to_string().contains("binary") && err.to_string().contains("output_path"),
            "error must explain why output_path is required; got {err}"
        );
    }

    #[tokio::test]
    async fn pandoc_convert_roundtrips_markdown_to_html_inline() {
        if !pandoc_present() {
            // Tool wouldn't be registered without pandoc; mirror the
            // catalog-build behaviour.
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("note.md");
        fs::write(&src, "# Title\n\nA paragraph with `inline code`.\n").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let result = PandocConvertTool
            .execute(
                json!({"source_path": "note.md", "target_format": "html"}),
                &ctx,
            )
            .await
            .expect("execute");
        assert!(result.success);
        assert!(
            result.content.contains("<h1") && result.content.contains("Title"),
            "html output must contain the heading; got {}",
            result.content
        );
        assert!(
            result.content.contains("<code") || result.content.contains("inline code"),
            "html output must preserve inline code; got {}",
            result.content
        );
    }

    #[tokio::test]
    async fn pandoc_convert_writes_output_path_and_reports_summary() {
        if !pandoc_present() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("note.md");
        fs::write(&src, "# Title\n").unwrap();
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let result = PandocConvertTool
            .execute(
                json!({
                    "source_path": "note.md",
                    "target_format": "html",
                    "output_path": "out.html",
                }),
                &ctx,
            )
            .await
            .expect("execute");
        assert!(result.success);
        assert!(result.content.contains("wrote"));
        let written = fs::read_to_string(tmp.path().join("out.html")).expect("read");
        assert!(
            written.contains("Title"),
            "written file must contain converted body; got {written}"
        );
    }

    #[tokio::test]
    async fn pandoc_convert_surfaces_missing_source_path_clearly() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let err = PandocConvertTool
            .execute(
                json!({"source_path": "missing.md", "target_format": "html"}),
                &ctx,
            )
            .await
            .expect_err("nonexistent source must reject");
        assert!(
            err.to_string().contains("source_path") && err.to_string().contains("does not exist"),
            "error must call out missing source; got {err}"
        );
    }
}
