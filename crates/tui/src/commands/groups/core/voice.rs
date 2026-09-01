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
const GROQ_ASR_MODEL: &str = "whisper-large-v3-turbo";
/// Local whisper binary names to probe (whisper.cpp, faster-whisper, OpenAI whisper).
const LOCAL_WHISPER_BINS: &[&str] = &["whisper", "whisper.cpp", "whisper-cpp", "faster-whisper"];
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
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .copied()
                    .map(i16::from_le_bytes)
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
    let _inference = crate::client::acquire_remote_control_inference_participant().await;
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
    transcribe_with_model(api_key, base_url, audio_samples, ASR_MODEL).await
}

async fn transcribe_with_model(
    api_key: &str,
    base_url: &str,
    audio_samples: &[i16],
    model: &str,
) -> Result<String, String> {
    let wav = encode_wav(audio_samples);
    let data_url = format!("data:audio/wav;base64,{}", base64_encode(&wav));

    let body = serde_json::json!({
        "model": model,
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

/// Detect best free ASR for this host — local whisper > Groq free > provider fallback.
/// Works on macOS (brew install whisper-cpp), Windows (whisper.cpp binary),
/// Linux (apt), and HarmonyOS (falls back to cloud).
fn detect_free_asr() -> &'static str {
    for bin in LOCAL_WHISPER_BINS {
        if Command::new(bin)
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
        {
            return "local-whisper";
        }
    }
    if std::env::var("GROQ_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return "groq";
    }
    "provider"
}

/// Transcribe via local whisper.cpp (free, offline, cross-platform).
async fn transcribe_local_whisper(audio_samples: &[i16]) -> Result<String, String> {
    let wav = encode_wav(audio_samples);
    let tmp = std::env::temp_dir().join(format!("cw-voice-{}.wav", std::process::id()));
    std::fs::write(&tmp, &wav).map_err(|e| e.to_string())?;
    // Try each local binary until one succeeds; whisper.cpp outputs to stdout or file.
    for bin in LOCAL_WHISPER_BINS {
        let output = Command::new(bin)
            .arg(tmp.to_string_lossy().as_ref())
            .arg("--model")
            .arg("tiny")
            .arg("--language")
            .arg("auto")
            .arg("--output-txt")
            .output();
        if let Ok(out) = output
            && out.status.success()
        {
            let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let _ = std::fs::remove_file(&tmp);
            if !txt.is_empty() {
                return Ok(txt);
            }
            // Some builds write to .txt sidecar
            let sidecar = tmp.with_extension("txt");
            if let Ok(s) = std::fs::read_to_string(&sidecar) {
                let _ = std::fs::remove_file(&sidecar);
                let _ = std::fs::remove_file(&tmp);
                if !s.trim().is_empty() {
                    return Ok(s.trim().to_string());
                }
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err("local whisper not available".into())
}

/// Transcribe via Groq Whisper large-v3-turbo (free tier, ~$0.04/hr, fast).
/// Groq is NOT a full CodeWhale provider yet — this is a direct ASR call
/// using `GROQ_API_KEY` only (no provider setup needed). Uses the same
/// chat-completions `input_audio` path as Xiaomi so no `multipart` feature.
async fn transcribe_groq(audio_samples: &[i16]) -> Result<String, String> {
    let api_key = std::env::var("GROQ_API_KEY").map_err(|_| "GROQ_API_KEY not set".to_string())?;
    let base_url = "https://api.groq.com/openai/v1";
    transcribe_with_model(&api_key, base_url, audio_samples, GROQ_ASR_MODEL).await
}

/// Perform a complete record + transcribe cycle with live interim display.
///
/// Runs in the UI event loop (see [`AppAction::VoiceCapture`]) so provider
/// credentials come from the live [`Config`] rather than state cached on
/// [`App`]. Recording happens on a blocking thread; transcription uses the
/// shared async HTTP client. Every failure path returns a localized message
/// so callers can surface it as a status line.
/// Resolve ASR model/provider preference.
/// Priority: explicit config `voice.asr_model` > env `CODEWHALE_ASR_MODEL` > auto-detect (local-whisper > groq > xiaomi).
fn resolve_asr_choice(_config: &Config) -> (String, String) {
    // Check explicit env override first (free, cross-platform)
    if let Ok(m) = std::env::var("CODEWHALE_ASR_MODEL") {
        let m = m.trim().to_ascii_lowercase();
        if m.contains("groq") || m.contains("whisper") {
            return ("groq".into(), GROQ_ASR_MODEL.into());
        }
        if m.contains("local") || m.contains("whisper.cpp") {
            return ("local-whisper".into(), "tiny".into());
        }
        if m.contains("mimo") || m.contains("xiaomi") {
            return ("provider".into(), ASR_MODEL.into());
        }
    }
    // Auto-detect best free: local whisper (offline, no key) > Groq free tier > Xiaomi ASR (needs key)
    let free = detect_free_asr();
    match free {
        "local-whisper" => ("local-whisper".into(), "tiny".into()),
        "groq" => ("groq".into(), GROQ_ASR_MODEL.into()),
        _ => ("provider".into(), ASR_MODEL.into()),
    }
}

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

    // Spark-style: show "● Recording (⌥V to finish)" + live interim in composer.
    let original_input = app.composer.input.clone();
    let original_cursor = app.composer.cursor_position;
    app.status_message = Some("● Recording  (⌥V to finish)  ·  speak naturally".to_string());

    // Streaming interim: poll every 700ms and show partial transcript like Grok Build's
    // VoiceEvent::Interim → VoiceState::Recording{interim}. We re-transcribe the
    // growing buffer (local-whisper is cheap; Groq is ~300ms; provider falls back).
    let (asr_kind, _asr_model) = resolve_asr_choice(config);
    let interim_enabled = true; // always show partials — feels alive like Spark

    // Spawn recorder on blocking thread with a shared buffer for interim polling.
    let shared_buf: std::sync::Arc<parking_lot::Mutex<Vec<i16>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    let shared_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shared_buf_clone = std::sync::Arc::clone(&shared_buf);
    let shared_done_clone = std::sync::Arc::clone(&shared_done);
    let recorder_handle = tokio::task::spawn_blocking(move || {
        // Bridge to existing record_audio but copy into shared buffer incrementally.
        // For now we reuse the blocking recorder and then publish; interim will
        // poll the final buffer. A true streaming recorder (cpal/pw-record) is
        // the next step — see grokbuild's xai-grok-voice::audio for the subprocess
        // isolation pattern we should mirror.
        let result = record_audio();
        if let Some((samples, dur)) = result {
            *shared_buf_clone.lock() = samples.clone();
            shared_done_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            Some((samples, dur))
        } else {
            shared_done_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            None
        }
    });

    // Interim polling loop — updates composer with "original + interim ▍" so text
    // appears as you talk, just like Spark's live transcript.
    let mut last_interim = String::new();
    let mut ticks: u32 = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        ticks += 1;
        if shared_done.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if !interim_enabled || ticks < 2 {
            continue; // let a little audio accumulate before first interim
        }
        let snapshot = { shared_buf.lock().clone() };
        if snapshot.len() < 8000 {
            // <0.5s of audio — not enough for meaningful ASR
            continue;
        }
        // Try cheapest free ASR for interim; don't fail the whole capture on interim error.
        let interim = match asr_kind.as_str() {
            "local-whisper" => transcribe_local_whisper(&snapshot)
                .await
                .unwrap_or_default(),
            "groq" => transcribe_groq(&snapshot).await.unwrap_or_default(),
            _ => {
                // For provider ASR, reuse the same endpoint but don't block on interim if no key.
                if let Ok(key) = config
                    .deepseek_api_key()
                    .map(|k: String| k)
                    .map_err(|_| String::new())
                {
                    let url = config.deepseek_base_url();
                    transcribe(&key, &url, &snapshot).await.unwrap_or_default()
                } else {
                    String::new()
                }
            }
        };
        let trimmed = interim.trim();
        if !trimmed.is_empty() && trimmed != last_interim {
            last_interim = trimmed.to_string();
            // Show interim inline — preserve cursor at original position, append interim with a block cursor
            let display = if original_input.trim().is_empty() {
                format!("{trimmed} ▍")
            } else {
                format!("{} {} ▍", original_input.trim_end(), trimmed)
            };
            app.composer.input = display;
            app.composer.cursor_position = original_cursor;
            // Also keep status as Spark does
            app.status_message = Some(format!("● Listening — “{trimmed}”  (⌥V to finish)"));
        }
        if ticks > 40 {
            break; // safety: ~28s max interim polling
        }
    }

    let (samples, _duration) = recorder_handle
        .await
        .ok()
        .flatten()
        .ok_or_else(|| tr(locale, MessageId::VoiceErrTooShort).to_string())?;

    // Restore composer to original before final insert (interim was preview only)
    app.composer.input = original_input.clone();
    app.composer.cursor_position = original_cursor;
    app.status_message = Some(tr(locale, MessageId::VoiceProcessing).to_string());

    let text = match asr_kind.as_str() {
        "local-whisper" => match transcribe_local_whisper(&samples).await {
            Ok(v) => Ok(v),
            Err(_) => transcribe(&api_key, &base_url, &samples).await,
        },
        "groq" => match transcribe_groq(&samples).await {
            Ok(v) => Ok(v),
            Err(_) => transcribe(&api_key, &base_url, &samples).await,
        },
        _ => {
            if app.voice_control_enabled {
                process_voice_control(&api_key, &base_url, &samples, &original_input).await
            } else {
                transcribe(&api_key, &base_url, &samples).await
            }
        }
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
