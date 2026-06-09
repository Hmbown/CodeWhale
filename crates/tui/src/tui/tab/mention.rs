//! Smart @-mention parsing for cross-tab references
//!
//! When a user writes a message like "Hey @Tab2 can you review this?",
//! the mention parser extracts the referenced tab and suggests
//! automatically switching to it (or routing the message).
//!
//! Supported mention forms:
//! - `@Tab2` - by tab number (1-indexed)
//! - `@2` - shorthand for tab number
//! - `@tab2` - case-insensitive
//!
//! Examples:
//! ```
//! use crate::tui::tab::mention::extract_tab_mention;
//! assert_eq!(extract_tab_mention("Hello @Tab2!"), Some(2));
//! assert_eq!(extract_tab_mention("see @3"), Some(3));
//! assert_eq!(extract_tab_mention("no mention here"), None);
//! ```

// WIP collaboration surface — narrow harvest. See `tab/mod.rs` for the
// PR #2753 context.
#![allow(dead_code)]

/// Parse a message and extract the first tab mention (1-indexed number).
/// Returns `None` if no valid mention is found.
///
/// Recognized patterns:
/// - `@Tab<number>` (e.g. `@Tab2`, `@tab3`)
/// - `@<number>` at the start of a word (e.g. `@2`, `@3`)
///
/// The mention must be a single token (preceded by start-of-string or
/// whitespace, followed by whitespace, punctuation, or end-of-string).
pub fn extract_tab_mention(message: &str) -> Option<usize> {
    let bytes = message.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            // Must be at start or after whitespace
            if i > 0 && !is_mention_boundary(bytes[i - 1]) {
                i += 1;
                continue;
            }
            // Try `@Tab<n>`
            if i + 4 <= bytes.len()
                && bytes[i..i + 4].eq_ignore_ascii_case(b"@tab")
                && i + 4 < bytes.len()
                && bytes[i + 4].is_ascii_digit()
            {
                let num_start = i + 4;
                let num_end = scan_digits(bytes, num_start);
                if let Some(num) = parse_usize(&bytes[num_start..num_end])
                    && num > 0
                    && is_mention_terminator(bytes, num_end)
                {
                    return Some(num);
                }
            }
            // Try `@<n>` shorthand
            if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                let num_start = i + 1;
                let num_end = scan_digits(bytes, num_start);
                if let Some(num) = parse_usize(&bytes[num_start..num_end])
                    && num > 0
                    && is_mention_terminator(bytes, num_end)
                {
                    return Some(num);
                }
            }
        }
        i += 1;
    }
    None
}

/// Extract all tab mentions in order
pub fn extract_all_tab_mentions(message: &str) -> Vec<usize> {
    let mut mentions = Vec::new();
    let bytes = message.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || is_mention_boundary(bytes[i - 1])) {
            // Determine the mention token length
            let token_start = i;
            // @Tab<n> or @<n>
            let mut j = i + 1;
            if j + 3 <= bytes.len() && bytes[j..j + 3].eq_ignore_ascii_case(b"tab") {
                j += 3;
            }
            let num_start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let num_end = j;
            if num_start < num_end
                && is_mention_terminator(bytes, num_end)
                && let Some(num) = parse_usize(&bytes[num_start..num_end])
                && num > 0
            {
                mentions.push(num);
                // Also skip past the terminator
                i = num_end;
                continue;
            }
            let _ = token_start; // suppress unused
        }
        i += 1;
    }
    mentions
}

fn is_mention_boundary(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b',' || b == b';'
}

fn is_mention_terminator(bytes: &[u8], pos: usize) -> bool {
    pos >= bytes.len()
        || bytes[pos] == b' '
        || bytes[pos] == b'\t'
        || bytes[pos] == b'\n'
        || bytes[pos] == b'\r'
        || bytes[pos] == b','
        || bytes[pos] == b'.'
        || bytes[pos] == b'!'
        || bytes[pos] == b'?'
        || bytes[pos] == b';'
        || bytes[pos] == b':'
}

fn scan_digits(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    end
}

fn parse_usize(s: &[u8]) -> Option<usize> {
    if s.is_empty() {
        return None;
    }
    let mut result: usize = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?;
        result = result.checked_add((b - b'0') as usize)?;
    }
    Some(result)
}

/// Given a tab number (1-indexed) and the list of tab IDs, return the
/// matching TabId. Returns `None` if the index is out of range.
///
/// The caller is expected to pass the IDs in **visual order** (i.e. the
/// order they appear in the tab bar). We deliberately do not sort the
/// list here — tab mentions like `@Tab2` should map to the second tab the
/// user sees, not the second-smallest ID.
pub fn resolve_tab_mention<'a, I>(tab_number: usize, tab_ids: I) -> Option<u64>
where
    I: IntoIterator<Item = &'a u64>,
{
    let ids: Vec<u64> = tab_ids.into_iter().copied().collect();
    if tab_number == 0 || tab_number > ids.len() {
        return None;
    }
    Some(ids[tab_number - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_tab_mention() {
        assert_eq!(extract_tab_mention("Hello @Tab2!"), Some(2));
        assert_eq!(extract_tab_mention("see @3 please"), Some(3));
        assert_eq!(extract_tab_mention("@Tab1 help"), Some(1));
        assert_eq!(extract_tab_mention("no mention here"), None);
        assert_eq!(extract_tab_mention("email@2 is wrong"), None); // not at boundary
        assert_eq!(extract_tab_mention("@0 invalid"), None); // 0 not allowed
        assert_eq!(extract_tab_mention(""), None);
    }

    #[test]
    fn test_extract_case_insensitive() {
        assert_eq!(extract_tab_mention("Hello @TAB2!"), Some(2));
        assert_eq!(extract_tab_mention("see @tab3 please"), Some(3));
    }

    #[test]
    fn test_extract_with_punctuation() {
        assert_eq!(extract_tab_mention("Please @Tab2, review"), Some(2));
        assert_eq!(extract_tab_mention("Ask @Tab2."), Some(2));
        assert_eq!(extract_tab_mention("Hey @Tab2!"), Some(2));
        assert_eq!(extract_tab_mention("What about @Tab2?"), Some(2));
    }

    #[test]
    fn test_extract_all_mentions() {
        let mentions = extract_all_tab_mentions("Hey @Tab2 and @3, also @Tab1");
        assert_eq!(mentions, vec![2, 3, 1]);
    }

    #[test]
    fn test_extract_mention_at_start() {
        assert_eq!(extract_tab_mention("@Tab1 hi"), Some(1));
    }

    #[test]
    fn test_resolve_tab_mention() {
        // Tab IDs in the visual order they appear in the tab bar.
        let tab_ids = vec![100, 50, 200];
        // Tab 1 = first in visual order (100)
        assert_eq!(resolve_tab_mention(1, tab_ids.iter()), Some(100));
        // Tab 2 = second in visual order (50)
        assert_eq!(resolve_tab_mention(2, tab_ids.iter()), Some(50));
        // Tab 3 = third in visual order (200)
        assert_eq!(resolve_tab_mention(3, tab_ids.iter()), Some(200));
        // Out of range
        assert_eq!(resolve_tab_mention(4, tab_ids.iter()), None);
        assert_eq!(resolve_tab_mention(0, tab_ids.iter()), None);
    }
}
