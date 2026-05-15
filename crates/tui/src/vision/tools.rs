//! `image_analyze` tool — analyze images using a dedicated vision model.

use std::path::{Component, Path};
use std::time::Duration;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

use crate::config::VisionModelConfig;
use crate::llm_client::{LlmError, RetryConfig, with_retry};
use crate::tools::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

pub struct ImageAnalyzeTool {
    config: VisionModelConfig,
    client: reqwest::Client,
}

impl ImageAnalyzeTool {
    #[must_use]
    pub fn new(config: VisionModelConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build HTTP client");
        Self { config, client }
    }

    async fn read_image_file(path: &Path) -> Result<(String, String), ToolError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to read image file: {e}")))?;

        let mime_type = Self::detect_mime_type(path)?;
        let base64_data = BASE64.encode(&bytes);
        Ok((base64_data, mime_type))
    }

    fn detect_mime_type(path: &Path) -> Result<String, ToolError> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match extension.as_str() {
            "png" => Ok("image/png".to_string()),
            "jpg" | "jpeg" => Ok("image/jpeg".to_string()),
            "gif" => Ok("image/gif".to_string()),
            "webp" => Ok("image/webp".to_string()),
            "bmp" => Ok("image/bmp".to_string()),
            _ => Err(ToolError::execution_failed(format!(
                "Unsupported image format: {extension}"
            ))),
        }
    }

    fn base_url(&self) -> String {
        self.config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
    }

    fn api_key(&self) -> String {
        self.config.api_key.clone().unwrap_or_default()
    }
}

#[async_trait]
impl ToolSpec for ImageAnalyzeTool {
    fn name(&self) -> &str {
        "image_analyze"
    }

    fn description(&self) -> &str {
        "Analyze an image using the configured vision model. \
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
        if image_path_buf.components().any(|c| {
            matches!(
                c,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir
            )
        }) {
            return Err(ToolError::execution_failed(
                "image_path must be a relative path within the workspace and cannot escape it.",
            ));
        }
        let resolved_path = context.workspace.join(image_path_buf);
        let (image_data, mime_type) = Self::read_image_file(&resolved_path).await?;

        let payload = json!({
            "model": self.config.model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": prompt},
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", mime_type, image_data)
                            }
                        }
                    ]
                }
            ],
            "max_tokens": 4096,
            "temperature": 0.7
        });

        let url = format!("{}/chat/completions", self.base_url());
        let api_key = self.api_key();

        let retry_config = RetryConfig {
            max_retries: 3,
            initial_delay: 1.0,
            max_delay: 30.0,
            enabled: true,
            ..Default::default()
        };

        let response = with_retry(
            &retry_config,
            || {
                let client = self.client.clone();
                let url = url.clone();
                let api_key = api_key.clone();
                let payload = payload.clone();
                async move {
                    let response = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Authorization", format!("Bearer {}", api_key))
                        .json(&payload)
                        .send()
                        .await
                        .map_err(|e| LlmError::from_reqwest(&e))?;

                    let status = response.status();
                    if !status.is_success() {
                        let error_text = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        return Err(LlmError::from_http_response(status.as_u16(), &error_text));
                    }
                    Ok(response)
                }
            },
            None,
        )
        .await
        .map_err(|e| ToolError::execution_failed(format!("Vision API request failed: {e}")))?;

        let json: Value = response
            .json()
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to parse response: {e}")))?;

        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let model = json
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or(&self.config.model)
            .to_string();

        let result = json!({
            "analysis": content,
            "model": model,
        });

        ToolResult::json(&result)
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
        }
    }

    #[test]
    fn tool_metadata_is_read_only_and_named_image_analyze() {
        let tool = ImageAnalyzeTool::new(fake_config());
        assert_eq!(tool.name(), "image_analyze");
        assert!(tool.capabilities().contains(&ToolCapability::ReadOnly));
    }

    #[test]
    fn mime_type_detection_covers_common_formats() {
        for (ext, expected) in [
            ("png", "image/png"),
            ("PNG", "image/png"),
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("gif", "image/gif"),
            ("webp", "image/webp"),
            ("bmp", "image/bmp"),
        ] {
            let path = std::path::PathBuf::from(format!("test.{ext}"));
            let mime = ImageAnalyzeTool::detect_mime_type(&path)
                .unwrap_or_else(|_| panic!("must detect {ext}"));
            assert_eq!(mime, expected);
        }
    }

    #[test]
    fn mime_type_detection_rejects_unsupported_extension() {
        let path = std::path::PathBuf::from("test.svg");
        let err = ImageAnalyzeTool::detect_mime_type(&path)
            .expect_err("svg is intentionally out of scope for vision tool");
        assert!(err.to_string().contains("Unsupported image format"));
    }

    #[tokio::test]
    async fn execute_rejects_absolute_path() {
        // Trust-boundary pin: image_path must stay inside the workspace
        // — an absolute path or a `..`-traversing path must reject
        // before any base64 / API call.
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ImageAnalyzeTool::new(fake_config());
        let outside_workspace = if cfg!(windows) {
            r"C:\Windows\System32\drivers\etc\hosts"
        } else {
            "/etc/hosts"
        };
        let err = tool
            .execute(json!({"image_path": outside_workspace}), &ctx)
            .await
            .expect_err("absolute path must reject");
        assert!(
            err.to_string()
                .contains("relative path within the workspace"),
            "error must call out the workspace boundary; got {err}"
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
