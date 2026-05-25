//! Voice-input command bridge for the composer.
//!
//! CodeWhale stays out of platform microphone APIs here. A configured command
//! owns recording and speech-to-text, writes the final transcript to stdout,
//! and the TUI inserts that transcript into the composer.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::process::Command as TokioCommand;

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 600;

pub(crate) fn clamp_timeout_secs(secs: u64) -> u64 {
    secs.clamp(1, MAX_TIMEOUT_SECS)
}

pub(crate) fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn parse_voice_command(command_line: &str) -> Result<(String, Vec<String>)> {
    let trimmed = command_line.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("voice_input_command is empty"));
    }

    let parts = shlex::split(trimmed).ok_or_else(|| {
        anyhow!("voice_input_command has invalid quoting; check spaces and quote pairs")
    })?;
    let Some((program, args)) = parts.split_first() else {
        return Err(anyhow!("voice_input_command is empty"));
    };
    Ok((program.clone(), args.to_vec()))
}

fn stdout_to_transcript(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let transcript = text.trim();
    (!transcript.is_empty()).then(|| transcript.to_string())
}

fn stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut summary: String = trimmed.chars().take(300).collect();
    if trimmed.chars().count() > 300 {
        summary.push_str("...");
    }
    format!(": {summary}")
}

pub(crate) async fn run_configured_voice_command(
    command_line: &str,
    timeout_secs: u64,
    cwd: &Path,
) -> Result<String> {
    let timeout_secs = clamp_timeout_secs(timeout_secs);
    let (program, args) = parse_voice_command(command_line)?;

    let mut command = TokioCommand::new(&program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), command.output())
        .await
        .map_err(|_| anyhow!("voice input command timed out after {timeout_secs}s"))?
        .with_context(|| format!("failed to run voice input command `{program}`"))?;

    if !output.status.success() {
        return Err(anyhow!(
            "voice input command exited with {}{}",
            output.status,
            stderr_summary(&output.stderr)
        ));
    }

    stdout_to_transcript(&output.stdout)
        .ok_or_else(|| anyhow!("voice input command produced no transcript on stdout"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_voice_command() {
        let (program, args) =
            parse_voice_command(r#"python3 "/tmp/codewhale voice.py" --lang en-US"#)
                .expect("parse command");
        assert_eq!(program, "python3");
        assert_eq!(args, vec!["/tmp/codewhale voice.py", "--lang", "en-US"]);
    }

    #[test]
    fn rejects_invalid_voice_command_quoting() {
        let err = parse_voice_command(r#"python3 "unterminated"#).expect_err("bad quotes");
        assert!(err.to_string().contains("invalid quoting"));
    }

    #[test]
    fn trims_stdout_to_transcript() {
        assert_eq!(
            stdout_to_transcript(b"\n  ship the voice input feature\r\n").as_deref(),
            Some("ship the voice input feature")
        );
        assert!(stdout_to_transcript(b"\n\t ").is_none());
    }

    #[test]
    fn timeout_clamps_to_supported_range() {
        assert_eq!(clamp_timeout_secs(0), 1);
        assert_eq!(clamp_timeout_secs(30), 30);
        assert_eq!(clamp_timeout_secs(999), MAX_TIMEOUT_SECS);
    }
}
