//! Tool-card visual vocabulary for the v0.6.6 transcript redesign.
//!
//! Tool cards are the boxes that appear when the agent runs `read_file`,
//! `exec_shell`, `apply_patch`, etc. The visual vocabulary is intentionally
//! sparse: a single verb glyph identifies the family, a left rail anchors
//! the card to the timeline, and the spinner cadence (720 ms/step) reuses
//! the existing tool-status animation.
//!
//! This module owns:
//!
//! - [`ToolFamily`] — the seven canonical families plus a `Generic`
//!   fallback for anything we don't have a family for yet.
//! - [`tool_family_for_title`] — maps the legacy `render_tool_header` title
//!   string (`"Shell"`, `"Patch"`, `"Workspace"`, etc.) to a family. Lets
//!   the existing call sites drop in family glyphs without re-architecting
//!   each cell.
//! - [`family_glyph`] / [`family_label`] — the verb glyph + label per
//!   family. Glyphs are single graphemes; labels are short verbs.
//! - [`CardRail`] / [`rail_glyph`] — the `╭ │ ╰` rail anchored to the
//!   left margin so the eye can group multi-line cards.
//!
//! The actual line composition still happens inside `history.rs`; this
//! module is the vocabulary, not the layout engine. Keeping it small means
//! a future visual refresh only has to touch the constants here.

/// Tool family — the verb the agent is performing. Used to pick a glyph
/// and label for the card header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFamily {
    /// Reads, listings, exploration. `Read`.
    Read,
    /// Edits, patches, writes. `Edit`.
    Patch,
    /// Shell, child processes. `Bash`.
    Run,
    /// Grep, fuzzy file search, web search. `Search`.
    Find,
    /// Single sub-agent dispatch. `Task`.
    Delegate,
    /// Multi-agent fanout dispatch. `Fanout`.
    Fanout,
    /// Recursive language model work. `RLM`.
    Rlm,
    /// Reasoning / chain-of-thought. `Think`. Reasoning has its own
    /// render path (`render_thinking` in `history.rs`); the family is
    /// declared here for completeness so any future code that reaches for
    /// it has the matching glyph + label vocabulary.
    #[allow(dead_code)]
    Think,
    /// Anything we don't have a family glyph for yet — falls back to a
    /// neutral bullet so the card still renders cleanly.
    Generic,
}

/// Map a legacy tool-header title string (the value passed to
/// `render_tool_header`) to a family. Anything unrecognised falls back to
/// [`ToolFamily::Generic`] so cards still render — they just lose the
/// verb-glyph treatment until the family is added here.
#[must_use]
pub fn tool_family_for_title(title: &str) -> ToolFamily {
    match title {
        "Shell" => ToolFamily::Run,
        "Patch" | "Diff" => ToolFamily::Patch,
        "Workspace" | "Image" => ToolFamily::Read,
        "Search" => ToolFamily::Find,
        "Plan" | "Review" => ToolFamily::Generic,
        _ => ToolFamily::Generic,
    }
}

/// Map an arbitrary tool name (as exposed to the model — e.g. `read_file`,
/// `apply_patch`, `agent_open`) to a family. Used by `GenericToolCell`
/// where the `tool_family_for_title` shortcut isn't enough because every
/// generic cell shares the title `"Tool"`.
#[must_use]
pub fn tool_family_for_name(name: &str) -> ToolFamily {
    match name {
        "read_file" | "list_dir" | "view_image" => ToolFamily::Read,
        "edit_file" | "apply_patch" | "write_file" => ToolFamily::Patch,
        "exec_shell" | "exec_shell_wait" | "exec_shell_interact" => ToolFamily::Run,
        "grep_files" | "file_search" | "web_search" | "fetch_url" => ToolFamily::Find,
        "Task" | "agent_open" | "agent_eval" | "agent_close" | "agent_spawn" => {
            ToolFamily::Delegate
        }
        "rlm_open" | "rlm_eval" | "rlm_configure" | "rlm_close" | "rlm" => ToolFamily::Rlm,
        _ => ToolFamily::Generic,
    }
}

/// Build a compact semantic summary for a tool header from the public tool
/// name and the already-sanitized argument summary.
#[must_use]
pub fn tool_header_summary_for_name(name: &str, input_summary: Option<&str>) -> Option<String> {
    let summary = input_summary?.trim();
    if summary.is_empty() {
        return None;
    }

    let preferred_keys = match tool_family_for_name(name) {
        ToolFamily::Read | ToolFamily::Patch => ["path", "file", "target", "content"].as_slice(),
        ToolFamily::Run => ["command", "cmd", "script"].as_slice(),
        ToolFamily::Find => ["query", "pattern", "path", "scope"].as_slice(),
        ToolFamily::Delegate | ToolFamily::Fanout | ToolFamily::Rlm => [
            "description",
            "display_name",
            "name",
            "subagent_type",
            "agent_type",
            "type",
            "role",
            "task",
            "prompt",
            "model",
        ]
        .as_slice(),
        ToolFamily::Think | ToolFamily::Generic => {
            ["query", "path", "command", "prompt"].as_slice()
        }
    };

    let family = tool_family_for_name(name);
    for key in preferred_keys {
        if let Some(value) = summary_value(summary, key) {
            if matches!(family, ToolFamily::Delegate) && value_is_char_count_placeholder(&value) {
                continue;
            }
            return Some(value);
        }
    }

    if matches!(family, ToolFamily::Delegate) && summary_contains_only_long_placeholder(summary) {
        return None;
    }
    if matches!(family, ToolFamily::Delegate) && summary_contains_only_internal_id(summary) {
        return None;
    }

    Some(summary.to_string())
}

fn value_is_char_count_placeholder(value: &str) -> bool {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('<').and_then(|v| v.strip_suffix('>')) else {
        return false;
    };
    let Some(number) = inner.strip_suffix(" chars") else {
        return false;
    };
    !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
}

fn summary_contains_only_long_placeholder(summary: &str) -> bool {
    let mut saw_part = false;
    for part in summary.split(", ") {
        let value = part
            .split_once(':')
            .map(|(_, value)| value.trim())
            .unwrap_or_else(|| part.trim());
        if value.is_empty() {
            continue;
        }
        saw_part = true;
        if !value_is_char_count_placeholder(value) {
            return false;
        }
    }
    saw_part
}

fn summary_contains_only_internal_id(summary: &str) -> bool {
    let mut saw_part = false;
    for part in summary.split(", ") {
        let Some((part_key, value)) = part.split_once(':') else {
            return false;
        };
        let key = part_key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        saw_part = true;
        if !matches!(key, "agent_id" | "id") {
            return false;
        }
    }
    saw_part
}

fn summary_value(summary: &str, key: &str) -> Option<String> {
    for part in summary.split(", ") {
        let Some((part_key, value)) = part.split_once(':') else {
            continue;
        };
        if part_key.trim() == key {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// The verb glyph for a family. Single grapheme so the header layout math
/// in `render_tool_header` stays simple (one cell wide).
#[must_use]
pub fn family_glyph(family: ToolFamily) -> &'static str {
    match family {
        ToolFamily::Read => "\u{25B7}",           // ▷
        ToolFamily::Patch => "\u{25C6}",          // ◆
        ToolFamily::Run => "\u{25B6}",            // ▶
        ToolFamily::Find => "\u{2315}",           // ⌕
        ToolFamily::Delegate => "\u{25D0}",       // ◐
        ToolFamily::Fanout => "\u{22EE}\u{22EE}", // ⋮⋮ (two cells)
        ToolFamily::Rlm => "\u{22EE}\u{22EE}",    // ⋮⋮ (two cells)
        ToolFamily::Think => "\u{2026}",          // …
        ToolFamily::Generic => "\u{2022}",        // •
    }
}

/// The short title for a family. Matches Claude Code's terse tool names.
#[must_use]
pub fn family_label(family: ToolFamily) -> &'static str {
    match family {
        ToolFamily::Read => "Read",
        ToolFamily::Patch => "Edit",
        ToolFamily::Run => "Bash",
        ToolFamily::Find => "Search",
        ToolFamily::Delegate => "Task",
        ToolFamily::Fanout => "Fanout",
        ToolFamily::Rlm => "RLM",
        ToolFamily::Think => "Think",
        ToolFamily::Generic => "Tool",
    }
}

/// Position of a line within a multi-line card — drives the left-rail
/// glyph so the box reads as a contiguous group from top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // wired by future card-refactor follow-ups
pub enum CardRail {
    /// First line of the card — the header. `╭`.
    Top,
    /// Any middle line — body content. `│`.
    Middle,
    /// Last line of the card. `╰`.
    Bottom,
    /// Single-line card — no rail at all.
    Single,
}

/// Map a [`CardRail`] position to its rail glyph. Returned as a `&str`
/// because callers paste it into a span.
#[must_use]
#[allow(dead_code)] // wired by future card-refactor follow-ups
pub fn rail_glyph(rail: CardRail) -> &'static str {
    match rail {
        CardRail::Top => "\u{256D}",    // ╭
        CardRail::Middle => "\u{2502}", // │
        CardRail::Bottom => "\u{2570}", // ╰
        CardRail::Single => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CardRail, ToolFamily, family_glyph, family_label, rail_glyph, tool_family_for_name,
        tool_family_for_title, tool_header_summary_for_name,
    };

    #[test]
    fn legacy_titles_route_to_expected_families() {
        assert_eq!(tool_family_for_title("Shell"), ToolFamily::Run);
        assert_eq!(tool_family_for_title("Patch"), ToolFamily::Patch);
        assert_eq!(tool_family_for_title("Workspace"), ToolFamily::Read);
        assert_eq!(tool_family_for_title("Search"), ToolFamily::Find);
        assert_eq!(tool_family_for_title("Diff"), ToolFamily::Patch);
        assert_eq!(tool_family_for_title("Plan"), ToolFamily::Generic);
        assert_eq!(tool_family_for_title("unknown title"), ToolFamily::Generic);
    }

    #[test]
    fn tool_names_route_to_families_by_verb() {
        assert_eq!(tool_family_for_name("read_file"), ToolFamily::Read);
        assert_eq!(tool_family_for_name("apply_patch"), ToolFamily::Patch);
        assert_eq!(tool_family_for_name("exec_shell"), ToolFamily::Run);
        assert_eq!(tool_family_for_name("grep_files"), ToolFamily::Find);
        assert_eq!(tool_family_for_name("agent_open"), ToolFamily::Delegate);
        assert_eq!(tool_family_for_name("rlm_eval"), ToolFamily::Rlm);
        assert_eq!(
            tool_family_for_name("totally_new_tool"),
            ToolFamily::Generic
        );
    }

    #[test]
    fn tool_header_summary_prefers_family_specific_arguments() {
        assert_eq!(
            tool_header_summary_for_name("read_file", Some("path: src/main.rs, limit: 20"))
                .as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            tool_header_summary_for_name("exec_shell", Some("command: cargo test, cwd: /repo"))
                .as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            tool_header_summary_for_name("grep_files", Some("pattern: TODO, path: crates"))
                .as_deref(),
            Some("TODO")
        );
        assert_eq!(
            tool_header_summary_for_name("unknown", Some("alpha: beta")).as_deref(),
            Some("alpha: beta")
        );
    }

    #[test]
    fn delegate_header_summary_hides_bare_internal_agent_id() {
        assert_eq!(
            tool_header_summary_for_name("agent_eval", Some("agent_id: agent_c339a5e40")),
            None
        );
        assert_eq!(
            tool_header_summary_for_name("agent_eval", Some("id: agent_01640bde")),
            None
        );
    }

    #[test]
    fn each_family_has_a_glyph_and_label() {
        // Smoke test — surface accidental empties from a future refactor.
        for family in [
            ToolFamily::Read,
            ToolFamily::Patch,
            ToolFamily::Run,
            ToolFamily::Find,
            ToolFamily::Delegate,
            ToolFamily::Fanout,
            ToolFamily::Rlm,
            ToolFamily::Think,
            ToolFamily::Generic,
        ] {
            assert!(
                !family_glyph(family).is_empty(),
                "family {family:?} has empty glyph",
            );
            assert!(
                !family_label(family).is_empty(),
                "family {family:?} has empty label",
            );
        }
    }

    #[test]
    fn card_rail_glyphs_form_a_box() {
        assert_eq!(rail_glyph(CardRail::Top), "\u{256D}");
        assert_eq!(rail_glyph(CardRail::Middle), "\u{2502}");
        assert_eq!(rail_glyph(CardRail::Bottom), "\u{2570}");
        assert!(rail_glyph(CardRail::Single).is_empty());
    }
}
