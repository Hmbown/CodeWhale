//! `image_analyze` tool — analyze images using a dedicated vision model.

use std::path::{Component, Path};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::bridge;
use crate::config::VisionModelConfig;
use crate::tools::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

pub struct ImageAnalyzeTool {
    config: VisionModelConfig,
}

impl ImageAnalyzeTool {
    #[must_use]
    pub fn new(config: VisionModelConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ToolSpec for ImageAnalyzeTool {
    fn name(&self) -> &str {
        "image_analyze"
    }

    fn description(&self) -> &str {
        "Analyze an image using the configured vision model. \
         Returns a structured description with object bounding boxes (0–1000 normalised coordinates). \
         Supports PNG, JPEG, GIF, WebP, and BMP formats."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "image_path": {
                    "type": "string",
                    "description": "Path to the image file to analyze"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt to guide the analysis."
                }
            },
            "required": ["image_path"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let image_path = required_str(&input, "image_path")?;
        let prompt = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("Describe this image in detail.");

        let image_path_buf = Path::new(image_path);
        let resolved_path = if image_path_buf.is_absolute() {
            image_path_buf.to_path_buf()
        } else {
            if image_path_buf
                .components()
                .any(|c| matches!(c, Component::ParentDir))
            {
                return Err(ToolError::execution_failed(
                    "image_path must be an absolute path or a relative path within the workspace.",
                ));
            }
            context.workspace.join(image_path_buf)
        };

        let mime = bridge::mime_type_for_path(&resolved_path).ok_or_else(|| {
            ToolError::execution_failed(format!(
                "Unsupported image format: {}",
                resolved_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("?")
            ))
        })?;
        let bytes = tokio::fs::read(&resolved_path)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to read image: {e}")))?;
        let data_url = bridge::build_data_url(mime, &bytes);

        let api_key = self.config.api_key.clone().unwrap_or_default();
        let base_url = self
            .config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let params = bridge::VisionAnalysisParams {
            api_key: &api_key,
            base_url: &base_url,
            model: &self.config.model,
            max_tokens: 8192,
            temperature: 0.0,
            timeout_secs: 120,
            image_data_url: &data_url,
            user_question: Some(prompt),
            primitives: self.config.primitives.unwrap_or(true),
        };

        let analysis = bridge::run_vision_analysis(params)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Vision analysis failed: {e}")))?;

        let context_text = bridge::format_vision_context(&analysis);

        ToolResult::json(&json!({
            "analysis": context_text,
            "model": self.config.model,
            "primitives_count": analysis.primitives.len(),
        }))
        .map_err(|e| ToolError::execution_failed(format!("Failed to serialize result: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fake_config() -> VisionModelConfig {
        VisionModelConfig {
            model: "test-vision-model".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: Some("https://example.invalid/v1".to_string()),
            primitives: None,
        }
    }

    #[test]
    fn tool_metadata_is_read_only_and_named_image_analyze() {
        let tool = ImageAnalyzeTool::new(fake_config());
        assert_eq!(tool.name(), "image_analyze");
        assert!(tool.capabilities().contains(&ToolCapability::ReadOnly));
    }

    #[tokio::test]
    async fn execute_accepts_absolute_path() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ImageAnalyzeTool::new(fake_config());
        // Absolute path is accepted (it will fail at file-read, not at validation)
        let abs = tmp.path().join("test.png");
        std::fs::write(&abs, b"\x89PNG\r\n").unwrap();
        let result = tool
            .execute(json!({"image_path": abs.to_str().unwrap()}), &ctx)
            .await;
        // Should not error on path validation — may fail on API call, but that's fine
        assert!(
            result.is_ok()
                || !result
                    .as_ref()
                    .unwrap_err()
                    .to_string()
                    .contains("relative path"),
            "absolute path must not be rejected by path validation; got {:?}",
            result.as_ref().map_err(|e| e.to_string())
        );
    }

    #[tokio::test]
    async fn execute_rejects_parent_dir_traversal() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ImageAnalyzeTool::new(fake_config());
        let err = tool
            .execute(json!({"image_path": "../escape.png"}), &ctx)
            .await
            .expect_err("`..`-traversal must reject");
        assert!(
            err.to_string()
                .contains("relative path within the workspace"),
            "error must call out the workspace boundary; got {err}"
        );
    }
}
