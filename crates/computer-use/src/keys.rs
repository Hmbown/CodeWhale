//! Platform-neutral key names and `ctrl+shift+t` style combo parsing.
//!
//! The model speaks one vocabulary on every target; each driver maps
//! [`NamedKey`] to its own key codes and decides what `Meta` means (Command
//! on macOS, Windows key on Windows, Super on Linux; ignored on devices).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    CapsLock,
    F(u8),
    // Device / media keys.
    Back,
    AppHome,
    Recents,
    Power,
    VolumeUp,
    VolumeDown,
    Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Named(NamedKey),
    /// A printable character; drivers pick the key code from their layout
    /// table and add Shift where the character requires it.
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.modifiers.ctrl {
            parts.push("ctrl".into());
        }
        if self.modifiers.alt {
            parts.push("alt".into());
        }
        if self.modifiers.shift {
            parts.push("shift".into());
        }
        if self.modifiers.meta {
            parts.push("meta".into());
        }
        parts.push(match self.key {
            Key::Named(named) => named_key_name(named).to_string(),
            Key::Char(c) => c.to_string(),
        });
        write!(f, "{}", parts.join("+"))
    }
}

pub const KEY_NAMES: &[&str] = &[
    "enter",
    "tab",
    "esc",
    "backspace",
    "delete",
    "space",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "insert",
    "capslock",
    "f1..f24",
    "back",
    "apphome",
    "recents",
    "power",
    "volume_up",
    "volume_down",
    "menu",
];

pub const MODIFIER_NAMES: &[&str] = &["ctrl", "alt", "shift", "meta"];

fn named_key_name(key: NamedKey) -> &'static str {
    match key {
        NamedKey::Enter => "enter",
        NamedKey::Tab => "tab",
        NamedKey::Escape => "esc",
        NamedKey::Backspace => "backspace",
        NamedKey::Delete => "delete",
        NamedKey::Space => "space",
        NamedKey::Up => "up",
        NamedKey::Down => "down",
        NamedKey::Left => "left",
        NamedKey::Right => "right",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::PageUp => "pageup",
        NamedKey::PageDown => "pagedown",
        NamedKey::Insert => "insert",
        NamedKey::CapsLock => "capslock",
        NamedKey::F(_) => "f<n>",
        NamedKey::Back => "back",
        NamedKey::AppHome => "apphome",
        NamedKey::Recents => "recents",
        NamedKey::Power => "power",
        NamedKey::VolumeUp => "volume_up",
        NamedKey::VolumeDown => "volume_down",
        NamedKey::Menu => "menu",
    }
}

fn parse_named(token: &str) -> Option<NamedKey> {
    let key = match token {
        "enter" | "return" | "ret" => NamedKey::Enter,
        "tab" => NamedKey::Tab,
        "esc" | "escape" => NamedKey::Escape,
        "backspace" | "bs" => NamedKey::Backspace,
        "delete" | "del" | "forwarddelete" => NamedKey::Delete,
        "space" | "spacebar" => NamedKey::Space,
        "up" | "arrowup" => NamedKey::Up,
        "down" | "arrowdown" => NamedKey::Down,
        "left" | "arrowleft" => NamedKey::Left,
        "right" | "arrowright" => NamedKey::Right,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" | "pgup" => NamedKey::PageUp,
        "pagedown" | "pgdn" | "pgdown" => NamedKey::PageDown,
        "insert" | "ins" => NamedKey::Insert,
        "capslock" => NamedKey::CapsLock,
        "back" => NamedKey::Back,
        "apphome" | "homescreen" => NamedKey::AppHome,
        "recents" | "appswitch" | "app_switch" | "overview" => NamedKey::Recents,
        "power" => NamedKey::Power,
        "volume_up" | "volumeup" | "vol_up" => NamedKey::VolumeUp,
        "volume_down" | "volumedown" | "vol_down" => NamedKey::VolumeDown,
        "menu" => NamedKey::Menu,
        _ => {
            let digits = token.strip_prefix('f')?;
            let n: u8 = digits.parse().ok()?;
            if (1..=24).contains(&n) {
                NamedKey::F(n)
            } else {
                return None;
            }
        }
    };
    Some(key)
}

/// Parse `"ctrl+shift+t"`, `"Cmd+a"`, `"enter"`, `"back"`, or a literal `"+"`.
///
/// Tokens are separated by `+`; a trailing empty token means the literal
/// plus sign (`"ctrl++"` is Ctrl and `+`). Modifier aliases: `cmd`, `command`,
/// `super`, `win`, `windows`, `meta` → meta; `option`, `opt`, `alt` → alt;
/// `control`, `ctl`, `ctrl` → ctrl; `shift` → shift.
pub fn parse_combo(input: &str) -> Result<KeyCombo, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("keys must not be empty".to_string());
    }
    if trimmed == "+" {
        return Ok(KeyCombo {
            modifiers: Modifiers::default(),
            key: Key::Char('+'),
        });
    }
    let mut modifiers = Modifiers::default();
    let mut key: Option<Key> = None;
    let mut tokens: Vec<&str> = trimmed.split('+').collect();
    // "ctrl++" splits into ["ctrl", "", ""]: fold the two empties into "+".
    if tokens.len() >= 2
        && tokens[tokens.len() - 1].is_empty()
        && tokens[tokens.len() - 2].is_empty()
    {
        tokens.truncate(tokens.len() - 2);
        tokens.push("+");
    }
    for raw in tokens {
        let token = raw.trim();
        if token.is_empty() {
            return Err(format!("malformed key combo `{input}`"));
        }
        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" | "ctl" => modifiers.ctrl = true,
            "alt" | "option" | "opt" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "meta" | "cmd" | "command" | "super" | "win" | "windows" => modifiers.meta = true,
            _ => {
                if key.is_some() {
                    return Err(format!(
                        "key combo `{input}` names more than one non-modifier key"
                    ));
                }
                if let Some(named) = parse_named(&lower) {
                    key = Some(Key::Named(named));
                } else if token.chars().count() == 1 {
                    key = Some(Key::Char(token.chars().next().unwrap_or(' ')));
                } else {
                    return Err(format!(
                        "unknown key `{token}`; modifiers: {}; keys: {} or a single character",
                        MODIFIER_NAMES.join(", "),
                        KEY_NAMES.join(", ")
                    ));
                }
            }
        }
    }
    let key = key.ok_or_else(|| {
        format!("key combo `{input}` has only modifiers; add a key such as `a` or `enter`")
    })?;
    Ok(KeyCombo { modifiers, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifier_combo() {
        let combo = parse_combo("ctrl+shift+t").unwrap();
        assert!(combo.modifiers.ctrl && combo.modifiers.shift);
        assert!(!combo.modifiers.alt && !combo.modifiers.meta);
        assert_eq!(combo.key, Key::Char('t'));
    }

    #[test]
    fn parses_cmd_alias_case_insensitively() {
        let combo = parse_combo("Cmd+A").unwrap();
        assert!(combo.modifiers.meta);
        assert_eq!(combo.key, Key::Char('A'));
    }

    #[test]
    fn parses_named_and_device_keys() {
        assert_eq!(
            parse_combo("enter").unwrap().key,
            Key::Named(NamedKey::Enter)
        );
        assert_eq!(parse_combo("Back").unwrap().key, Key::Named(NamedKey::Back));
        assert_eq!(parse_combo("f12").unwrap().key, Key::Named(NamedKey::F(12)));
        assert_eq!(
            parse_combo("volume_up").unwrap().key,
            Key::Named(NamedKey::VolumeUp)
        );
    }

    #[test]
    fn parses_literal_plus() {
        assert_eq!(parse_combo("+").unwrap().key, Key::Char('+'));
        let combo = parse_combo("ctrl++").unwrap();
        assert!(combo.modifiers.ctrl);
        assert_eq!(combo.key, Key::Char('+'));
    }

    #[test]
    fn rejects_unknown_and_modifier_only() {
        let err = parse_combo("ctrl+launch").unwrap_err();
        assert!(err.contains("unknown key `launch`"), "{err}");
        assert!(err.contains("enter"));
        let err = parse_combo("ctrl+shift").unwrap_err();
        assert!(err.contains("only modifiers"), "{err}");
        assert!(parse_combo("").is_err());
        assert!(parse_combo("a+b").is_err());
    }

    #[test]
    fn display_round_trips() {
        assert_eq!(
            parse_combo("ctrl+shift+t").unwrap().to_string(),
            "ctrl+shift+t"
        );
        assert_eq!(parse_combo("cmd+enter").unwrap().to_string(), "meta+enter");
    }
}
