//! Clipboard handling for paste support in TUI
//!
//! Supports text and image paste operations. Images on the clipboard are
//! encoded as PNG and persisted under `~/.codewhale/clipboard-images/` so the
//! model can reach them via the existing `@`-mention / file tools (DeepSeek
//! V4 does not currently accept inline image input on its Chat Completions
//! endpoint, so we materialize the bytes to disk instead of base64-embedding
//! them in the request).

#[cfg(not(test))]
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
#[cfg(any(
    all(test, unix),
    all(not(test), target_os = "macos"),
    all(not(test), target_os = "windows"),
    all(not(test), target_os = "linux", not(target_env = "ohos"))
))]
use std::process::{Command, Stdio};
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use arboard::{Clipboard, ImageData};
use base64::Engine as _;
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use image::{ImageBuffer, Rgba};

const OSC52_MAX_BYTES: usize = 100 * 1024;

// === Types ===

/// Metadata captured for a pasted clipboard image. Used by the composer to
/// render a status hint like `Pasted 1024x768 image (235KB) → <path>`.
#[derive(Clone)]
pub struct PastedImage {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub byte_len: usize,
}

impl PastedImage {
    /// Short human-readable summary, e.g. `1024x768 PNG`.
    pub fn short_label(&self) -> String {
        format!("{}x{} PNG", self.width, self.height)
    }

    /// Approximate file size suffix, e.g. `235KB`.
    pub fn size_label(&self) -> String {
        let kb = (self.byte_len as f64 / 1024.0).round() as u64;
        format!("{kb}KB")
    }
}

/// Clipboard payloads supported by the TUI.
#[cfg_attr(all(target_env = "ohos", not(test)), allow(dead_code))]
pub enum ClipboardContent {
    Text(String),
    Image(PastedImage),
}

/// Clipboard reader/writer helper.
pub struct ClipboardHandler {
    #[cfg(any(
        test,
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    clipboard: Option<Clipboard>,
    #[cfg(any(
        test,
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    clipboard_init_attempted: bool,
    #[cfg(test)]
    written_text: Vec<String>,
}

impl ClipboardHandler {
    /// Create a new clipboard handler without connecting.
    ///
    /// The actual clipboard connection is deferred to first use
    /// (`ensure_clipboard`) so that startup on hosts without an X11/Wayland
    /// server (headless, WSL2) never blocks the TUI event loop.
    pub fn new() -> Self {
        Self {
            #[cfg(any(
                test,
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            clipboard: None,
            #[cfg(any(
                test,
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            clipboard_init_attempted: false,
            #[cfg(test)]
            written_text: Vec::new(),
        }
    }

    /// Try to connect to the system clipboard, bounded by a short timeout.
    ///
    /// On Linux, `arboard::Clipboard::new()` opens a blocking X11 connection.
    /// When no X server is running (headless, WSL2 without WSLg), the connect
    /// call can hang indefinitely. We spawn the connection attempt on a
    /// temporary thread and give it 500 ms; if it doesn't return in time the
    /// handler stays in fallback/no-op mode and `read`/`write_text` fall
    /// through to their OSC 52 and pbcopy/powershell fallbacks.
    #[cfg(any(
        test,
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    fn ensure_clipboard(&mut self) {
        if self.clipboard_init_attempted {
            return;
        }
        self.clipboard_init_attempted = true;

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Clipboard::new().ok());
        });
        self.clipboard = rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .ok()
            .flatten();
    }

    /// Read the clipboard and return the parsed content.
    ///
    /// `workspace` is used as a fallback location when `~/.codewhale/` cannot
    /// be resolved (e.g. running with a stripped HOME in CI sandboxes).
    pub fn read(&mut self, workspace: &Path) -> Option<ClipboardContent> {
        #[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
        if let Ok(text) = read_text_with_wlpaste() {
            return Some(ClipboardContent::Text(text));
        }

        #[cfg(any(
            test,
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"))
        ))]
        {
            self.ensure_clipboard();
            let clipboard = self.clipboard.as_mut()?;
            if let Ok(text) = clipboard.get_text() {
                return Some(ClipboardContent::Text(text));
            }

            if let Ok(image) = clipboard.get_image()
                && let Ok(pasted) = save_image_as_png(workspace, &image)
            {
                return Some(ClipboardContent::Image(pasted));
            }
        }

        let _ = workspace;
        None
    }

    /// Write text to the clipboard (no-op if unavailable).
    pub fn write_text(&mut self, text: &str) -> Result<()> {
        #[cfg(test)]
        {
            self.written_text.push(text.to_string());
            Ok(())
        }

        #[cfg(not(test))]
        {
            #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
            if write_text_with_wlcopy(text).is_ok() {
                return Ok(());
            }

            #[cfg(any(
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            {
                self.ensure_clipboard();
                if let Some(clipboard) = self.clipboard.as_mut()
                    && clipboard.set_text(text.to_string()).is_ok()
                {
                    return Ok(());
                }
            }

            #[cfg(target_os = "macos")]
            if write_text_with_pbcopy(text).is_ok() {
                return Ok(());
            }

            #[cfg(target_os = "windows")]
            if write_text_with_set_clipboard(text).is_ok() {
                return Ok(());
            }

            write_text_with_osc52(text)
                .map_err(|err| anyhow::anyhow!("Clipboard unavailable: {err}"))
        }
    }

    #[cfg(test)]
    pub fn last_written_text(&self) -> Option<&str> {
        self.written_text.last().map(String::as_str)
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn write_text_with_pbcopy(text: &str) -> Result<()> {
    write_text_with_stdin_command("pbcopy", &[], text, "pbcopy")
}

#[cfg(all(target_os = "windows", not(test)))]
fn write_text_with_set_clipboard(text: &str) -> Result<()> {
    write_text_with_stdin_command(
        "powershell.exe",
        &["-NoProfile", "-Command", "Set-Clipboard -Value $input"],
        text,
        "Set-Clipboard",
    )
}

#[cfg(all(any(target_os = "macos", target_os = "windows"), not(test)))]
fn write_text_with_stdin_command(
    program: &str,
    args: &[&str],
    text: &str,
    label: &str,
) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run {label}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write to {label}: {e}"))?;
    }
    let _ = std::thread::Builder::new()
        .name("clipboard-wait".to_string())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

#[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
fn write_text_with_wlcopy(text: &str) -> Result<()> {
    write_text_with_wlcopy_using_argv("wl-copy", text)
}

#[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
fn read_text_with_wlpaste() -> Result<String> {
    read_text_with_wlpaste_using_argv("wl-paste")
}

#[cfg(any(all(test, unix), all(target_os = "linux", not(target_env = "ohos"))))]
fn read_text_with_wlpaste_using_argv(program: &str) -> Result<String> {
    let output = Command::new(program)
        .arg("--no-newline")
        .arg("--type")
        .arg("text/plain")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run {program}: {e}"))?;
    if !output.status.success() {
        bail!("{program} exited with {}", output.status);
    }
    String::from_utf8(output.stdout).context("wl-paste returned non-UTF-8 text")
}

#[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
fn write_text_with_wlcopy_using_argv(program: &str, text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run {program}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write to {program}: {e}"))?;
    }
    // stdin is dropped here, closing the pipe so wl-copy flushes.
    let status = child
        .wait()
        .map_err(|e| anyhow::anyhow!("Failed to wait on {program}: {e}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

#[cfg(not(test))]
fn write_text_with_osc52(text: &str) -> Result<()> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        bail!("OSC 52 clipboard fallback requires a terminal");
    }

    let in_tmux = std::env::var_os("TMUX").is_some();
    let sequence = osc52_sequence(text, in_tmux)?;
    stdout
        .write_all(sequence.as_bytes())
        .context("write OSC 52 clipboard sequence")?;
    stdout.flush().context("flush OSC 52 clipboard sequence")
}

fn osc52_sequence(text: &str, in_tmux: bool) -> Result<String> {
    if text.len() > OSC52_MAX_BYTES {
        bail!("selection is too large for OSC 52 clipboard fallback");
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    if in_tmux {
        return Ok(format!("\x1bPtmux;\x1b{sequence}\x1b\\"));
    }
    Ok(sequence)
}

/// Resolve the directory pasted images should land in. Prefers
/// `~/.codewhale/clipboard-images/` so the path is stable across worktrees and
/// matches the location described in user-facing docs; falls back to
/// `<workspace>/clipboard-images/` if the home dir is unavailable.
pub(crate) fn clipboard_images_dir(workspace: &Path) -> PathBuf {
    let home = dirs::home_dir();
    clipboard_images_dir_for_home(workspace, home.as_deref())
}

fn clipboard_images_dir_for_home(workspace: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        return home.join(".codewhale").join("clipboard-images");
    }
    workspace.join("clipboard-images")
}

/// Encode an RGBA `ImageData` from arboard as PNG and persist it. Returns
/// the resulting path along with metadata used to render the paste hint.
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
fn save_image_as_png(workspace: &Path, image: &ImageData) -> Result<PastedImage> {
    save_image_as_png_in(&clipboard_images_dir(workspace), image)
}

/// Lower-level variant that writes into an explicit directory. Exposed so the
/// unit tests don't have to scribble inside the user's real home directory.
#[cfg(any(
    test,
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
fn save_image_as_png_in(dir: &Path, image: &ImageData) -> Result<PastedImage> {
    std::fs::create_dir_all(dir).context("create clipboard-images dir")?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("clipboard-{timestamp}.png"));

    let width = u32::try_from(image.width).context("clipboard image width too large")?;
    let height = u32::try_from(image.height).context("clipboard image height too large")?;

    // arboard hands us RGBA8 row-major. Copy into an ImageBuffer so we can
    // run it through the `image` crate's PNG encoder. We pad / truncate any
    // mismatched trailing bytes — defensive only, arboard already validates
    // the buffer length on every supported backend.
    let expected = (width as usize) * (height as usize) * 4;
    let mut rgba = image.bytes.as_ref().to_vec();
    if rgba.len() < expected {
        rgba.resize(expected, 0);
    } else if rgba.len() > expected {
        rgba.truncate(expected);
    }

    let buffer: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(width, height, rgba)
        .context("clipboard image dimensions did not match buffer length")?;
    buffer
        .save_with_format(&path, image::ImageFormat::Png)
        .context("write clipboard PNG")?;

    let byte_len = std::fs::metadata(&path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    Ok(PastedImage {
        path,
        width,
        height,
        byte_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn solid_rgba(width: u16, height: u16, rgba: [u8; 4]) -> ImageData<'static> {
        let mut bytes = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width as usize * height as usize) {
            bytes.extend_from_slice(&rgba);
        }
        ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(bytes),
        }
    }

    #[test]
    fn save_image_as_png_writes_valid_png() {
        let dir = tempfile::tempdir().unwrap();
        let img = solid_rgba(8, 4, [255, 0, 0, 255]);
        let pasted = save_image_as_png_in(dir.path(), &img).expect("encode png");

        assert_eq!(pasted.width, 8);
        assert_eq!(pasted.height, 4);
        assert!(pasted.byte_len > 0);
        assert_eq!(
            pasted.path.extension().and_then(|s| s.to_str()),
            Some("png")
        );

        // The first eight bytes of any PNG file are the magic signature; if
        // we ever regress to PPM or another format this will catch it.
        let header = std::fs::read(&pasted.path).unwrap();
        assert_eq!(&header[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn clipboard_images_dir_uses_codewhale_home_directory() {
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();

        assert_eq!(
            clipboard_images_dir_for_home(workspace.path(), Some(home.path())),
            home.path().join(".codewhale").join("clipboard-images")
        );
    }

    #[test]
    fn clipboard_images_dir_falls_back_to_workspace_without_home() {
        let workspace = tempfile::tempdir().unwrap();

        assert_eq!(
            clipboard_images_dir_for_home(workspace.path(), None),
            workspace.path().join("clipboard-images")
        );
    }

    #[test]
    fn pasted_image_labels_format_correctly() {
        let p = PastedImage {
            path: PathBuf::from("/tmp/x.png"),
            width: 1024,
            height: 768,
            byte_len: 235 * 1024,
        };
        assert_eq!(p.short_label(), "1024x768 PNG");
        assert_eq!(p.size_label(), "235KB");
    }

    #[test]
    fn osc52_sequence_encodes_text_clipboard_write() {
        let sequence = osc52_sequence("hello", false).expect("sequence");
        assert_eq!(sequence, "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn osc52_sequence_wraps_for_tmux_passthrough() {
        let sequence = osc52_sequence("copy", true).expect("sequence");
        assert_eq!(sequence, "\x1bPtmux;\x1b\x1b]52;c;Y29weQ==\x07\x1b\\");
    }

    #[test]
    fn osc52_sequence_rejects_oversized_selection() {
        let text = "x".repeat(OSC52_MAX_BYTES + 1);
        let err = osc52_sequence(&text, false).expect_err("oversized should fail");
        assert!(
            err.to_string().contains("too large"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wl_paste_helper_reads_text_from_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("wl-paste");
        std::fs::write(
            &script,
            r#"#!/bin/sh
seen_no_newline=0
seen_text_plain=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-newline) seen_no_newline=1 ;;
    --type)
      shift
      [ "${1:-}" = "text/plain" ] && seen_text_plain=1
      ;;
  esac
  shift
done
[ "$seen_text_plain" -eq 1 ] || exit 40
if [ "$seen_no_newline" -eq 1 ]; then
  printf 'from-wayland'
else
  printf 'from-wayland\n'
fi
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let text = read_text_with_wlpaste_using_argv(script.to_str().unwrap())
            .expect("read text through wl-paste helper");

        assert_eq!(text, "from-wayland");
    }
}
