//! Convenience `codew` alias.
//!
//! Forwards argv to the `codewhale` dispatcher silently. This is a
//! permanent short-form alias — six fewer keystrokes, same binary.

use std::env;
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    let status = match spawn_codewhale(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "error: failed to spawn `codewhale`: {e}. Is it on PATH? \
                 Install with `cargo install codewhale-cli` or via npm/Homebrew."
            );
            std::process::exit(127);
        }
    };
    std::process::exit(status.code().unwrap_or(1));
}

fn spawn_codewhale(args: &[String]) -> std::io::Result<std::process::ExitStatus> {
    // Try PATH first.
    match Command::new("codewhale").args(args).status() {
        Ok(s) => return Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    // On Windows, after an update the sibling `codewhale.exe` may be in the
    // same directory as this shim but not on PATH (#2006).
    #[cfg(windows)]
    {
        if let Ok(exe_path) = env::current_exe() {
            if let Some(dir) = exe_path.parent() {
                let sibling = dir.join("codewhale.exe");
                if sibling.is_file() {
                    return Command::new(sibling).args(args).status();
                }
            }
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "codewhale not found on PATH or in sibling directory",
    ))
}
