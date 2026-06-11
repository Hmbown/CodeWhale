//! Voice input command — `/voice`, `/voice-send`, `/voice-control`
//!
//! Records audio from the default microphone, sends it to the configured
//! provider's API for transcription, and inserts the transcribed text into
//! the composer. The interaction model mirrors MiMo Code's voice UX:
//!
//!   `/voice`         — toggle voice input on/off
//!   `/voice-send`    — toggle auto-send after transcription
//!   `/voice-control` — toggle AI-powered voice control (edit/send/agent)
//!
//! ## Recording
//!
//! Uses platform-specific command-line tools (sox, rec, arecord) to capture
//! 16kHz mono 16-bit PCM audio. Records until a silence gap is detected or
//! the maximum duration is reached (default 10 s).

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

// --- Recorder detection ----------------------------------------------------

/// Platform-specific recorder definitions.
#[derive(Debug, Clone)]
struct Recorder {
    cmd: &'static str,
    /// Returns the CLI arguments for piping raw 16kHz mono S16_LE PCM to stdout.
    pipe_args: &'static [&'static str],
}

fn detect_recorder() -> Option<Recorder> {
    let candidates: &[Recorder] = if cfg!(target_os = "macos") {
        &[
            Recorder {
                cmd: "sox",
                pipe_args: &["-d", "-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
            },
            Recorder {
                cmd: "rec",
                pipe_args: &["-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
            },
        ]
    } else if cfg!(target_os = "linux") {
        &[
            Recorder {
                cmd: "arecord",
                pipe_args: &["-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "raw"],
            },
            Recorder {
                cmd: "sox",
                pipe_args: &["-d", "-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
            },
        ]
    } else if cfg!(target_os = "windows") {
        &[Recorder {
            cmd: "sox",
            pipe_args: &["-d", "-r", "16000", "-c", "1", "-b", "16", "-t", "raw", "-"],
        }]
    } else {
        &[]
    };

    candidates
        .iter()
        .find(|r| {
            // Check if the command exists by trying to spawn it with --version
            Command::new(r.cmd)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
        })
        .cloned()
}

/// Check whether voice recording is available on this system.
pub fn is_available() -> bool {
    detect_recorder().is_some()
}

// --- WAV encoding ----------------------------------------------------------

/// Encode raw 16kHz mono S16_LE PCM samples as a WAV buffer.
fn encode_wav(samples: &[i16]) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let sample_rate: u32 = 16000;
    let mut buf = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_size).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &sample in samples {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    buf
}

// --- Recording -------------------------------------------------------------

/// Maximum recording duration in seconds before auto-stopping.
const MAX_RECORD_SECS: u64 = 10;
/// Minimum segment duration in seconds to consider as valid speech.
const MIN_SEGMENT_SECS: f64 = 0.3;

/// Record audio from the default microphone.
///
/// Returns raw 16kHz mono S16_LE PCM samples. Returns `None` if no recorder
/// is available or the recording failed.
#[allow(dead_code)]
pub fn record_audio() -> Option<(Vec<i16>, Duration)> {
    let recorder = detect_recorder()?;
    let start = std::time::Instant::now();

    let mut child = Command::new(recorder.cmd)
        .args(recorder.pipe_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let mut reader = std::io::BufReader::new(stdout);
    let mut all_samples: Vec<i16> = Vec::with_capacity(16000 * MAX_RECORD_SECS as usize);

    // Read until timeout or silence
    let mut buf = [0u8; 320]; // 10ms of 16kHz S16_LE
    let max_duration = Duration::from_secs(MAX_RECORD_SECS);
    let mut silence_samples = 0u32;
    let mut had_speech = false;
    let speech_threshold: i16 = 500; // RMS-based speech detection threshold
    let silence_duration_samples = 16000u32; // 1 second of silence to stop

    loop {
        use std::io::Read;
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                let chunk: Vec<i16> = buf
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect();

                // Simple RMS-based VAD
                let rms = (chunk
                    .iter()
                    .map(|&s| (s as f64) * (s as f64))
                    .sum::<f64>()
                    / chunk.len() as f64)
                    .sqrt();
                let is_speech = rms > speech_threshold as f64;

                if is_speech {
                    had_speech = true;
                    silence_samples = 0;
                } else if had_speech {
                    silence_samples += chunk.len() as u32;
                }

                if had_speech {
                    all_samples.extend_from_slice(&chunk);
                }

                if start.elapsed() > max_duration {
                    let _ = child.kill();
                    break;
                }
                if had_speech && silence_samples >= silence_duration_samples {
                    let _ = child.kill();
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(_) => {
                let _ = child.kill();
                break;
            }
        }
    }

    let _ = child.wait();
    let elapsed = start.elapsed();

    let min_samples = (MIN_SEGMENT_SECS * 16000.0) as usize;
    if all_samples.len() < min_samples {
        return None;
    }

    Some((all_samples, elapsed))
}

// --- Transcription ---------------------------------------------------------

/// Re-export for use by the command handler: regular expression to match
/// explicit send commands at the end of transcribed text.
#[allow(dead_code)]
pub static SEND_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)^(send\s*it|发送)\s*$").unwrap()
});

/// Send audio to the provider's API for transcription.
///
/// Uses the chat completions endpoint with `input_audio` content blocks.
/// The model is chosen based on the active provider.
fn transcribe_internal(
    api_key: &str,
    base_url: &str,
    audio_samples: &[i16],
) -> Result<String, String> {
    let wav = encode_wav(audio_samples);
    let base64 = base64_encode(&wav);
    let data_url = format!("data:audio/wav;base64,{base64}");

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let body = serde_json::json!({
        "model": "mimo-v2.5-asr",
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_audio",
                        "input_audio": {
                            "data": data_url
                        }
                    }
                ]
            }
        ],
        "asr_options": {
            "language": "auto"
        }
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .map_err(|e| format!("Transcription request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Transcription API returned status {}",
            resp.status()
        ));
    }

    let data: serde_json::Value =
        resp.json().map_err(|e| format!("Failed to parse response: {e}"))?;

    data["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "No transcription in response".to_string())
}

/// Simpler base64 encoding (no external crate dependency needed for the
/// command layer — the app already depends on base64 via other crates).
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// --- Voice control ---------------------------------------------------------

/// Process audio through the voice-control pipeline (AI-powered edit/send/
/// agent actions).  This mirrors MiMo Code's `processVoiceControl`.
///
/// When voice-control is off, plain transcription is used instead.
#[allow(dead_code)]
pub fn process_voice_control(
    api_key: &str,
    base_url: &str,
    audio_samples: &[i16],
    current_text: &str,
    _current_agent: &str,
    _available_agents: &[String],
) -> Result<VoiceControlResult, String> {
    let wav = encode_wav(audio_samples);
    let base64 = base64_encode(&wav);
    let data_url = format!("data:audio/wav;base64,{base64}");

    let user_context = serde_json::json!({
        "current_text": current_text,
        "cursor": "end",
    });

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let body = serde_json::json!({
        "model": "mimo-v2.5",
        "messages": [
            {
                "role": "system",
                "content": "You are a voice input assistant. Transcribe the user's speech. Output JSON: {\"text\": \"transcribed text\"}."
            },
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": user_context.to_string() },
                    { "type": "input_audio", "input_audio": { "data": data_url } }
                ]
            }
        ],
        "response_format": { "type": "json_object" }
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .map_err(|e| format!("Voice control request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Voice control API returned status {}",
            resp.status()
        ));
    }

    let data: serde_json::Value =
        resp.json().map_err(|e| format!("Failed to parse response: {e}"))?;

    let content = data["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "No response content".to_string())?;

    let parsed: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("Failed to parse voice control JSON: {e}"))?;

    let text = parsed["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "No text field in voice control response".to_string())?;

    Ok(VoiceControlResult { text })
}

/// Result of voice control processing.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VoiceControlResult {
    pub text: String,
}

// --- Command handlers ------------------------------------------------------

/// Handle the `/voice` command: toggle voice input or perform a one-shot
/// recording + transcription.
pub fn voice(app: &mut App) -> crate::commands::CommandResult {
    use crate::commands::CommandResult;
    use crate::localization::tr;

    let locale = app.ui_locale;

    // Toggle voice-enabled state
    app.voice_enabled = !app.voice_enabled;

    if app.voice_enabled {
        // Perform recording + transcription immediately
        if !is_available() {
            app.voice_enabled = false;
            return CommandResult::error(tr(locale, MessageId::VoiceErrNoRecorder));
        }

        match record_and_transcribe(app) {
            Ok((text, _duration)) => {
                let clean = text.trim().to_string();
                if clean.is_empty() {
                    return CommandResult::message(tr(locale, MessageId::VoiceErrEmptySend));
                }
                CommandResult {
                    message: Some(format!(
                        "{}: {}",
                        tr(locale, MessageId::VoiceTranscribed),
                        &clean
                    )),
                    action: Some(AppAction::InsertComposerText(clean)),
                    is_error: false,
                }
            }
            Err(err) => {
                app.voice_enabled = false;
                CommandResult::error(err)
            }
        }
    } else {
        CommandResult::message(tr(locale, MessageId::VoiceDisabled))
    }
}

/// Handle the `/voice-send` command: toggle auto-send after transcription.
pub fn voice_send(app: &mut App) -> crate::commands::CommandResult {
    use crate::commands::CommandResult;
    use crate::localization::tr;

    let locale = app.ui_locale;
    app.voice_send_enabled = !app.voice_send_enabled;

    let msg = if app.voice_send_enabled {
        tr(locale, MessageId::VoiceSendEnabled)
    } else {
        tr(locale, MessageId::VoiceSendDisabled)
    };
    CommandResult::message(msg)
}

/// Handle the `/voice-control` command: toggle AI-powered voice control.
pub fn voice_control(app: &mut App) -> crate::commands::CommandResult {
    use crate::commands::CommandResult;
    use crate::localization::tr;

    let locale = app.ui_locale;
    app.voice_control_enabled = !app.voice_control_enabled;

    let msg = if app.voice_control_enabled {
        tr(locale, MessageId::VoiceControlEnabled)
    } else {
        tr(locale, MessageId::VoiceControlDisabled)
    };
    CommandResult::message(msg)
}

// --- Internal helpers ------------------------------------------------------

/// Perform a complete record+transcribe cycle.
fn record_and_transcribe(app: &mut App) -> Result<(String, Duration), String> {
    use crate::localization::tr;

    let locale = app.ui_locale;

    // Set status to let the user know
    let _old_status = app.status_message.take();
    app.status_message = Some(tr(locale, MessageId::VoiceRecording).to_string());

    let (samples, duration) = record_audio().ok_or_else(|| {
        tr(locale, MessageId::VoiceErrNoRecorder).to_string()
    })?;

    app.status_message = Some(tr(locale, MessageId::VoiceProcessing).to_string());

    // Gather provider credentials
    let (api_key, base_url) = get_active_provider_credentials(app)
        .ok_or_else(|| tr(locale, MessageId::VoiceErrNoAuth).to_string())?;

    let text = transcribe_internal(&api_key, &base_url, &samples)
        .map_err(|e| format!("{}: {e}", tr(locale, MessageId::VoiceErrNetwork)))?;

    Ok((text, duration))
}

/// Get the API key and base URL for the active provider.
fn get_active_provider_credentials(app: &App) -> Option<(String, String)> {
    let api_key = app.api_key.clone()?;
    let base_url = app
        .base_url
        .clone()
        .unwrap_or_else(|| default_base_url_for(app.api_provider).to_string());
    Some((api_key, base_url))
}

fn default_base_url_for(provider: crate::config::ApiProvider) -> &'static str {
    match provider {
        crate::config::ApiProvider::XiaomiMimo => "https://api.xiaomimimo.com/v1",
        crate::config::ApiProvider::Deepseek => "https://api.deepseek.com/v1",
        crate::config::ApiProvider::DeepseekCN => "https://api.deepseek.com/v1",
        crate::config::ApiProvider::Openai => "https://api.openai.com/v1",
        crate::config::ApiProvider::Openrouter => "https://openrouter.ai/api/v1",
        crate::config::ApiProvider::Novita => "https://api.novita.ai/v3/openai",
        crate::config::ApiProvider::NvidiaNim => "https://integrate.api.nvidia.com/v1",
        crate::config::ApiProvider::Ollama => "http://localhost:11434/v1",
        crate::config::ApiProvider::Moonshot => "https://api.moonshot.cn/v1",
        crate::config::ApiProvider::Volcengine => "https://ark.cn-beijing.volces.com/api/v3",
        crate::config::ApiProvider::Arcee => "https://api.arcee.ai/v1",
        _ => "https://api.openai.com/v1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_encoding_produces_valid_header() {
        let samples = vec![0i16; 16000]; // 1 second of silence
        let wav = encode_wav(&samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        // data size = 16000 * 2 = 32000
        assert_eq!(&wav[4..8], &(36 + 32000u32).to_le_bytes());
    }

    #[test]
    fn wav_encoding_empty_is_minimal() {
        let wav = encode_wav(&[]);
        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[4..8], &36u32.to_le_bytes());
    }

    #[test]
    fn send_re_matches() {
        assert!(SEND_RE.is_match("send it"));
        assert!(SEND_RE.is_match("Send It"));
        assert!(SEND_RE.is_match("发送"));
        assert!(!SEND_RE.is_match("send it now"));
        assert!(!SEND_RE.is_match("帮我发送一封邮件"));
        assert!(!SEND_RE.is_match("发送邮件"));
    }

    #[test]
    fn recorder_detection_does_not_crash() {
        // Just verify the function runs without panicking
        let _ = is_available();
    }
}
