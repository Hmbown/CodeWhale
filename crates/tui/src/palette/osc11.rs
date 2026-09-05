//! OSC 11 terminal-background query.
//!
//! `COLORFGBG` is the only background signal the palette had before this
//! module, and most modern terminals never set it — Windows Terminal, conhost,
//! VS Code, GNOME Terminal, Alacritty and Ghostty all omit it. Without it a
//! white terminal was indistinguishable from a black one, so detection fell
//! back to `Dark` and painted dark-tuned text onto a light surface (#4833).
//!
//! OSC 11 (`ESC ] 11 ; ? BEL`) asks the terminal for its actual background
//! color and is answered by every terminal listed above. The reply is an
//! `xterm`-style color spec, e.g.
//!
//! ```text
//! ESC ] 11 ; rgb:ffff/ffff/ffff ESC \
//! ```
//!
//! The parse is a pure function so it can be tested without a terminal; the
//! query itself is Unix-only, bounded by a short deadline, and never runs when
//! stdin/stdout are not both TTYs.

/// Upper bound on how long startup will wait for a terminal that never
/// answers. A terminal that supports OSC 11 replies in well under a
/// millisecond; anything past this is a terminal that will never reply, and
/// startup latency matters more than the answer.
pub const OSC11_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(120);

/// The query sequence. `ESC \` (ST) is the terminator we prefer in the reply,
/// but terminals may answer with BEL instead, so the reader accepts both.
///
/// Only the Unix query path writes it — see the note on [`parse_osc11_reply`].
#[cfg_attr(not(unix), allow(dead_code))]
const OSC11_QUERY: &[u8] = b"\x1b]11;?\x1b\\";

/// Extract an RGB triple from an OSC 11 reply body.
///
/// Accepts the shapes terminals actually emit:
/// - `rgb:RRRR/GGGG/BBBB` (xterm, 1–4 hex digits per channel, any width)
/// - `#RRGGBB` / `#RGB` / `#RRRRGGGGBBBB`
///
/// Leading `ESC ] 11 ;` and the trailing BEL/ST are optional — anything
/// outside the color spec is ignored, so a reply that arrived interleaved with
/// other terminal chatter still parses.
///
/// Returns `None` when no color spec is present or a channel is malformed.
/// Channels wider than 8 bits are scaled down, not truncated, so `ffff` is
/// `255` rather than `0`.
// The parser is deliberately cross-platform while the query is Unix-only:
// there is no portable way to read a raw OSC reply off a Windows console
// handle yet, so on Windows nothing calls these. They are kept (rather than
// cfg'd out) because they are pure, fully tested on every platform, and are
// exactly what a future Windows read path would need — but that leaves them
// dead in a non-test Windows build, which `-D warnings` rejects.
#[cfg_attr(not(unix), allow(dead_code))]
#[must_use]
pub fn parse_osc11_reply(reply: &str) -> Option<(u8, u8, u8)> {
    if let Some(idx) = reply.find("rgb:") {
        return parse_slash_separated(&reply[idx + 4..]);
    }
    if let Some(idx) = reply.find('#') {
        return parse_hash_hex(&reply[idx + 1..]);
    }
    None
}

#[cfg_attr(not(unix), allow(dead_code))]
fn parse_slash_separated(spec: &str) -> Option<(u8, u8, u8)> {
    let spec: String = spec
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '/')
        .collect();
    let mut parts = spec.split('/');
    let r = scale_hex_channel(parts.next()?)?;
    let g = scale_hex_channel(parts.next()?)?;
    let b = scale_hex_channel(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((r, g, b))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn parse_hash_hex(spec: &str) -> Option<(u8, u8, u8)> {
    let digits: String = spec.chars().take_while(char::is_ascii_hexdigit).collect();
    if !digits.len().is_multiple_of(3) || digits.is_empty() || digits.len() > 12 {
        return None;
    }
    let width = digits.len() / 3;
    let r = scale_hex_channel(&digits[..width])?;
    let g = scale_hex_channel(&digits[width..width * 2])?;
    let b = scale_hex_channel(&digits[width * 2..])?;
    Some((r, g, b))
}

/// Normalize a hex channel of arbitrary width (1–4 digits) to 8 bits by
/// rescaling across the channel's full range: `f` → `255`, `ffff` → `255`,
/// `8000` → `128`.
#[cfg_attr(not(unix), allow(dead_code))]
fn scale_hex_channel(digits: &str) -> Option<u8> {
    if digits.is_empty() || digits.len() > 4 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let max = (1u32 << (4 * digits.len() as u32)) - 1;
    Some(((value * 255 + max / 2) / max) as u8)
}

/// Ask the terminal for its background color, giving up after `timeout`.
///
/// Returns `None` — never blocks past `timeout`, never panics — when:
/// - stdin and stdout are not both TTYs (piped output, CI, `codewhale < file`),
/// - the platform has no supported query path (non-Unix; see the module docs),
/// - the terminal does not answer, or answers with something unparsable.
///
/// # Caveat
///
/// This reads from stdin, so it must only be called while the terminal is in
/// raw mode and before the event loop starts. Bytes that arrive during the
/// window and are not part of the reply are discarded — at startup that window
/// is sub-millisecond on any terminal that answers at all.
#[must_use]
pub fn query_terminal_background(timeout: std::time::Duration) -> Option<(u8, u8, u8)> {
    let reply = query_terminal(OSC11_QUERY, timeout)?;
    parse_osc11_reply(&String::from_utf8_lossy(&reply))
}

/// Write `query` to the terminal and read back one reply, giving up after
/// `timeout`. The reply is the bytes up to (not including) its BEL or `ESC \`
/// terminator; an `ESC` that opens the reply is kept. Shared by the OSC 11
/// background query and the kitty graphics probe (`tui::mark`), under the
/// same caveat as [`query_terminal_background`]: raw mode on, event loop not
/// yet reading stdin.
#[cfg(unix)]
pub(crate) fn query_terminal(query: &[u8], timeout: std::time::Duration) -> Option<Vec<u8>> {
    query_terminal_inner(query, timeout, false)
}

/// CSI-terminated variant of [`query_terminal`] for the sixel probe
/// (`tui::mark`): a primary-DA reply ends at its alphabetic final byte
/// (`c`), which is neither BEL nor `ESC \`, so the plain reader would keep
/// swallowing input — including the user's own typed-ahead keystrokes —
/// until its byte cap. Stops after the final byte of a reply that opened
/// with `ESC [` and keeps the same raw-mode caveat.
#[cfg(unix)]
pub(crate) fn query_terminal_csi(query: &[u8], timeout: std::time::Duration) -> Option<Vec<u8>> {
    query_terminal_inner(query, timeout, true)
}

#[cfg(unix)]
fn query_terminal_inner(
    query: &[u8],
    timeout: std::time::Duration,
    stop_at_csi_final: bool,
) -> Option<Vec<u8>> {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::time::Instant;

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let in_fd = stdin.as_raw_fd();
    let out_fd = stdout.as_raw_fd();

    // SAFETY: `isatty` only inspects the descriptor; both fds are owned by the
    // std handles held above for the duration of the call.
    let both_tty = unsafe { libc::isatty(in_fd) == 1 && libc::isatty(out_fd) == 1 };
    if !both_tty {
        return None;
    }

    {
        let mut out = stdout.lock();
        out.write_all(query).ok()?;
        out.flush().ok()?;
    }

    let deadline = Instant::now() + timeout;
    let mut reply = Vec::with_capacity(32);
    let mut stdin = stdin.lock();
    let mut byte = [0u8; 1];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        if !wait_readable(in_fd, remaining) {
            return None;
        }
        match stdin.read(&mut byte) {
            Ok(1) => {}
            _ => return None,
        }
        // BEL, or the ESC of a `ESC \` string terminator, ends the reply.
        if byte[0] == 0x07 || (byte[0] == 0x1b && !reply.is_empty()) {
            // Consume the `\` of an `ESC \` terminator so it cannot surface
            // later as a keypress once the event loop owns stdin.
            if byte[0] == 0x1b
                && wait_readable(in_fd, std::time::Duration::from_millis(5))
                && stdin.read(&mut byte).is_ok_and(|n| n == 1)
                && byte[0] != b'\\'
            {
                reply.push(byte[0]);
            }
            break;
        }
        reply.push(byte[0]);
        // A CSI reply (`ESC [` …) ends at its first final byte (`@..=~`):
        // keep the final and stop, so a DA answer never eats past itself.
        if stop_at_csi_final
            && reply.len() >= 3
            && reply[0] == 0x1b
            && reply[1] == b'['
            && (0x40..=0x7e).contains(&byte[0])
        {
            break;
        }
        if reply.len() >= 128 {
            return None;
        }
    }

    Some(reply)
}

/// Block until `fd` has data or `timeout` elapses. `true` means readable.
#[cfg(unix)]
fn wait_readable(fd: std::os::fd::RawFd, timeout: std::time::Duration) -> bool {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(timeout.as_millis())
        .unwrap_or(i32::MAX)
        .max(1);
    // SAFETY: `pollfd` is a live, correctly-initialized single-element array
    // and the count matches.
    let rc = unsafe { libc::poll(std::ptr::addr_of_mut!(pollfd), 1, millis) };
    rc > 0 && (pollfd.revents & libc::POLLIN) != 0
}

/// Non-Unix platforms have no portable way to read a raw OSC reply back off
/// the console handle, so detection falls through to the environment-based
/// sources. Callers treat `None` as "no evidence", never as "dark".
#[cfg(not(unix))]
pub(crate) fn query_terminal(_query: &[u8], _timeout: std::time::Duration) -> Option<Vec<u8>> {
    None
}

/// Non-Unix twin of [`query_terminal_csi`]: no console to ask, no evidence.
#[cfg(not(unix))]
pub(crate) fn query_terminal_csi(_query: &[u8], _timeout: std::time::Duration) -> Option<Vec<u8>> {
    None
}
