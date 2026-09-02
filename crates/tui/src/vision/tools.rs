//! `image_analyze` tool — analyze images using a dedicated vision model.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};

use crate::client::DeepSeekClient;
use crate::config::ApiProvider;
use crate::config::VisionModelConfig;
use crate::llm_client::{LlmError, RetryConfig, sanitize_http_error_body, with_retry};
use crate::tools::spec::{
    ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

pub struct ImageAnalyzeTool {
    config: VisionModelConfig,
    client: reqwest::Client,
    route_client: Option<DeepSeekClient>,
}

impl ImageAnalyzeTool {
    #[cfg(test)]
    #[must_use]
    pub fn new(config: VisionModelConfig) -> Self {
        Self::new_with_route_client(config, None)
    }

    #[must_use]
    pub fn new_with_route_client(
        config: VisionModelConfig,
        route_client: Option<DeepSeekClient>,
    ) -> Self {
        let client = crate::tls::reqwest_client_builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            config,
            client,
            route_client,
        }
    }

    async fn read_image_file(path: &Path) -> Result<(String, String), ToolError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| ToolError::execution_failed(format!("Failed to read image file: {e}")))?;

        let mime_type = Self::detect_mime_type(path)?;
        let base64_data = BASE64.encode(&bytes);
        Ok((base64_data, mime_type))
    }

    fn resolve_image_path(workspace: &Path, image_path: &str) -> Result<PathBuf, ToolError> {
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

        let workspace = workspace.canonicalize().map_err(|e| {
            ToolError::execution_failed(format!("Failed to resolve workspace path: {e}"))
        })?;
        let candidate = workspace.join(image_path_buf);
        let resolved = candidate.canonicalize().map_err(|e| {
            ToolError::execution_failed(format!("Failed to resolve image file: {e}"))
        })?;
        if !resolved.starts_with(&workspace) {
            return Err(ToolError::execution_failed(
                "image_path must resolve within the workspace and cannot escape it.",
            ));
        }
        Ok(resolved)
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

    fn is_xiaomi_mimo_model(model: &str) -> bool {
        let normalized = model.trim().to_ascii_lowercase();
        let normalized = normalized.strip_prefix("xiaomi/").unwrap_or(&normalized);
        normalized.starts_with("mimo-")
    }

    fn uses_max_completion_tokens(config: &VisionModelConfig) -> bool {
        if Self::is_xiaomi_mimo_model(&config.model) {
            return true;
        }

        let base_url = config.base_url.as_deref().unwrap_or_default();
        let Ok(url) = reqwest::Url::parse(base_url) else {
            return false;
        };
        let Some(domain) = url.domain() else {
            return false;
        };

        domain.eq_ignore_ascii_case("xiaomimimo.com")
            || domain.to_ascii_lowercase().ends_with(".xiaomimimo.com")
    }

    fn request_payload(&self, prompt: &str, image_data: &str, mime_type: &str) -> Value {
        let mut payload = json!({
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
            ]
        });

        let token_limit_field = if Self::uses_max_completion_tokens(&self.config) {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        let configured_base = self.base_url();
        let route_cap = self
            .route_client
            .as_ref()
            .filter(|client| {
                client.base_url().trim_end_matches('/') == configured_base.trim_end_matches('/')
            })
            .map_or_else(
                || {
                    // A standalone `[vision_model]` route has no resolved
                    // max-model-len fact. Do not guess one or let a process
                    // override turn a capability maximum into an unbounded
                    // request; a matched active client above carries exact
                    // route limits when the vision route is shared.
                    crate::route_budget::effective_max_output_tokens_for_route(
                        ApiProvider::Custom,
                        &self.config.model,
                        None,
                    )
                    .min(65_536)
                },
                |client| client.effective_max_output_tokens(&self.config.model),
            );
        payload[token_limit_field] = json!(route_cap);

        payload
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

        let resolved_path = Self::resolve_image_path(&context.workspace, image_path)?;
        let (image_data, mime_type) = Self::read_image_file(&resolved_path).await?;

        let payload = self.request_payload(prompt, &image_data, &mime_type);

        let url = format!("{}/chat/completions", self.base_url());
        let api_key = self.api_key();

        let retry_config = RetryConfig {
            max_retries: 3,
            initial_delay: 1.0,
            max_delay: 30.0,
            enabled: true,
            ..Default::default()
        };
        let _inference = match self.route_client.as_ref() {
            Some(client) => client.acquire_remote_control_inference_permit().await,
            None => Some(crate::client::acquire_remote_control_inference_participant().await),
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
                        .header("Authorization", format!("Bearer {api_key}"))
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
                        let error_text = sanitize_http_error_body(
                            Some("Vision provider"),
                            status.as_u16(),
                            &error_text,
                        );
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

    #[cfg(unix)]
    fn create_file_symlink(
        target: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_file_symlink(
        target: &std::path::Path,
        link: &std::path::Path,
    ) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    fn fake_config() -> VisionModelConfig {
        VisionModelConfig {
            model: "test-vision-model".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: Some("https://example.invalid/v1".to_string()),
        }
    }

    fn standalone_vision_cap(model: &str) -> u64 {
        u64::from(
            crate::route_budget::effective_max_output_tokens_for_route(
                ApiProvider::Custom,
                model,
                None,
            )
            .min(65_536),
        )
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

    #[test]
    fn generic_vision_payload_uses_max_tokens() {
        let tool = ImageAnalyzeTool::new(fake_config());

        let payload = tool.request_payload("describe", "abc123", "image/png");

        assert_eq!(
            payload.get("max_tokens").and_then(Value::as_u64),
            Some(standalone_vision_cap(&tool.config.model))
        );
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_completion_tokens").is_none());
    }

    #[test]
    fn xiaomi_mimo_vision_payload_uses_max_completion_tokens() {
        let mut config = fake_config();
        config.model = "mimo-v2.5".to_string();
        config.base_url = Some("https://api.xiaomimimo.com/v1".to_string());
        let tool = ImageAnalyzeTool::new(config);

        let payload = tool.request_payload("describe", "abc123", "image/png");

        assert_eq!(
            payload.get("max_completion_tokens").and_then(Value::as_u64),
            Some(standalone_vision_cap(&tool.config.model))
        );
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_tokens").is_none());
    }

    #[test]
    fn xiaomi_mimo_vision_payload_uses_max_completion_tokens_with_custom_proxy() {
        let mut config = fake_config();
        config.model = "mimo-v2.5".to_string();
        config.base_url = Some("https://vision-proxy.example.invalid/v1".to_string());
        let tool = ImageAnalyzeTool::new(config);

        let payload = tool.request_payload("describe", "abc123", "image/png");

        assert_eq!(
            payload.get("max_completion_tokens").and_then(Value::as_u64),
            Some(standalone_vision_cap(&tool.config.model))
        );
        assert!(payload.get("max_tokens").is_none());
    }

    #[test]
    fn matched_vision_route_uses_bound_client_window_cap() {
        let _lock = crate::test_support::lock_test_env();
        let _canonical =
            crate::test_support::EnvVarGuard::set("CODEWHALE_MAX_OUTPUT_TOKENS", "384000");
        let base_url = "http://127.0.0.1:18080/v1".to_string();
        let model = "DeepSeek-V4-Flash".to_string();
        let client = DeepSeekClient::new(&crate::config::Config {
            provider: Some("vllm".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                vllm: crate::config::ProviderConfig {
                    base_url: Some(base_url.clone()),
                    model: Some(model.clone()),
                    context_window: Some(327_680),
                    ..crate::config::ProviderConfig::default()
                },
                ..crate::config::ProvidersConfig::default()
            }),
            ..crate::config::Config::default()
        })
        .expect("bound vLLM client");
        let tool = ImageAnalyzeTool::new_with_route_client(
            VisionModelConfig {
                model,
                api_key: None,
                base_url: Some(base_url),
            },
            Some(client),
        );

        let payload = tool.request_payload("describe", "abc123", "image/png");
        assert_eq!(payload["max_tokens"], 325_632);
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

    #[tokio::test]
    async fn execute_rejects_symlink_that_resolves_outside_workspace() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        let outside_image = outside.path().join("outside.png");
        std::fs::write(&outside_image, b"not a real png").expect("write outside image");
        let link = workspace.path().join("linked.png");
        if let Err(err) = create_file_symlink(&outside_image, &link) {
            eprintln!("skipping symlink assertion: {err}");
            return;
        }

        let ctx = ToolContext::new(workspace.path().to_path_buf());
        let tool = ImageAnalyzeTool::new(fake_config());
        let err = tool
            .execute(json!({"image_path": "linked.png"}), &ctx)
            .await
            .expect_err("symlink target outside workspace must reject before reading");
        assert!(
            err.to_string().contains("resolve within the workspace"),
            "error must call out the canonical workspace boundary; got {err}"
        );
    }
}
