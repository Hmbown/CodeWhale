//! Voice input commands — `/voice`, `/voice-send`, `/voice-control`.
//!
//! Records audio from the default microphone, sends it to the configured
//! provider's API for transcription, and inserts the transcribed text into
//! the composer. The interaction model mirrors MiMo Code's voice UX:
//!
//!   `/voice`         — toggle voice input on/off (records when toggled on)
//!   `/voice-send`    — toggle auto-send when the transcript ends with
//!                      "send it" / "发送"
//!   `/voice-control` — toggle AI-assisted dictation that sees the current
//!                      composer text
//!
//! The slash commands only flip state and emit [`AppAction::VoiceCapture`];
//! the actual capture runs in the UI event loop where the live [`Config`]
//! supplies provider credentials. That keeps the handlers side-effect free
//! (the registry smoke tests execute every command) and avoids caching
//! auth material on [`App`].
//!
//! ## Recording
//!
//! Uses platform-specific command-line tools (sox, rec, arecord) to capture
//! 16kHz mono 16-bit PCM audio. Records until a silence gap is detected or
//! the maximum duration is reached (default 10 s).

use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::commands::CommandResult;
use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::config::Config;
use crate::localization::{MessageId, tr};
use crate::tui::app::{App, AppAction};

/// Transcription model requested from the provider's chat-completions API.
const ASR_MODEL: &str = "mimo-v2.5-asr";
/// Model used for the AI-assisted voice-control pipeline.
const VOICE_CONTROL_MODEL: &str = "mimo-v2.5";

pub(in crate::commands) const VOICE_INFO: CommandInfo = CommandInfo {
    name: "voice",
    aliases: &["yuyin", "语音"],
    usage: "/voice",
    description_id: MessageId::CmdVoiceDescription,
};

pub(in crate::commands) const VOICE_SEND_INFO: CommandInfo = CommandInfo {
    name: "voicesend",
    aliases: &["voice-send", "yuyinsend", "语音发送"],
    usage: "/voicesend",
    description_id: MessageId::CmdVoiceSendDescription,
};

pub(in crate::commands) const VOICE_CONTROL_INFO: CommandInfo = CommandInfo {
    name: "voicecontrol",
    aliases: &["voice-control", "yuyincontrol", "语音控制"],
    usage: "/voicecontrol",
    description_id: MessageId::CmdVoiceControlDescription,
};

pub(in crate::commands) struct VoiceCmd;
pub(in crate::commands) struct VoiceSendCmd;
pub(in crate::commands) struct VoiceControlCmd;

impl RegisterCommand for VoiceCmd {
    fn info() -> &'static CommandInfo {
        &VOICE_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        voice(app)
    }
}

impl RegisterCommand for VoiceSendCmd {
    fn info() -> &'static CommandInfo {
        &VOICE_SEND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        voice_send(app)
    }
}

impl RegisterCommand for VoiceControlCmd {
    fn info() -> &'static CommandInfo {
        &VOICE_CONTROL_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        voice_control(app)
    }
}

// --- Recorder detection ----------------------------------------------------

/// Platform-specific recorder definitions.
#[derive(Debug, Clone)]
struct Recorder {
    cmd: &'static str,
    /// CLI arguments for piping raw 16kHz mono S16_LE PCM to stdout.
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
/// is available, the recording failed, or no speech was detected.
fn record_audio() -> Option<(Vec<i16>, Duration)> {
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
                let rms = (chunk.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>()
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

// --- Auto-send suffix ------------------------------------------------------

/// Matches an explicit send instruction at the end of transcribed text:
/// "send it" (any spacing/case) or 发送/發送, with trailing punctuation.
static SEND_SUFFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[\s,，.。!！?？]+)(?:send\s*it|发送|發送)[\s.。!！?？]*$").unwrap()
});

/// Split a transcript into the message remainder and whether it ended with an
/// explicit send instruction. `"ship the fix, send it"` → `("ship the fix", true)`.
fn split_send_suffix(text: &str) -> (&str, bool) {
    match SEND_SUFFIX_RE.find(text) {
        Some(found) => (text[..found.start()].trim(), true),
        None => (text.trim(), false),
    }
}

// --- Transcription ---------------------------------------------------------

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

async fn post_chat_completions(
    api_key: &str,
    base_url: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = crate::tls::reqwest_client();
    let resp = client
        .post(chat_completions_url(base_url))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .timeout(Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("API returned status {}", resp.status()));
    }

    resp.json()
        .await
        .map_err(|e| format!("failed to parse response: {e}"))
}

/// Send audio to the provider's API for plain transcription.
///
/// Uses the chat completions endpoint with `input_audio` content blocks.
async fn transcribe(
    api_key: &str,
    base_url: &str,
    audio_samples: &[i16],
) -> Result<String, String> {
    let wav = encode_wav(audio_samples);
    let data_url = format!("data:audio/wav;base64,{}", base64_encode(&wav));

    let body = serde_json::json!({
        "model": ASR_MODEL,
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

    let data = post_chat_completions(api_key, base_url, body).await?;
    data["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "no transcription in response".to_string())
}

/// Process audio through the voice-control pipeline: AI-assisted dictation
/// that sees the current composer text, mirroring MiMo Code's
/// `processVoiceControl`. Used when `/voice-control` is enabled.
async fn process_voice_control(
    api_key: &str,
    base_url: &str,
    audio_samples: &[i16],
    current_text: &str,
) -> Result<String, String> {
    let wav = encode_wav(audio_samples);
    let data_url = format!("data:audio/wav;base64,{}", base64_encode(&wav));

    let user_context = serde_json::json!({
        "current_text": current_text,
        "cursor": "end",
    });

    let body = serde_json::json!({
        "model": VOICE_CONTROL_MODEL,
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

    let data = post_chat_completions(api_key, base_url, body).await?;
    let content = data["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "no response content".to_string())?;

    let parsed: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| format!("failed to parse voice control JSON: {e}"))?;

    parsed["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "no text field in voice control response".to_string())
}

// --- Capture orchestration (UI event loop) ---------------------------------

/// What the UI should do with a finished capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceCaptureOutcome {
    /// Insert the transcribed text into the composer at the cursor.
    Insert(String),
    /// Submit this text as a message (auto-send).
    Send(String),
}

/// Perform a complete record + transcribe cycle.
///
/// Runs in the UI event loop (see [`AppAction::VoiceCapture`]) so provider
/// credentials come from the live [`Config`] rather than state cached on
/// [`App`]. Recording happens on a blocking thread; transcription uses the
/// shared async HTTP client. Every failure path returns a localized message
/// so callers can surface it as a status line.
pub async fn capture_and_transcribe(
    app: &mut App,
    config: &Config,
) -> Result<VoiceCaptureOutcome, String> {
    let locale = app.ui_locale;

    if !is_available() {
        return Err(tr(locale, MessageId::VoiceErrNoRecorder).to_string());
    }
    let api_key = config
        .deepseek_api_key()
        .map_err(|_| tr(locale, MessageId::VoiceErrNoAuth).to_string())?;
    let base_url = config.deepseek_base_url();

    app.status_message = Some(tr(locale, MessageId::VoiceRecording).to_string());
    let (samples, _duration) = tokio::task::spawn_blocking(record_audio)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| tr(locale, MessageId::VoiceErrTooShort).to_string())?;

    app.status_message = Some(tr(locale, MessageId::VoiceProcessing).to_string());
    let text = if app.voice_control_enabled {
        process_voice_control(&api_key, &base_url, &samples, &app.composer.input).await
    } else {
        transcribe(&api_key, &base_url, &samples).await
    }
    .map_err(|e| format!("{}: {e}", tr(locale, MessageId::VoiceErrNetwork)))?;

    let clean = text.trim();
    if app.voice_send_enabled {
        let (remainder, wants_send) = split_send_suffix(clean);
        if wants_send {
            // A bare "send it" submits whatever is already in the composer.
            let outgoing = if remainder.is_empty() {
                let existing = app.composer.input.trim().to_string();
                if !existing.is_empty() {
                    app.clear_input();
                }
                existing
            } else {
                remainder.to_string()
            };
            if outgoing.is_empty() {
                return Err(tr(locale, MessageId::VoiceErrEmptySend).to_string());
            }
            return Ok(VoiceCaptureOutcome::Send(outgoing));
        }
    }
    if clean.is_empty() {
        return Err(tr(locale, MessageId::VoiceErrEmptySend).to_string());
    }
    Ok(VoiceCaptureOutcome::Insert(clean.to_string()))
}

// --- Command handlers ------------------------------------------------------

/// Handle the `/voice` command: toggle voice input. Toggling on requests a
/// one-shot recording + transcription via [`AppAction::VoiceCapture`].
pub fn voice(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;

    if app.voice_enabled {
        app.voice_enabled = false;
        return CommandResult::message(tr(locale, MessageId::VoiceDisabled));
    }
    if !is_available() {
        return CommandResult::error(tr(locale, MessageId::VoiceErrNoRecorder));
    }
    app.voice_enabled = true;
    CommandResult::with_message_and_action(
        tr(locale, MessageId::VoiceEnabled),
        AppAction::VoiceCapture,
    )
}

/// Handle the `/voice-send` command: toggle auto-send after transcription.
pub fn voice_send(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    app.voice_send_enabled = !app.voice_send_enabled;

    let msg = if app.voice_send_enabled {
        tr(locale, MessageId::VoiceSendEnabled)
    } else {
        tr(locale, MessageId::VoiceSendDisabled)
    };
    CommandResult::message(msg)
}

/// Handle the `/voice-control` command: toggle AI-assisted dictation.
pub fn voice_control(app: &mut App) -> CommandResult {
    let locale = app.ui_locale;
    app.voice_control_enabled = !app.voice_control_enabled;

    let msg = if app.voice_control_enabled {
        tr(locale, MessageId::VoiceControlEnabled)
    } else {
        tr(locale, MessageId::VoiceControlDisabled)
    };
    CommandResult::message(msg)
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
    fn send_suffix_detected_and_stripped() {
        assert_eq!(split_send_suffix("send it"), ("", true));
        assert_eq!(split_send_suffix("Send It!"), ("", true));
        assert_eq!(split_send_suffix("发送"), ("", true));
        assert_eq!(split_send_suffix("發送。"), ("", true));
        assert_eq!(
            split_send_suffix("ship the fix, send it"),
            ("ship the fix", true)
        );
        assert_eq!(
            split_send_suffix("修复这个问题，发送"),
            ("修复这个问题", true)
        );
    }

    #[test]
    fn send_suffix_leaves_plain_text_alone() {
        assert_eq!(split_send_suffix("send it now"), ("send it now", false));
        assert_eq!(
            split_send_suffix("帮我发送一封邮件"),
            ("帮我发送一封邮件", false)
        );
        assert_eq!(split_send_suffix("发送邮件"), ("发送邮件", false));
        assert_eq!(
            split_send_suffix("resend it to the queue"),
            ("resend it to the queue", false)
        );
    }

    #[test]
    fn recorder_detection_does_not_crash() {
        // Just verify the function runs without panicking
        let _ = is_available();
    }
}
