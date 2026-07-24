//! Clipboard handling for paste support in TUI
//!
//! Supports text and image paste operations. Images on the clipboard are
//! encoded as PNG and persisted under `~/.codewhale/clipboard-images/` so the
//! model can reach them via the existing `@`-mention / file tools (DeepSeek
//! V4 does not currently accept inline image input on its Chat Completions
//! endpoint, so we materialize the bytes to disk instead of base64-embedding
//! them in the request).

use std::ffi::OsStr;
#[cfg(any(not(test), all(test, unix)))]
use std::io::Write;
#[cfg(not(test))]
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
#[cfg(any(not(test), all(test, unix)))]
use std::process::{Command, Stdio};
#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use arboard::{Clipboard, ImageData};
use base64::Engine as _;
#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
use image::{ImageBuffer, Rgba};

const OSC52_MAX_BYTES: usize = 100 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardEndpoint {
    /// The TUI and desktop clipboard live on the same host.
    NativeHost,
    /// SSH exported a graphical display (X11 or Wayland), so the native
    /// clipboard intentionally addresses that forwarded display.
    ForwardedDisplay,
    /// No graphical endpoint is available over SSH. Clipboard transfer must
    /// be requested from the terminal client instead.
    TerminalClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipboardWriteOrder {
    /// An SSH TUI without an exported graphical display must target the
    /// terminal client. A native clipboard on the remote host can succeed
    /// while writing to the wrong machine.
    TerminalClientOnly,
    /// A local TUI should prefer the native clipboard (including images) and
    /// retain OSC 52 as the terminal fallback.
    NativeHostThenTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalClipboardContext {
    endpoint: ClipboardEndpoint,
    in_tmux: bool,
}

impl TerminalClipboardContext {
    fn detect() -> Self {
        let ssh_client = std::env::var_os("SSH_CLIENT");
        let ssh_connection = std::env::var_os("SSH_CONNECTION");
        let ssh_tty = std::env::var_os("SSH_TTY");
        let display = std::env::var_os("DISPLAY");
        let wayland_display = std::env::var_os("WAYLAND_DISPLAY");
        let ssh_clipboard = std::env::var_os("CODEWHALE_SSH_CLIPBOARD");
        let tmux = std::env::var_os("TMUX");
        Self::from_env_values(
            ssh_client.as_deref(),
            ssh_connection.as_deref(),
            ssh_tty.as_deref(),
            display.as_deref(),
            wayland_display.as_deref(),
            ssh_clipboard.as_deref(),
            tmux.as_deref(),
        )
    }

    fn from_env_values(
        ssh_client: Option<&OsStr>,
        ssh_connection: Option<&OsStr>,
        ssh_tty: Option<&OsStr>,
        display: Option<&OsStr>,
        wayland_display: Option<&OsStr>,
        ssh_clipboard: Option<&OsStr>,
        tmux: Option<&OsStr>,
    ) -> Self {
        let in_ssh_session = [ssh_client, ssh_connection, ssh_tty]
            .into_iter()
            .flatten()
            .any(|value| !value.is_empty());
        let has_graphical_display = [display, wayland_display]
            .into_iter()
            .flatten()
            .any(|value| !value.is_empty());
        let forwarded_x11 = display.and_then(OsStr::to_str).is_some_and(|value| {
            ["localhost:", "127.0.0.1:", "[::1]:", "::1:"]
                .iter()
                .any(|prefix| value.starts_with(prefix))
        });
        let use_graphical_display = match ssh_clipboard.and_then(OsStr::to_str) {
            Some("graphical") => has_graphical_display,
            Some("terminal") => false,
            _ => forwarded_x11,
        };

        Self {
            // OpenSSH normally exports SSH_CLIENT and SSH_CONNECTION.
            // SSH_TTY is an additional PTY-only marker and is independently
            // sufficient when wrappers preserve it without the other two.
            endpoint: match (in_ssh_session, use_graphical_display) {
                (false, _) => ClipboardEndpoint::NativeHost,
                (true, true) => ClipboardEndpoint::ForwardedDisplay,
                (true, false) => ClipboardEndpoint::TerminalClient,
            },
            in_tmux: tmux.is_some_and(|value| !value.is_empty()),
        }
    }

    fn write_order(self) -> ClipboardWriteOrder {
        if self.endpoint == ClipboardEndpoint::TerminalClient {
            ClipboardWriteOrder::TerminalClientOnly
        } else {
            ClipboardWriteOrder::NativeHostThenTerminal
        }
    }

    fn permits_native_read(self) -> bool {
        self.endpoint != ClipboardEndpoint::TerminalClient
    }

    fn requires_terminal_paste(self) -> bool {
        self.endpoint == ClipboardEndpoint::TerminalClient
    }
}

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
#[cfg_attr(
    all(
        any(target_env = "ohos", target_os = "android", target_os = "netbsd"),
        not(test)
    ),
    allow(dead_code)
)]
pub enum ClipboardContent {
    Text(String),
    Image(PastedImage),
}

/// Clipboard reader/writer helper.
pub struct ClipboardHandler {
    terminal_context: TerminalClipboardContext,
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    clipboard: Option<Clipboard>,
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    clipboard_init_attempted: bool,
    #[cfg(test)]
    written_text: Vec<String>,
    #[cfg(test)]
    fail_text_writes: bool,
}

impl ClipboardHandler {
    /// Create a new clipboard handler without connecting.
    ///
    /// The actual clipboard connection is deferred to first use
    /// (`ensure_clipboard`) so that startup on hosts without an X11/Wayland
    /// server (headless, WSL2) never blocks the TUI event loop.
    pub fn new() -> Self {
        Self::with_terminal_context(TerminalClipboardContext::detect())
    }

    fn with_terminal_context(terminal_context: TerminalClipboardContext) -> Self {
        Self {
            terminal_context,
            #[cfg(any(
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            clipboard: None,
            #[cfg(any(
                target_os = "macos",
                target_os = "windows",
                all(target_os = "linux", not(target_env = "ohos"))
            ))]
            clipboard_init_attempted: false,
            #[cfg(test)]
            written_text: Vec::new(),
            #[cfg(test)]
            fail_text_writes: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(in_ssh_session: bool, in_tmux: bool) -> Self {
        Self::with_terminal_context(TerminalClipboardContext {
            endpoint: if in_ssh_session {
                ClipboardEndpoint::TerminalClient
            } else {
                ClipboardEndpoint::NativeHost
            },
            in_tmux,
        })
    }

    /// Construct a deterministic unavailable clipboard for command tests.
    #[cfg(test)]
    pub(crate) fn unavailable_for_test(in_ssh_session: bool) -> Self {
        let mut handler = Self::for_test(in_ssh_session, false);
        handler.fail_text_writes = true;
        handler
    }

    /// SSH without a forwarded graphical display cannot synchronously read
    /// the terminal client's clipboard. Paste must be initiated by the local
    /// terminal so it arrives as bracketed paste (or a raw paste burst on
    /// older terminals).
    pub(crate) fn requires_terminal_paste(&self) -> bool {
        self.terminal_context.requires_terminal_paste()
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
        // With no display exported over SSH there is no synchronously readable
        // clipboard endpoint. A forwarded X11/Wayland display is explicit and
        // remains readable, including its image clipboard.
        if !self.terminal_context.permits_native_read() {
            return None;
        }

        #[cfg(all(target_os = "linux", not(target_env = "ohos"), not(test)))]
        if let Ok(text) = read_text_with_wlpaste() {
            return Some(ClipboardContent::Text(text));
        }

        #[cfg(any(
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
            if self.fail_text_writes {
                bail!("test clipboard unavailable");
            }
            self.written_text.push(text.to_string());
            Ok(())
        }

        #[cfg(not(test))]
        {
            if self.terminal_context.write_order() == ClipboardWriteOrder::TerminalClientOnly {
                return write_text_to_terminal_client(text, self.terminal_context.in_tmux)
                    .map_err(|err| anyhow::anyhow!("Clipboard unavailable: {err}"));
            }

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

            write_text_to_terminal_client(text, self.terminal_context.in_tmux)
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
fn write_text_to_terminal_client(text: &str, in_tmux: bool) -> Result<()> {
    if in_tmux {
        return write_text_with_tmux(text);
    }
    write_text_with_osc52(text)
}

#[cfg(not(test))]
fn write_text_with_tmux(text: &str) -> Result<()> {
    write_text_with_tmux_using_argv("tmux", &[], text)
}

/// Ask tmux to set both its paste buffer and the attached client's clipboard.
/// Unlike DCS passthrough, `load-buffer -w` works with tmux's default
/// `allow-passthrough off` policy and returns a non-zero status when tmux
/// cannot honor the command.
#[cfg(any(not(test), all(test, unix)))]
fn write_text_with_tmux_using_argv(program: &str, prefix_args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(prefix_args)
        .args(["load-buffer", "-w", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to run tmux load-buffer -w: {e}"))?;

    let write_result = child
        .stdin
        .take()
        .context("open tmux clipboard input")
        .and_then(|mut stdin| {
            stdin
                .write_all(text.as_bytes())
                .context("write tmux clipboard input")
        });
    let output = child
        .wait_with_output()
        .context("wait for tmux load-buffer -w")?;
    write_result?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        if detail.is_empty() {
            bail!("tmux load-buffer -w exited with {}", output.status);
        }
        bail!(
            "tmux load-buffer -w exited with {}: {detail}",
            output.status
        );
    }
    Ok(())
}

#[cfg(not(test))]
fn write_text_with_osc52(text: &str) -> Result<()> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        bail!("OSC 52 clipboard fallback requires a terminal");
    }

    let sequence = osc52_sequence(text)?;
    stdout
        .write_all(sequence.as_bytes())
        .context("write OSC 52 clipboard sequence")?;
    stdout.flush().context("flush OSC 52 clipboard sequence")
}

fn osc52_sequence(text: &str) -> Result<String> {
    if text.len() > OSC52_MAX_BYTES {
        bail!("selection is too large for OSC 52 clipboard fallback");
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    Ok(format!("\x1b]52;c;{encoded}\x07"))
}

/// Resolve the directory pasted images should land in. Prefers
/// `~/.codewhale/clipboard-images/` so the path is stable across worktrees and
/// matches the location described in user-facing docs; falls back to
/// `<workspace>/clipboard-images/` if the home dir is unavailable.
pub(crate) fn clipboard_images_dir(workspace: &Path) -> PathBuf {
    let home = crate::config::effective_home_dir();
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
    // ImageData from arboard is only available on these platforms.
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    use std::borrow::Cow;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
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
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
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
    fn ssh_detection_covers_openssh_markers_and_ignores_empty_values() {
        let client = TerminalClipboardContext::from_env_values(
            Some(OsStr::new("192.0.2.10 51234 22")),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let connection = TerminalClipboardContext::from_env_values(
            None,
            Some(OsStr::new("192.0.2.10 51234 192.0.2.20 22")),
            None,
            None,
            None,
            None,
            None,
        );
        let tty = TerminalClipboardContext::from_env_values(
            None,
            None,
            Some(OsStr::new("/dev/pts/4")),
            None,
            None,
            None,
            None,
        );
        let empty = TerminalClipboardContext::from_env_values(
            Some(OsStr::new("")),
            Some(OsStr::new("")),
            Some(OsStr::new("")),
            Some(OsStr::new("")),
            Some(OsStr::new("")),
            Some(OsStr::new("")),
            Some(OsStr::new("")),
        );

        assert_eq!(client.endpoint, ClipboardEndpoint::TerminalClient);
        assert_eq!(connection.endpoint, ClipboardEndpoint::TerminalClient);
        assert_eq!(tty.endpoint, ClipboardEndpoint::TerminalClient);
        assert_eq!(empty.endpoint, ClipboardEndpoint::NativeHost);
        assert!(!empty.in_tmux);
    }

    #[test]
    fn ssh_without_display_targets_terminal_client() {
        let remote_tmux = TerminalClipboardContext::from_env_values(
            Some(OsStr::new("192.0.2.10 51234 22")),
            None,
            None,
            None,
            None,
            None,
            Some(OsStr::new("/tmp/tmux-1000/default,1,0")),
        );
        let local =
            TerminalClipboardContext::from_env_values(None, None, None, None, None, None, None);

        assert_eq!(
            remote_tmux.write_order(),
            ClipboardWriteOrder::TerminalClientOnly
        );
        assert!(!remote_tmux.permits_native_read());
        assert!(remote_tmux.requires_terminal_paste());
        assert!(remote_tmux.in_tmux);
        assert_eq!(
            local.write_order(),
            ClipboardWriteOrder::NativeHostThenTerminal
        );
        assert!(local.permits_native_read());
        assert!(!local.requires_terminal_paste());
    }

    #[test]
    fn ssh_uses_forwarded_x11_or_explicit_graphical_clipboard_endpoint() {
        let x11 = TerminalClipboardContext::from_env_values(
            None,
            Some(OsStr::new("192.0.2.10 51234 192.0.2.20 22")),
            None,
            Some(OsStr::new("localhost:10.0")),
            None,
            None,
            None,
        );
        let wayland = TerminalClipboardContext::from_env_values(
            Some(OsStr::new("192.0.2.10 51234 22")),
            None,
            None,
            None,
            Some(OsStr::new("wayland-1")),
            Some(OsStr::new("graphical")),
            None,
        );

        for context in [x11, wayland] {
            assert_eq!(context.endpoint, ClipboardEndpoint::ForwardedDisplay);
            assert_eq!(
                context.write_order(),
                ClipboardWriteOrder::NativeHostThenTerminal
            );
            assert!(context.permits_native_read());
            assert!(!context.requires_terminal_paste());
        }

        let ambient_remote = TerminalClipboardContext::from_env_values(
            Some(OsStr::new("192.0.2.10 51234 22")),
            None,
            None,
            Some(OsStr::new(":0")),
            Some(OsStr::new("wayland-0")),
            None,
            None,
        );
        assert_eq!(ambient_remote.endpoint, ClipboardEndpoint::TerminalClient);

        let forced_terminal = TerminalClipboardContext::from_env_values(
            Some(OsStr::new("192.0.2.10 51234 22")),
            None,
            None,
            Some(OsStr::new("localhost:10.0")),
            None,
            Some(OsStr::new("terminal")),
            None,
        );
        assert_eq!(forced_terminal.endpoint, ClipboardEndpoint::TerminalClient);
    }

    #[test]
    fn osc52_sequence_encodes_text_clipboard_write() {
        let sequence = osc52_sequence("hello").expect("sequence");
        assert_eq!(sequence, "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn osc52_sequence_rejects_oversized_selection() {
        let text = "x".repeat(OSC52_MAX_BYTES + 1);
        let err = osc52_sequence(&text).expect_err("oversized should fail");
        assert!(
            err.to_string().contains("too large"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn tmux_helper_reports_command_failure() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("tmux");
        std::fs::write(
            &script,
            r#"#!/bin/sh
cat >/dev/null
echo 'clipboard denied' >&2
exit 42
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let err = write_text_with_tmux_using_argv(script.to_str().unwrap(), &[], "copy")
            .expect_err("non-zero tmux status should fail");

        assert!(err.to_string().contains("exited with"));
        assert!(err.to_string().contains("clipboard denied"));
    }

    #[cfg(all(unix, not(target_env = "ohos")))]
    #[test]
    fn tmux_load_buffer_w_reaches_attached_client_with_default_passthrough_disabled() {
        use std::io::Read as _;

        let version = match Command::new("tmux").arg("-V").output() {
            Ok(output) if output.status.success() => output,
            _ => return,
        };
        assert!(
            String::from_utf8_lossy(&version.stdout).starts_with("tmux "),
            "unexpected tmux version output"
        );

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let socket = format!("codewhale-clipboard-{}-{nonce}", std::process::id());

        struct TmuxServer(String);
        impl Drop for TmuxServer {
            fn drop(&mut self) {
                let _ = Command::new("tmux")
                    .args(["-L", self.0.as_str(), "kill-server"])
                    .status();
            }
        }
        let server = TmuxServer(socket);
        let started = Command::new("tmux")
            .args([
                "-L",
                server.0.as_str(),
                "-f",
                "/dev/null",
                "new-session",
                "-d",
            ])
            .status()
            .expect("start isolated tmux server");
        assert!(started.success(), "isolated tmux server should start");

        let option = |name: &str| {
            let output = Command::new("tmux")
                .args(["-L", server.0.as_str(), "show-options", "-gv", name])
                .output()
                .expect("read tmux option");
            assert!(output.status.success(), "read tmux option {name}");
            String::from_utf8(output.stdout)
                .expect("tmux option should be utf-8")
                .trim()
                .to_string()
        };
        assert_eq!(option("allow-passthrough"), "off");
        assert_eq!(option("set-clipboard"), "external");

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open attached-client PTY");
        let mut attach = portable_pty::CommandBuilder::new("tmux");
        for arg in ["-L", server.0.as_str(), "attach-session", "-t", "0"] {
            attach.arg(arg);
        }
        attach.env("TERM", "xterm-256color");
        let mut attached_client = pair
            .slave
            .spawn_command(attach)
            .expect("attach tmux client to PTY");
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("clone attached-client PTY reader");
        let (output_tx, output_rx) = std::sync::mpsc::channel();
        let reader_thread = std::thread::spawn(move || {
            let mut chunk = [0_u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(len) => {
                        if output_tx.send(chunk[..len].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let attach_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let clients = Command::new("tmux")
                .args(["-L", server.0.as_str(), "list-clients"])
                .output()
                .expect("list attached tmux clients");
            if clients.status.success() && !clients.stdout.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < attach_deadline,
                "tmux client did not attach to the test PTY"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        while output_rx.try_recv().is_ok() {}

        let copied_text = "copy through default tmux";
        write_text_with_tmux_using_argv("tmux", &["-L", server.0.as_str()], copied_text)
            .expect("tmux-native clipboard request");

        let encoded = base64::engine::general_purpose::STANDARD.encode(copied_text.as_bytes());
        let expected_receipts = [
            format!("\x1b]52;;{encoded}\x07").into_bytes(),
            format!("\x1b]52;c;{encoded}\x07").into_bytes(),
            format!("\x1b]52;;{encoded}\x1b\\").into_bytes(),
            format!("\x1b]52;c;{encoded}\x1b\\").into_bytes(),
        ];
        let receipt_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut attached_output = Vec::new();
        let receipt_received = loop {
            if expected_receipts.iter().any(|receipt| {
                attached_output
                    .windows(receipt.len())
                    .any(|window| window == receipt)
            }) {
                break true;
            }
            if std::time::Instant::now() >= receipt_deadline {
                break false;
            }
            match output_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(bytes) => attached_output.extend_from_slice(&bytes),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break false,
            }
        };

        let buffer = Command::new("tmux")
            .args(["-L", server.0.as_str(), "show-buffer"])
            .output()
            .expect("read tmux buffer");
        assert!(buffer.status.success(), "tmux buffer should be readable");
        assert_eq!(buffer.stdout, copied_text.as_bytes());

        let _ = attached_client.kill();
        let _ = attached_client.wait();
        drop(pair.master);
        drop(output_rx);
        let _ = reader_thread.join();

        assert!(
            receipt_received,
            "attached tmux client did not receive the OSC 52 clipboard request: {attached_output:?}"
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
