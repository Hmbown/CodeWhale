//! Model-visible Xiaomi MiMo speech/TTS generation tool.
//!
//! This mirrors the CLI `speech` / `tts` command as a first-class API tool so
//! the TUI model can generate narrated audio without shelling out to a nested
//! CodeWhale process.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};

use crate::client::{DeepSeekClient, SpeechSynthesisRequest};
use crate::config::{ApiProvider, normalize_model_name_for_provider};
use crate::network_policy::{Decision, host_from_url};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, required_str,
};

pub(crate) const DEFAULT_FORMAT: &str = "wav";
pub(crate) const DEFAULT_VOICE: &str = "mimo_default";
const VOICE_CLONE_BASE64_MAX_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const SUPPORTED_SPEECH_FORMATS: &[&str] = &["wav", "mp3", "pcm16"];

pub const SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS: &[&str] = &[
    "mimo-v2.5-tts-voiceclone",
    "mimo-v2.5-tts-voicedesign",
    "mimo-v2.5-tts",
    "mimo-v2-tts",
];

pub(crate) const SPEECH_MODEL_EXAMPLES: &[&str] = &[
    "mimo-v2.5-tts",
    "mimo-v2.5-tts-voicedesign",
    "mimo-v2.5-tts-voiceclone",
    "mimo-v2-tts",
];

pub struct SpeechTool {
    name: &'static str,
    client: Option<DeepSeekClient>,
    output_dir: Option<PathBuf>,
}

impl SpeechTool {
    #[must_use]
    pub fn new(
        name: &'static str,
        client: Option<DeepSeekClient>,
        output_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            name,
            client,
            output_dir,
        }
    }
}

#[async_trait]
impl ToolSpec for SpeechTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Generate speech/audio directly through the configured Xiaomi MiMo OpenAI-compatible API. Use this when the user asks for speech, TTS, narration, read-aloud, voice design, or voice cloning."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to synthesize. This is sent as the assistant message and is the spoken content; MiMo TTS style/audio tags may be included here."
                },
                "output": {
                    "type": "string",
                    "description": "Audio file path to write, relative to the workspace unless absolute. Default: speech.<format> in output_dir, configured [speech].output_dir, or the workspace."
                },
                "output_dir": {
                    "type": "string",
                    "description": "Directory for the default speech.<format> output file when output is omitted. Relative paths stay inside the workspace."
                },
                "model": {
                    "type": "string",
                    "description": "TTS model. Defaults to mimo-v2.5-tts, or infers voice-design/voice-clone models from voice_prompt/clone_voice.",
                    "enum": SPEECH_MODEL_EXAMPLES
                },
                "voice": {
                    "type": "string",
                    "description": "Built-in voice ID (for example mimo_default, 冰糖, 茉莉, 苏打, 白桦, Mia, Chloe, Milo, Dean) or a data:audio/...;base64,... URI for voice clone."
                },
                "instruction": {
                    "type": "string",
                    "description": "Natural-language style, emotion, speed, scene, or performance instruction. It is not spoken verbatim."
                },
                "voice_prompt": {
                    "type": "string",
                    "description": "Voice design prompt. When model is omitted this uses mimo-v2.5-tts-voicedesign."
                },
                "clone_voice": {
                    "type": "string",
                    "description": "Path to a .mp3 or .wav voice sample for cloning. When model is omitted this uses mimo-v2.5-tts-voiceclone."
                },
                "format": {
                    "type": "string",
                    "description": "Requested audio format. Default: wav. MiMo-V2.5-TTS documentation examples use wav and pcm16; mp3 is accepted when the API returns it.",
                    "enum": SUPPORTED_SPEECH_FORMATS
                },
                "stream": {
                    "type": "boolean",
                    "description": "Low-latency streaming request. The direct tool currently writes complete audio files only, so leave this false."
                }
            },
            "required": ["text"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::WritesFiles,
            ToolCapability::Network,
            ToolCapability::Sandboxable,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        // Speech generation is an explicit user-facing generation action.
        // Path resolution still enforces workspace/trusted-root boundaries.
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let text = required_str(&input, "text")?.trim().to_string();
        if text.is_empty() {
            return Err(ToolError::invalid_input("speech text cannot be empty"));
        }

        let client = self.client.clone().ok_or_else(|| {
            ToolError::not_available(
                "speech tool requires an active Xiaomi MiMo API client; configure provider = \"xiaomi-mimo\" and an API key first",
            )
        })?;

        let requested_format_raw = optional_str(&input, "format")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_FORMAT);
        let requested_format = normalize_speech_format(requested_format_raw).ok_or_else(|| {
            ToolError::invalid_input(format!(
                "unsupported speech format '{requested_format_raw}' (allowed: {})",
                SUPPORTED_SPEECH_FORMATS.join(", ")
            ))
        })?;
        if optional_bool(&input, "stream", false) {
            return Err(ToolError::invalid_input(
                "stream=true low-latency speech output is not implemented in the direct tool yet; use stream=false to generate a complete audio file",
            ));
        }
        let output_raw = optional_str(&input, "output")
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let output_path = resolve_speech_output_path(
            &input,
            context,
            output_raw,
            &requested_format,
            self.output_dir.as_ref(),
        )?;
        let output_label = output_raw
            .map(str::to_string)
            .unwrap_or_else(|| output_path.display().to_string());

        let raw_voice = optional_str(&input, "voice")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let raw_instruction = optional_str(&input, "instruction")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let voice_prompt = optional_str(&input, "voice_prompt")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let clone_voice = optional_str(&input, "clone_voice")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let voice_is_data_uri = raw_voice
            .as_deref()
            .is_some_and(|value| value.starts_with("data:audio/"));
        if clone_voice.is_some() && raw_voice.is_some() {
            return Err(ToolError::invalid_input(
                "use either clone_voice or voice for cloned voice data, not both",
            ));
        }
        let model = infer_speech_model(
            optional_str(&input, "model"),
            clone_voice.is_some() || voice_is_data_uri,
            voice_prompt.is_some(),
        );
        let model_lower = model.to_ascii_lowercase();
        if !model_lower.contains("tts") {
            return Err(ToolError::invalid_input(format!(
                "speech tool requires a TTS model (examples: {}), got '{model}'",
                SPEECH_MODEL_EXAMPLES.join(", ")
            )));
        }

        let is_voice_design = model_lower.contains("voicedesign");
        let is_voice_clone = model_lower.contains("voiceclone");
        let instruction = combine_speech_instructions(raw_instruction, voice_prompt);
        if is_voice_design
            && instruction
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ToolError::invalid_input(
                "mimo-v2.5-tts-voicedesign requires voice_prompt or instruction",
            ));
        }

        let voice = if let Some(clone_path) = clone_voice {
            let clone_path = context.resolve_path(&clone_path)?;
            Some(encode_voice_clone_data_uri(&clone_path).await?)
        } else if is_voice_design {
            None
        } else if let Some(value) = raw_voice {
            Some(value)
        } else if is_voice_clone {
            return Err(ToolError::invalid_input(
                "mimo-v2.5-tts-voiceclone requires clone_voice <mp3|wav> or voice <data-uri>",
            ));
        } else {
            Some(DEFAULT_VOICE.to_string())
        };

        check_network_policy(context, client.base_url())?;

        let response = client
            .synthesize_speech(SpeechSynthesisRequest {
                model: model.clone(),
                text,
                instruction,
                audio_format: requested_format,
                voice,
            })
            .await
            .map_err(|err| {
                ToolError::execution_failed(format!("speech synthesis failed: {err}"))
            })?;

        if let Some(parent) = output_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await.map_err(|err| {
                ToolError::execution_failed(format!(
                    "failed to create output directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        tokio::fs::write(&output_path, &response.audio_bytes)
            .await
            .map_err(|err| {
                ToolError::execution_failed(format!(
                    "failed to write audio file {}: {err}",
                    output_path.display()
                ))
            })?;

        let result = json!({
            "mode": "speech",
            "success": true,
            "api": "Xiaomi MiMo OpenAI-compatible chat/completions speech synthesis",
            "base_url": openai_compatible_base_url(client.base_url()),
            "model": response.model,
            "format": response.audio_format,
            "stream": false,
            "output": output_label,
            "absolute_output": output_path.display().to_string(),
            "bytes": response.audio_bytes.len(),
            "voice": response.voice.as_deref().map(describe_speech_voice),
            "transcript": response.transcript,
            "supported_formats": SUPPORTED_SPEECH_FORMATS,
            "supported_xiaomi_mimo_models": SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS,
        });
        ToolResult::json(&result).map_err(|err| {
            ToolError::execution_failed(format!("failed to serialize result: {err}"))
        })
    }
}

pub(crate) fn infer_speech_model(
    model: Option<&str>,
    has_clone_voice: bool,
    has_voice_prompt: bool,
) -> String {
    match model.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => normalize_model_name_for_provider(ApiProvider::XiaomiMimo, value)
            .unwrap_or_else(|| value.into()),
        None if has_clone_voice => "mimo-v2.5-tts-voiceclone".to_string(),
        None if has_voice_prompt => "mimo-v2.5-tts-voicedesign".to_string(),
        None => "mimo-v2.5-tts".to_string(),
    }
}

pub(crate) fn combine_speech_instructions(
    instruction: Option<String>,
    voice_prompt: Option<String>,
) -> Option<String> {
    match (instruction, voice_prompt) {
        (Some(instruction), Some(voice_prompt)) => {
            let instruction = instruction.trim();
            let voice_prompt = voice_prompt.trim();
            if instruction.is_empty() {
                Some(voice_prompt.to_string()).filter(|value| !value.is_empty())
            } else if voice_prompt.is_empty() {
                Some(instruction.to_string()).filter(|value| !value.is_empty())
            } else {
                Some(format!("{voice_prompt}\n\n{instruction}"))
            }
        }
        (Some(value), None) | (None, Some(value)) => {
            let value = value.trim().to_string();
            if value.is_empty() { None } else { Some(value) }
        }
        (None, None) => None,
    }
}

pub(crate) fn normalize_speech_format(format: &str) -> Option<String> {
    let normalized = format.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "wav" | "mp3" | "pcm16" => Some(normalized),
        "pcm" => Some("pcm16".to_string()),
        _ => None,
    }
}

pub(crate) fn default_speech_output_name(format: &str) -> String {
    format!(
        "speech.{}",
        normalize_speech_format(format)
            .as_deref()
            .unwrap_or(DEFAULT_FORMAT)
    )
}

fn resolve_speech_output_path(
    input: &Value,
    context: &ToolContext,
    output_raw: Option<&str>,
    format: &str,
    configured_output_dir: Option<&PathBuf>,
) -> Result<PathBuf, ToolError> {
    if let Some(output) = output_raw {
        return context.resolve_path(output);
    }

    let filename = default_speech_output_name(format);
    if let Some(output_dir) = optional_str(input, "output_dir")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(context.resolve_path(output_dir)?.join(filename));
    }

    if let Some(output_dir) = configured_output_dir {
        return Ok(output_dir.join(filename));
    }

    Ok(context.workspace.join(filename))
}

async fn encode_voice_clone_data_uri(path: &Path) -> Result<String, ToolError> {
    let bytes = tokio::fs::read(path).await.map_err(|err| {
        ToolError::execution_failed(format!(
            "failed to read voice clone sample {}: {err}",
            path.display()
        ))
    })?;

    voice_clone_data_uri_from_bytes(path, &bytes)
        .map_err(|err| ToolError::invalid_input(err.to_string()))
}

pub(crate) fn encode_voice_clone_sample_data_uri(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read voice clone sample {}", path.display()))?;

    voice_clone_data_uri_from_bytes(path, &bytes)
}

fn voice_clone_data_uri_from_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<String> {
    let base64_audio = general_purpose::STANDARD.encode(bytes);
    if base64_audio.len() > VOICE_CLONE_BASE64_MAX_BYTES {
        anyhow::bail!(
            "voice clone sample is too large after base64 encoding ({} bytes > 10 MB)",
            base64_audio.len()
        );
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        other => {
            anyhow::bail!("unsupported voice clone sample extension '{other}'. Use .mp3 or .wav.");
        }
    };

    Ok(format!("data:{mime};base64,{base64_audio}"))
}

pub(crate) fn describe_speech_voice(voice: &str) -> String {
    if voice.starts_with("data:") {
        "embedded voice clone sample".to_string()
    } else {
        voice.to_string()
    }
}

fn openai_compatible_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") || trimmed.ends_with("/beta") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

fn check_network_policy(context: &ToolContext, base_url: &str) -> Result<(), ToolError> {
    let Some(decider) = context.network_policy.as_ref() else {
        return Ok(());
    };
    let display_url = openai_compatible_base_url(base_url);
    let Some(host) = host_from_url(&display_url) else {
        return Ok(());
    };
    match decider.evaluate(&host, "speech") {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(ToolError::permission_denied(format!(
            "speech network call to '{host}' blocked by network policy"
        ))),
        Decision::Prompt => Err(ToolError::permission_denied(format!(
            "speech network call to '{host}' requires approval; re-run after `/network allow {host}` or set network.default = \"allow\" in config"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_speech_model_from_requested_mode() {
        assert_eq!(infer_speech_model(None, false, false), "mimo-v2.5-tts");
        assert_eq!(
            infer_speech_model(None, false, true),
            "mimo-v2.5-tts-voicedesign"
        );
        assert_eq!(
            infer_speech_model(None, true, false),
            "mimo-v2.5-tts-voiceclone"
        );
        assert_eq!(
            infer_speech_model(Some("mimo-tts"), false, false),
            "mimo-v2.5-tts"
        );
        assert_eq!(
            infer_speech_model(Some("mimo-v2-tts"), false, false),
            "mimo-v2-tts"
        );
    }

    #[test]
    fn combines_voice_prompt_before_instruction() {
        assert_eq!(
            combine_speech_instructions(
                Some("Speak warmly.".to_string()),
                Some("Young Chinese female voice".to_string())
            )
            .as_deref(),
            Some("Young Chinese female voice\n\nSpeak warmly.")
        );
        assert_eq!(
            combine_speech_instructions(Some("  calm  ".to_string()), None).as_deref(),
            Some("calm")
        );
    }

    #[test]
    fn normalizes_documented_speech_formats() {
        assert_eq!(normalize_speech_format("WAV").as_deref(), Some("wav"));
        assert_eq!(normalize_speech_format("pcm16").as_deref(), Some("pcm16"));
        assert_eq!(normalize_speech_format("pcm").as_deref(), Some("pcm16"));
        assert_eq!(normalize_speech_format("flac"), None);
    }

    #[test]
    fn supported_xiaomi_mimo_speech_models_are_tts_only() {
        assert!(
            SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS
                .iter()
                .all(|model| model.to_ascii_lowercase().contains("tts")),
            "model-visible speech list must not include chat-only MiMo models"
        );
        assert!(SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS.contains(&"mimo-v2.5-tts"));
        assert!(!SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS.contains(&"mimo-v2.5-pro"));
        assert!(!SUPPORTED_XIAOMI_MIMO_SPEECH_MODELS.contains(&"mimo-v2.5"));
    }

    #[test]
    fn configured_output_dir_is_used_for_default_tool_output() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(tmp.path().to_path_buf());
        let configured = tmp.path().join("speech-artifacts");

        let output = resolve_speech_output_path(
            &json!({"text": "hello"}),
            &context,
            None,
            "pcm",
            Some(&configured),
        )
        .expect("output path");

        assert_eq!(output, configured.join("speech.pcm16"));
    }

    #[test]
    fn displays_openai_compatible_base_url() {
        assert_eq!(
            openai_compatible_base_url("https://api.xiaomimimo.com"),
            "https://api.xiaomimimo.com/v1"
        );
        assert_eq!(
            openai_compatible_base_url("https://api.xiaomimimo.com/v1"),
            "https://api.xiaomimimo.com/v1"
        );
    }

    #[test]
    fn speech_tool_is_auto_approved_but_not_read_only() {
        let tool = SpeechTool::new("speech", None, None);
        assert_eq!(tool.name(), "speech");
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
        assert!(!tool.is_read_only());
        let schema = tool.input_schema();
        assert!(schema.to_string().contains("mimo-v2.5-tts-voiceclone"));
        assert!(schema.to_string().contains("pcm16"));
        assert!(schema.to_string().contains("stream"));
    }
}
