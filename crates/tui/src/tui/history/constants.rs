//! Shared constants for history transcript rendering.

pub(super) const TOOL_COMMAND_LINE_LIMIT: usize = 3;
pub(super) const TOOL_OUTPUT_LINE_LIMIT: usize = 6;
pub(super) const TOOL_TEXT_LIMIT: usize = 300;
pub(super) const TOOL_HEADER_SUMMARY_LIMIT: usize = 56;
pub(super) const TOOL_OUTPUT_HEAD_LINES: usize = 2;
pub(super) const TOOL_OUTPUT_TAIL_LINES: usize = 2;
#[cfg(test)]
pub(super) const TOOL_RUNNING_SYMBOLS: [&str; 12] = crate::tui::spinner::BRAILLE_SPINNER_FRAMES;
#[cfg(test)]
pub(super) const TOOL_STATUS_SYMBOL_MS: u64 = crate::tui::spinner::BRAILLE_SPINNER_FRAME_MS;
/// Visual marker for the user role at the start of their message line. Solid
/// vertical bar — no animation; user input is a finished thing.
pub(super) const USER_GLYPH: &str = "\u{258E}"; // ▎
/// Visual marker for the assistant role. Solid bullet that pulses at 2s
/// cycle while the response is streaming, holds full brightness when idle.
pub(super) const ASSISTANT_GLYPH: &str = "\u{25CF}"; // ●
/// Transcript body left rail. Solid 1/8 block (`▏`) followed by a space —
/// used as a visual left-margin anchor for continuation lines, tool-card
/// detail rows, and affordance lines. Dimmed so it guides the eye without
/// competing with content.
pub(super) const TRANSCRIPT_RAIL: &str = "\u{258F} "; // ▏ + space
pub(super) const TOOL_CARD_SUMMARY_LINES: usize = 4;
pub(super) const TOOL_DONE_SYMBOL: &str = "•";
pub(super) const TOOL_FAILED_SYMBOL: &str = "•";
/// Compact Ctrl+B affordance for foreground shell waits in the live transcript.
pub(super) const FOREGROUND_SHELL_WAIT_HINT: &str = "Ctrl+B → /jobs";
