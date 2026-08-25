//! Child-process helpers shared by the process-based drivers.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::driver::DriverError;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// Run a command with a wall-clock timeout, capturing stdout and stderr.
pub fn run(program: &Path, args: &[&str], timeout: Duration) -> Result<Output, DriverError> {
    run_with_env(program, args, &[], timeout)
}

pub fn run_with_env(
    program: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output, DriverError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| {
        DriverError::Unavailable(format!("failed to start {}: {e}", program.display()))
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut s) = stdout {
            let _ = std::io::Read::read_to_end(&mut s, &mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut s) = stderr {
            let _ = std::io::Read::read_to_end(&mut s, &mut buf);
        }
        buf
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DriverError::Failed(format!(
                        "{} {} timed out after {}s",
                        program.display(),
                        args.join(" "),
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                return Err(DriverError::Failed(format!(
                    "failed waiting for {}: {e}",
                    program.display()
                )));
            }
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Run and require exit status 0; the error carries a trimmed stderr tail.
pub fn run_ok(program: &Path, args: &[&str], timeout: Duration) -> Result<Output, DriverError> {
    let out = run(program, args, timeout)?;
    if out.success() {
        Ok(out)
    } else {
        Err(DriverError::Failed(format!(
            "{} {} failed (status {:?}): {}",
            program
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("command"),
            args.join(" "),
            out.status,
            tail(&out.stderr, 400)
        )))
    }
}

pub fn tail(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let skip = trimmed.chars().count() - max;
        format!("…{}", trimmed.chars().skip(skip).collect::<String>())
    }
}

fn executable_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    }
}

/// Look up `name` on PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for candidate in executable_names(name) {
            let full = dir.join(&candidate);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

/// Find a helper binary: explicit path, then PATH, then well-known
/// directories (SDK installs) since plugin children see a scrubbed env.
pub fn find_binary(explicit: &str, name: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    if !explicit.trim().is_empty() {
        let p = PathBuf::from(explicit.trim());
        return if p.is_file() { Some(p) } else { None };
    }
    if let Some(found) = which(name) {
        return Some(found);
    }
    for dir in extra_dirs {
        for candidate in executable_names(name) {
            let full = dir.join(&candidate);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

pub fn home() -> Option<PathBuf> {
    crate::config::home_dir()
}

/// Quote for POSIX `sh` (what `adb shell` / `hdc shell` hand the string to).
pub fn sh_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | ','))
    {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quote_escapes_single_quotes_and_spaces() {
        assert_eq!(sh_quote("abc"), "abc");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn tail_keeps_last_chars() {
        assert_eq!(tail("hello world", 5), "…world");
        assert_eq!(tail("  hi  ", 5), "hi");
    }

    #[cfg(unix)]
    #[test]
    fn run_captures_output_and_times_out() {
        let sh = which("sh").expect("sh on PATH");
        let out = run(
            &sh,
            &["-c", "printf hi; echo err >&2; exit 3"],
            DEFAULT_TIMEOUT,
        )
        .unwrap();
        assert_eq!(out.status, Some(3));
        assert_eq!(out.stdout_text(), "hi");
        assert_eq!(out.stderr.trim(), "err");
        let err = run(&sh, &["-c", "sleep 5"], Duration::from_millis(200)).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }
}
