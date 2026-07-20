//! Codewhale's terminal glyph charter.
//!
//! Renderers use semantic names from this module instead of choosing visual
//! punctuation ad hoc. The solid current marker (`●`) is the recurring
//! Codewhale anchor: it marks the active speaker or current human choice.
//! ASCII-safe terminals receive the semantic fallback from the same owner.

/// Current speaker or current human choice — the recurring identity anchor.
pub const CURRENT: &str = "●";
/// Available but not current.
pub const AVAILABLE: &str = "○";
/// Keyboard/list selection pointer.
pub const SELECTION: &str = "▸";
/// Finished user-authored message marker.
pub const USER: &str = "▎";
/// Transcript continuation rail, including its authored trailing space.
pub const TRANSCRIPT_RAIL: &str = "▏ ";
/// Settled successful state.
pub const DONE: &str = "✓";
/// Settled failed state.
pub const FAILED: &str = "✕";
/// State that needs human attention.
pub const ATTENTION: &str = "◆";
/// Ready but not active.
pub const READY: &str = "○";
/// Paused work.
pub const PAUSED: &str = "⏸";
/// Fleet role marks share the same charter while retaining distinct shapes.
pub const ROLE_MANAGER: &str = "◆";
pub const ROLE_BUILDER: &str = "■";
pub const ROLE_REVIEWER: &str = "◇";
pub const ROLE_VERIFIER: &str = CURRENT;
pub const ROLE_SYNTHESIZER: &str = "▲";
pub const NEUTRAL: &str = "·";

#[must_use]
pub const fn selection_marker(selected: bool) -> &'static str {
    if selected { SELECTION } else { " " }
}

/// Reduce a single Codewhale-authored decorative glyph to narrow ASCII.
/// Language text and model/user content are intentionally outside this map.
#[must_use]
pub fn ascii_fallback(symbol: &str) -> Option<&'static str> {
    match symbol {
        "─" | "━" | "═" | "╌" | "╍" | "┄" | "┅" | "┈" | "┉" | "—" | "–" => {
            Some("-")
        }
        "│" | "┃" | "║" | "╎" | "╏" | "▏" | "▎" | "▍" | "▌" | "▐" | "▕" => {
            Some("|")
        }
        "┌" | "┐" | "└" | "┘" | "╭" | "╮" | "╰" | "╯" | "├" | "┤" | "┬" | "┴" | "┼" => {
            Some("+")
        }
        "█" | "▉" | "▊" | "▋" | "▀" | "▄" | "▅" | "▆" | "▇" | "▙" | "▛" | "▜" | "▟" | "▰" => {
            Some("#")
        }
        "▁" | "▂" | "▃" => Some("_"),
        "▖" | "▗" | "▘" | "▝" => Some("."),
        "▚" => Some("\\"),
        "▞" => Some("/"),
        "░" | "▒" | "▓" => Some(":"),
        "▱" => Some("-"),
        "▶" | "▸" | "›" | "❯" | "→" | "↗" | "↘" | "»" => Some(">"),
        "◀" | "◂" | "‹" | "❮" | "←" | "↖" | "↙" | "«" => Some("<"),
        "▼" | "▾" | "▽" | "↓" => Some("v"),
        "▲" | "△" | "↑" => Some("^"),
        "◆" | "◇" | "♦" | "✦" | "◍" | "◉" | "★" | "☆" => Some("*"),
        "■" | "□" | "▪" | "▫" | "◼" | "◻" => Some("#"),
        "●" | "○" | "∘" | "•" | "·" | "☐" => Some("."),
        "◌" | "˚" | "°" | "◦" => Some("o"),
        "✓" | "✔" | "☑" => Some("Y"),
        "✕" | "×" | "⊘" | "✗" | "✘" | "☒" => Some("X"),
        "⏸" => Some("="),
        "🐳" | "🐋" => Some("w"),
        "…" => Some("."),
        _ => None,
    }
}

/// Preserve the working-bubble fill signal when Braille is unavailable.
#[must_use]
pub fn braille_ascii_fallback(ch: char) -> Option<&'static str> {
    if !(('\u{2800}'..='\u{28FF}').contains(&ch)) {
        return None;
    }
    let dots = ((ch as u32) - 0x2800).count_ones();
    Some(match dots {
        0 => " ",
        1..=2 => ".",
        3..=4 => ":",
        5..=6 => "+",
        _ => "#",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charter_has_narrow_semantic_fallbacks() {
        for (rich, safe) in [
            (SELECTION, ">"),
            (CURRENT, "."),
            (USER, "|"),
            (DONE, "Y"),
            (FAILED, "X"),
            (ATTENTION, "*"),
        ] {
            assert_eq!(ascii_fallback(rich), Some(safe));
        }
        assert_eq!(braille_ascii_fallback('\u{2801}'), Some("."));
        assert_eq!(braille_ascii_fallback('A'), None);
    }
}
