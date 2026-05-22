//! `image_analyze` tool — analyze images using a dedicated vision model.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

use crate::config::VisionModelConfig;
use crate::llm_client::{LlmError, RetryConfig, with_retry};
use crate::tools::image_fetch::download_image;
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

    /// Resolve the image cache directory: `~/.deepseek/cache/images/`
    fn image_cache_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".deepseek")
            .join("cache")
            .join("images")
    }
}

#[async_trait]
impl ToolSpec for ImageAnalyzeTool {
    fn name(&self) -> &str {
        "image_analyze"
    }

    fn description(&self) -> &str {
        "Analyze an image using the configured vision model. \
         Supports PNG, JPEG, GIF, WebP, and BMP formats. \
         Provide either `image_path` (local workspace file) or \
         `image_url` (remote HTTP/HTTPS URL to download and analyze)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "image_path": {
                    "type": "string",
                    "description": "Path to the image file to analyze (relative to workspace)"
                },
                "image_url": {
                    "type": "string",
                    "description": "HTTP/HTTPS URL of an image to download and analyze"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt to guide the analysis."
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let prompt = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("Describe this image in detail.");

        // Resolve the image source: local path or remote URL.
        let image_source: ImageSource = if let Some(url) = input
            .get("image_url")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            // Download image from URL to local cache.
            let cache_dir = Self::image_cache_dir();
            let downloaded = download_image(url, &cache_dir, context).await?;
            ImageSource::Url {
                url: url.to_string(),
                cached_path: downloaded.path,
                mime_type: downloaded.content_type,
            }
        } else {
            let image_path = required_str(&input, "image_path")?;
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
            let mime_type = Self::detect_mime_type(&resolved_path)?;
            ImageSource::Local {
                path: resolved_path,
                mime_type,
            }
        };

        let (image_data, mime_type) = match &image_source {
            ImageSource::Local { path, mime_type } => {
                let (data, _) = Self::read_image_file(path).await?;
                (data, mime_type.clone())
            }
            ImageSource::Url {
                cached_path,
                mime_type,
                ..
            } => {
                let (data, _) = Self::read_image_file(cached_path).await?;
                (data, mime_type.clone())
            }
        };

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

        let source_label = match &image_source {
            ImageSource::Local { path, .. } => {
                format!("local:{}", path.display())
            }
            ImageSource::Url { url, .. } => {
                format!("url:{url}")
            }
        };

        let result = json!({
            "analysis": content,
            "model": model,
            "source": source_label,
        });

        ToolResult::json(&result)
            .map_err(|e| ToolError::execution_failed(format!("Failed to serialize result: {e}")))
    }
}

/// Internal enum for image source resolution.
enum ImageSource {
    Local { path: PathBuf, mime_type: String },
    Url { url: String, cached_path: PathBuf, mime_type: String },
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

    #[test]
    fn input_schema_accepts_image_url() {
        let tool = ImageAnalyzeTool::new(fake_config());
        let schema = tool.input_schema();
        let props = schema.get("properties").expect("has properties");
        assert!(props.get("image_url").is_some(), "must have image_url property");
        let required = schema.get("required").expect("has required");
        let req_arr = required.as_array().expect("required is array");
        assert!(req_arr.is_empty(), "required should be empty (one of image_path or image_url)");
    }
}
