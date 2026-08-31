//! Phase and identity bands for the underwater shell.
//!
//! Two one-row bands bracket the composer, and they never trade places
//! with it:
//!
//! * the **identity band** below the composer is the canonical, persistent
//!   home for `provider · model · thinking level`. The same standing facts
//!   render before, during, and after a prompt; a live turn never relocates
//!   them and neither band ever duplicates them.
//! * the **activity band** above the composer carries the transient pulse:
//!   phase marker, live work detail, session notices, and the cost/metrics
//!   ledger.
//!
//! Both bands are reserved in every frame, so a turn moving between idle,
//! thinking, tool use, approval, completion, failure, and cancellation
//! changes text inside fixed rows and never displaces the composer — the
//! route identity does not jump above the prompt when a turn starts, and
//! the prompt does not slide down to make room for it.

use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::{MessageId, tr};
use crate::palette::ChromeInk;
use crate::tui::{
    app::App,
    underwater::{LiveActivity, ShellPhase, ShellTier, phase_marker_with_activity},
};

/// Fixed one-row reservation for the identity band below the composer.
#[must_use]
pub fn height() -> u16 {
    1
}

/// Compact working detail for the phase band: `×N` for tools or `1m 15s`
/// while the model is thinking.
/// Kept quieter than the classic footer's verbose tool-status line so the
/// transcript owns the ledger and the strip only names the live pulse.
fn working_detail(app: &App, activity: LiveActivity) -> Option<String> {
    let running = activity.running_tool_count();
    let secs = app
        .turn_started_at
        .map(|started| started.elapsed().as_secs());
    match (running, secs) {
        (0, Some(secs)) if secs > 0 => Some(crate::elapsed::format_elapsed_secs(secs)),
        (n, Some(_)) if n > 0 => Some(format!("×{n}")),
        (n, None) if n > 0 => Some(format!("×{n}")),
        _ => None,
    }
}

/// Route identity for a rail or topbar segment, shed field by field until it
/// fits `budget`.
///
/// The old version composed the full `provider · model · effort` label and
/// then `truncate_to_width`'d it to a fixed 24/44/64 columns, which happily
/// rendered `deepseek-v4-flash-prev…`. A clipped model name is worse than no
/// model name: routes share prefixes, so the ellipsis is the rail admitting
/// it will not tell you which model is answering. Shed the qualifiers
/// instead — provider first, then effort — and if the bare model name still
/// does not fit, shed the whole group. `/model` and `/status` own the full
/// route either way.
pub(crate) fn route_identity_fields(
    app: &App,
    tier: ShellTier,
    budget: usize,
) -> Option<Vec<String>> {
    let (provider, model) = app.effective_route_identity_display();
    let effort = app.reasoning_effort_display_label();
    if model.is_empty() {
        return None;
    }
    let mut candidates: Vec<Vec<String>> = Vec::new();
    if tier != ShellTier::Compact && !provider.is_empty() && !effort.is_empty() {
        // The smallest shell never repeats the provider: model and effort are
        // the two facts that change what comes back.
        candidates.push(vec![provider, model.clone(), effort.clone()]);
    }
    if !effort.is_empty() {
        candidates.push(vec![model.clone(), effort]);
    }
    candidates.push(vec![model]);
    candidates.into_iter().find(|fields| {
        let width = fields.iter().map(|field| field.width()).sum::<usize>()
            + fields.len().saturating_sub(1) * ITEM_SEPARATOR_WIDTH;
        width <= budget
    })
}

/// Split a notice at its joints, coarsest first.
///
/// A rail notice is prose, and prose has joints. Cutting at a joint keeps
/// every word that survives true; cutting mid-phrase and hanging an ellipsis
/// off the end only advertises that the row lost the argument. Sentence stops
/// are the joint we want; the inner marks are the fallback for a one-sentence
/// notice that is still too long for a narrow rail — losing the second half
/// of `Auto-denied exec_shell: denied earlier` beats losing the warning.
fn notice_clauses<'a>(text: &'a str, marks: &[char]) -> Vec<&'a str> {
    let mut clauses = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if !marks.contains(&ch) {
            continue;
        }
        // Full-width marks carry no trailing space, so they break on sight.
        // ASCII marks only break before whitespace, which keeps `0.9.11`,
        // `docs/TELEMETRY.md`, and `https://…` in one piece.
        let breaks = !ch.is_ascii() || chars.peek().is_none_or(|(_, next)| next.is_whitespace());
        if !breaks {
            continue;
        }
        let end = idx + ch.len_utf8();
        let clause = text[start..end].trim();
        if !clause.is_empty() {
            clauses.push(clause);
        }
        start = end;
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        clauses.push(rest);
    }
    clauses
}

/// Sentence stops — the joint a notice prefers to be cut at.
const SENTENCE_MARKS: [char; 7] = ['.', '!', '?', '…', '。', '！', '？'];
/// Inner joints, used only when one sentence still will not fit the rail.
const CLAUSE_MARKS: [char; 8] = [';', ':', ',', '—', '；', '：', '，', '、'];

fn join_while_fitting(clauses: &[&str], budget: usize) -> Option<String> {
    let mut fitted = String::new();
    for clause in clauses {
        // A full-width stop already carries its own breathing room; putting
        // a Latin space after `。` is a typographic accent in the wrong
        // language.
        let space = usize::from(!fitted.is_empty() && fitted.ends_with(|ch: char| ch.is_ascii()));
        let candidate = fitted.width() + space + clause.width();
        if candidate > budget {
            break;
        }
        if space == 1 {
            fitted.push(' ');
        }
        fitted.push_str(clause);
    }
    // A phrase that ends on `:` or `;` is still telling you more is coming —
    // the same lie an ellipsis tells. Cut the mark and let the phrase stand.
    let fitted = fitted
        .trim_end_matches(|ch| CLAUSE_MARKS.contains(&ch) || ch == ' ')
        .to_string();
    (!fitted.is_empty()).then_some(fitted)
}

/// Fit a notice into `budget` by dropping whole trailing clauses.
///
/// Returns `None` only when not even the first inner phrase fits, and the
/// rail then says nothing rather than dangling a stump. Notices get first
/// call on the row: identity and the ledger chips have already stood down by
/// the time this is asked, and the key hints stand down after it if that is
/// what the notice needs.
fn fit_notice(text: &str, budget: usize) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || budget == 0 {
        return None;
    }
    if text.width() <= budget {
        return Some(text.to_string());
    }
    let sentences = notice_clauses(text, &SENTENCE_MARKS);
    if let Some(fitted) = join_while_fitting(&sentences, budget) {
        return Some(fitted);
    }
    let first = sentences.first().copied().unwrap_or(text);
    join_while_fitting(&notice_clauses(first, &CLAUSE_MARKS), budget)
}

/// Toasts share the footer rail, so their typed level must resolve through
/// the same closed status-bar grammar as the phase marker around them.
fn status_toast_ink(level: crate::tui::app::StatusToastLevel) -> ChromeInk {
    match level {
        crate::tui::app::StatusToastLevel::Info => ChromeInk::Info,
        crate::tui::app::StatusToastLevel::Success => ChromeInk::Outcome,
        crate::tui::app::StatusToastLevel::Warning => ChromeInk::Attention,
        crate::tui::app::StatusToastLevel::Error => ChromeInk::Failure,
    }
}

/// Map the boot surface's typed severity through the same semantic palette as
/// every other footer fact. Keeping this conversion closed makes the plugin
/// warning/failure distinction testable without guessing from its text.
fn boot_activity_ink(level: crate::tui::session_boot::SessionBootActivityLevel) -> ChromeInk {
    match level {
        crate::tui::session_boot::SessionBootActivityLevel::Active => ChromeInk::Active,
        crate::tui::session_boot::SessionBootActivityLevel::Attention => ChromeInk::Attention,
        crate::tui::session_boot::SessionBootActivityLevel::Failure => ChromeInk::Failure,
    }
}

/// Pick the notice a band owes its row to right now, if any. Shared by the
/// classic activity band and the Tideline merged footer so the two can never
/// disagree about which toast is live. Completion may land in the same event
/// drain as an approval denial: unresolved Warning/Error receipts stay
/// visible after `done`, only routine informational copy yields.
fn selected_notice(
    status_toast: Option<crate::tui::app::StatusToast>,
    phase: ShellPhase,
    phase_label: &str,
) -> Option<(String, ChromeInk, bool)> {
    status_toast
        .filter(|toast| {
            let survives_completion = matches!(
                toast.level,
                crate::tui::app::StatusToastLevel::Warning
                    | crate::tui::app::StatusToastLevel::Error
            );
            (phase != ShellPhase::Done || survives_completion)
                && !toast.text.trim().is_empty()
                && toast.text.trim() != phase_label
        })
        .map(|toast| {
            let urgent = matches!(
                toast.level,
                crate::tui::app::StatusToastLevel::Warning
                    | crate::tui::app::StatusToastLevel::Error
            );
            (toast.text.clone(), status_toast_ink(toast.level), urgent)
        })
}

/// The identity band's right-aligned key legend, and — since the Tideline
/// footer merge — the merged footer's `keys_legend` source. Live phases keep
/// the row quiet; idle and drafting advertise the chords the shell owns.
/// `← for agents · ↓ to manage` joins the chorus whenever the empty composer
/// still owns those keys.
fn keys_legend(app: &App, tier: ShellTier, phase: ShellPhase) -> Cow<'static, str> {
    let right_text: Cow<'static, str> = if matches!(
        phase,
        ShellPhase::Working
            | ShellPhase::Verifying
            | ShellPhase::Waiting
            | ShellPhase::Approval
            | ShellPhase::Failed
    ) {
        Cow::Borrowed("")
    } else {
        use crate::tui::shell_key_routing::{ShellBindingId, binding, footer_action_hints};
        let hint_keys = tr(app.ui_locale, MessageId::FooterHintKeys);
        let hint_output = tr(app.ui_locale, MessageId::FooterHintOutput);
        Cow::Owned(match tier {
            ShellTier::Compact => {
                format!("{}:{hint_keys}", binding(ShellBindingId::Help).footer_chord)
            }
            // Wide used to add `/context:context`. The rail advertises
            // chords you cannot discover any other way; a slash command
            // announces itself the moment you type `/`, so it was paying
            // eighteen columns of a 24-row screen to tell you something
            // the composer already tells you. The rail now reads the same
            // at 80 columns as at 200.
            ShellTier::Normal | ShellTier::Wide => footer_action_hints(false)
                .replace("{output}", hint_output.as_ref())
                .replace("{keys}", hint_keys.as_ref()),
        })
    };

    // `← for agents · ↓ to manage`: advertise only while the empty
    // composer owns those keys and a worker exists. Focused surfaces,
    // modals, attachments, and draft text keep the arrows' local meaning.
    let agent_hints = (tier != ShellTier::Compact
        // Slash and mention menus require non-empty trigger text, which the
        // shared predicate rejects; dispatch additionally passes its exact
        // post-completion menu ownership into the same predicate.
        && crate::tui::agent_focus::shell_shortcuts_available(app, false))
    .then(|| crate::tui::agent_focus::footer_agent_hints(app));
    match agent_hints {
        Some(hints) if !right_text.is_empty() => {
            Cow::Owned(format!("{hints}{ITEM_SEPARATOR}{right_text}"))
        }
        // A settled turn keeps the agent keys meaningful, so they stay
        // visible on the identity row after `done` even without the chorus.
        Some(hints) if phase == ShellPhase::Done => Cow::Owned(hints),
        _ => right_text,
    }
}

/// Peers inside one group — provider and model, a count and its verb — keep
/// the middle dot.
const ITEM_SEPARATOR: &str = " · ";
const ITEM_SEPARATOR_WIDTH: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, tui::app::TuiOptions};
    use std::path::PathBuf;

    fn test_app() -> App {
        App::new(
            TuiOptions {
                model: "deepseek-v4-flash".to_string(),
                ..crate::test_support::test_tui_options(PathBuf::from("."))
            },
            &Config::default(),
        )
    }

    #[test]
    fn working_marker_uses_the_live_work_status_role() {
        let mut app = test_app();
        // Match Terminal intentionally aliases both roles to ANSI Cyan. Use
        // the branded palette here to prove the renderer selects the working
        // slot rather than merely observing an equal terminal color.
        app.ui_theme = crate::palette::UI_THEME;
        assert_eq!(ShellPhase::Working.color(&app), app.ui_theme.status_working);
        assert_ne!(ShellPhase::Working.color(&app), app.ui_theme.info);
        assert_eq!(
            crate::tui::underwater::phase_ink(ShellPhase::Working),
            ChromeInk::Active
        );
        assert_eq!(
            crate::tui::underwater::phase_ink(ShellPhase::Failed),
            ChromeInk::Failure
        );
        assert_ne!(
            crate::tui::underwater::phase_ink(ShellPhase::Working).family(),
            crate::palette::SemanticFamily::Failure
        );
    }

    #[test]
    fn boot_activity_levels_keep_plugin_attention_and_failure_distinct() {
        use crate::tui::session_boot::SessionBootActivityLevel;

        assert_eq!(
            boot_activity_ink(SessionBootActivityLevel::Active),
            ChromeInk::Active
        );
        assert_eq!(
            boot_activity_ink(SessionBootActivityLevel::Attention),
            ChromeInk::Attention
        );
        assert_eq!(
            boot_activity_ink(SessionBootActivityLevel::Failure),
            ChromeInk::Failure
        );
    }

    #[test]
    fn notice_clauses_split_on_sentences_and_keep_versions_and_paths_whole() {
        assert_eq!(
            notice_clauses(
                "Counts are on. Code is never collected. See docs/T.md",
                &SENTENCE_MARKS
            ),
            vec![
                "Counts are on.",
                "Code is never collected.",
                "See docs/T.md"
            ]
        );
        assert_eq!(
            notice_clauses("Updated to 0.9.11 from 0.9.10", &SENTENCE_MARKS),
            vec!["Updated to 0.9.11 from 0.9.10"]
        );
        // Full-width stops carry no trailing space, so they break on sight.
        assert_eq!(
            notice_clauses(
                "匿名の利用回数は有効です。会話とコードは収集されません。",
                &SENTENCE_MARKS
            ),
            vec![
                "匿名の利用回数は有効です。",
                "会話とコードは収集されません。"
            ]
        );
        // A colon inside a URL is not a joint.
        assert_eq!(
            notice_clauses("Docs: https://example.test/x", &CLAUSE_MARKS),
            vec!["Docs:", "https://example.test/x"]
        );
    }

    #[test]
    fn shed_clauses_rejoin_without_a_latin_space_after_a_full_width_stop() {
        const JA: &str = "匿名の利用状況集計はオンです。会話やコードは一切収集しません。/settings で変更できます。スキーマ: docs/TELEMETRY.md";
        let clauses = notice_clauses(JA, &SENTENCE_MARKS);
        let joined = join_while_fitting(&clauses, 200).expect("fits");
        assert!(!joined.contains("。 "), "{joined:?}");
        assert!(joined.ends_with("docs/TELEMETRY.md"), "{joined:?}");
    }

    #[test]
    fn a_notice_sheds_whole_clauses_and_never_dangles() {
        const NOTICE: &str = "Anonymous usage counts are on. Conversations and code are never collected. Change this in /settings; schema: docs/TELEMETRY.md";
        assert_eq!(fit_notice(NOTICE, 200).as_deref(), Some(NOTICE));
        assert_eq!(
            fit_notice(NOTICE, 80).as_deref(),
            Some("Anonymous usage counts are on. Conversations and code are never collected.")
        );
        assert_eq!(
            fit_notice(NOTICE, 40).as_deref(),
            Some("Anonymous usage counts are on.")
        );
        assert_eq!(fit_notice("   ", 40), None);
    }

    /// The failure this caught: a one-sentence warning longer than the row
    /// used to have no sentence joint to shed at, so the rail dropped the
    /// whole warning. Inner joints are the fallback, and the phrase that
    /// survives never ends on a `:` or `;` — that mark says "more is coming"
    /// as loudly as an ellipsis does.
    #[test]
    fn a_clause_less_warning_sheds_at_inner_joints_rather_than_vanishing() {
        const WARNING: &str =
            "Auto-denied exec_shell: denied earlier; restart Codewhale to re-enable it.";
        assert_eq!(fit_notice(WARNING, 120).as_deref(), Some(WARNING));
        assert_eq!(
            fit_notice(WARNING, 60).as_deref(),
            Some("Auto-denied exec_shell: denied earlier")
        );
        assert_eq!(
            fit_notice(WARNING, 30).as_deref(),
            Some("Auto-denied exec_shell")
        );
    }

    #[test]
    fn session_metrics_strip_is_on_by_default() {
        assert!(
            crate::config::StatusItem::default_footer()
                .contains(&crate::config::StatusItem::SessionMetrics)
        );
        assert_eq!(
            crate::config::StatusItem::from_key("session_metrics"),
            Some(crate::config::StatusItem::SessionMetrics)
        );
    }

    /// The keys legend (the merged footer's right side) advertises chords
    /// you cannot discover any other way; a slash command announces itself
    /// the moment you type `/`, so it must not pay columns to advertise
    /// itself. Ported from the identity band to its live consumer.
    #[test]
    fn the_keys_legend_advertises_chords_not_slash_commands() {
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        for tier in [ShellTier::Compact, ShellTier::Normal, ShellTier::Wide] {
            let legend = keys_legend(&app, tier, ShellPhase::Idle).into_owned();
            assert!(
                legend.contains("keys") || legend.contains('?'),
                "{tier:?}: {legend}"
            );
            assert!(!legend.contains("/context"), "{tier:?}: {legend}");
        }
        // Live phases keep the row quiet — the legend stands down, not clips.
        assert_eq!(
            keys_legend(&app, ShellTier::Wide, ShellPhase::Working),
            Cow::Borrowed("")
        );
    }

    /// The route identity (the topbar Model segment's value) sheds whole
    /// fields — provider first, then the effort label — and stands down
    /// entirely rather than clip a model name. Ported from the identity
    /// band to `route_identity_fields`, the live shedding authority the
    /// topbar calls with the same budget rule.
    #[test]
    fn route_identity_sheds_qualifiers_before_it_would_clip_a_model_name() {
        let model = "deepseek-v4-flash-preview-2026-05-01";
        let mut app = App::new(
            TuiOptions {
                model: model.to_string(),
                ..crate::test_support::test_tui_options(PathBuf::from("."))
            },
            &Config::default(),
        );
        app.ui_locale = crate::localization::Locale::En;

        // The topbar's own budget rule (ui/frame.rs): width minus the brand
        // lockup, meter, and clock floor, never below 24.
        let topbar_budget = |width: u16| (usize::from(width)).saturating_sub(60).max(24);
        let fields = |width: u16| {
            route_identity_fields(
                &app,
                ShellTier::for_chrome_width(width),
                topbar_budget(width),
            )
        };

        let wide = fields(140).expect("wide budget keeps the route");
        assert!(
            wide.iter().any(|f| f.contains("DeepSeek")) && wide.iter().any(|f| f == model),
            "{wide:?}"
        );

        // Below the group's width the provider sheds first; the model stays
        // whole or the whole group stands down — never a clipped name.
        for width in [30u16, 34, 40, 46, 50, 60] {
            let shed = fields(width).unwrap_or_default();
            for field in &shed {
                assert!(
                    !field.contains('…'),
                    "{width} dangled a clipped field: {shed:?}"
                );
            }
            if shed.iter().any(|f| f.contains("deepseek-v4-flash-p")) {
                assert!(
                    shed.iter().any(|f| f == model),
                    "{width} clipped the model name: {shed:?}"
                );
            }
        }
    }

    /// A named custom route can carry a long provider identity next to a
    /// long model id; whole fields shed (provider first, effort label next)
    /// and neither name is ever clipped. Ported from the identity band to
    /// `route_identity_fields`.
    #[test]
    fn long_custom_route_names_shed_whole_fields_across_width_tiers() {
        let model = "deepseek-v4-flash-vision-preview-2026-08-01";
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        app.set_provider_identity(
            crate::config::ApiProvider::Custom,
            "acme-research-gateway-eu-central",
        );
        app.model = model.to_string();

        let topbar_budget = |width: u16| (usize::from(width)).saturating_sub(60).max(24);
        for width in [30u16, 40, 50, 60, 70, 80, 160] {
            let shed = route_identity_fields(
                &app,
                ShellTier::for_chrome_width(width),
                topbar_budget(width),
            )
            .unwrap_or_default();
            for field in &shed {
                assert!(
                    !field.contains('…'),
                    "{width} dangled a clipped field: {shed:?}"
                );
            }
            if shed.iter().any(|f| f.contains("deepseek-v4-flash-vision")) {
                assert!(
                    shed.iter().any(|f| f == model),
                    "{width} clipped the model name: {shed:?}"
                );
            }
            assert!(
                !shed.iter().any(|f| f.contains("acme-research-gateway-eu-c")
                    && f != "acme-research-gateway-eu-central"),
                "{width} clipped the provider name: {shed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tideline merged footer (spec §3 slots 6+8 merged, §5a "Footer", §5e depth
// line): one band — phase·cost on the left, depth line·keys on the right.
// Wired into `ui/frame.rs` as the shell's single footer row: the classic
// activity band (slot 6) and identity band (slot 8) collapsed into it, with
// the old header's mode/permission chips carried in the left half per §3.
//
// Motion contract (spec §5e): the echolocation chip renders its still frame
// `<·>` — the animated family is the landing slice's job through the 420 ms
// heartbeat. The depth line is a hand-rolled span builder, never `Gauge`,
// and changes only when the token count changes (no private clock).

/// Depth-line cells: 9 rising ramp cells then wave cells for open water,
/// plus the percentage — always ≤16 cells in the footer right.
const DEPTH_CELLS: usize = 9;
/// Ramp glyphs for the filled prefix (matches the spec's `▁▂▄▆` rise).
const DEPTH_RAMP: [&str; 4] = ["▁", "▂", "▄", "▆"];
/// Wave cell for unfilled depth (open water).
const DEPTH_WAVE: &str = "∿";
/// The depth cap warning at ≥80% (spec §5a/§5e).
const DEPTH_WARN: &str = "surface soon — /compact";

/// What the caller owes the merged footer. All injected, deterministic.
pub struct TidelineFooter<'a> {
    pub theme: &'a crate::palette::UiTheme,
    /// Phase word, e.g. `thinking` / `surfaced` / `idle`.
    pub phase_word: &'a str,
    /// Per-phase ink (the caller maps phase → ChromeInk; §5a "per-phase ink").
    pub phase_ink: crate::palette::ChromeInk,
    /// Live detail (`1m 15s`, `×3`), None when idle.
    pub live_detail: Option<&'a str>,
    /// Cost ledger label, e.g. `$0.42 · 61K tok`.
    pub cost_label: &'a str,
    /// Context window percentage 0–100 (depth line source).
    pub context_percent: u8,
    /// Key legend, e.g. `Enter send · Ctrl+K clear · ? help`.
    pub keys_legend: &'a str,
    /// Mode chip (`act` / `plan` / `operate`) in its Policy ink — the old
    /// header's leftmost posture word, moved into the footer per spec §3.
    pub mode_chip: Option<(&'a str, crate::palette::ChromeInk)>,
    /// Permission chip (`ask` / `auto review` / `full access`, plus the
    /// filesystem scope notice when it deviates) in its Permission ink.
    pub permission_chip: Option<(&'a str, crate::palette::ChromeInk)>,
    /// Urgent session notice (status toast / boot activity chip) that owns the
    /// right-hand keys slot while it is live.
    pub notice: Option<(&'a str, crate::palette::ChromeInk)>,
    pub ascii_safe: bool,
}

impl<'a> TidelineFooter<'a> {
    #[must_use]
    pub fn new(
        theme: &'a crate::palette::UiTheme,
        phase_word: &'a str,
        phase_ink: crate::palette::ChromeInk,
        cost_label: &'a str,
        context_percent: u8,
        keys_legend: &'a str,
    ) -> Self {
        Self {
            theme,
            phase_word,
            phase_ink,
            live_detail: None,
            cost_label,
            context_percent,
            keys_legend,
            mode_chip: None,
            permission_chip: None,
            notice: None,
            ascii_safe: false,
        }
    }

    #[must_use]
    pub fn live_detail(mut self, detail: Option<&'a str>) -> Self {
        self.live_detail = detail;
        self
    }

    #[must_use]
    pub fn mode_chip(mut self, chip: Option<(&'a str, crate::palette::ChromeInk)>) -> Self {
        self.mode_chip = chip;
        self
    }

    #[must_use]
    pub fn permission_chip(mut self, chip: Option<(&'a str, crate::palette::ChromeInk)>) -> Self {
        self.permission_chip = chip;
        self
    }

    #[must_use]
    pub fn notice(mut self, notice: Option<(&'a str, crate::palette::ChromeInk)>) -> Self {
        self.notice = notice;
        self
    }

    #[must_use]
    pub fn ascii_safe(mut self, ascii_safe: bool) -> Self {
        self.ascii_safe = ascii_safe;
        self
    }

    fn sym(&self, glyph: &str) -> String {
        if !self.ascii_safe {
            return glyph.to_string();
        }
        if let Some(fb) = crate::tui::glyphs::ascii_fallback(glyph) {
            return fb.to_string();
        }
        glyph
            .chars()
            .map(|c| {
                crate::tui::glyphs::ascii_fallback(&c.to_string())
                    .map(str::to_string)
                    .unwrap_or_else(|| c.to_string())
            })
            .collect()
    }

    /// The depth sparkline for the current percent: filled prefix on the
    /// `▁▂▄▆` ramp, `∿` waves for open water. Pure function of the count —
    /// it never moves on its own (spec §5e).
    #[must_use]
    pub fn depth_cells(&self) -> String {
        let pct = self.context_percent.clamp(0, 100);
        let filled = (usize::from(pct) * DEPTH_CELLS / 100).min(DEPTH_CELLS);
        let mut out = String::new();
        for i in 0..DEPTH_CELLS {
            if i < filled {
                out.push_str(DEPTH_RAMP[i.min(DEPTH_RAMP.len() - 1)]);
            } else {
                out.push_str(DEPTH_WAVE);
            }
        }
        self.sym(&out)
    }

    /// Depth ink: Info below 80%, Attention at the cap (§5a "80% warn").
    #[must_use]
    pub fn depth_ink(&self) -> crate::palette::ChromeInk {
        depth_ink_for(self.context_percent)
    }
}

/// Shared warn threshold ink rule (mirrors `topbar::meter_ink_for`).
#[must_use]
pub fn depth_ink_for(pct: u8) -> crate::palette::ChromeInk {
    if pct >= 80 {
        crate::palette::ChromeInk::Attention
    } else {
        crate::palette::ChromeInk::Info
    }
}

fn tchrome(theme: &crate::palette::UiTheme, ink: crate::palette::ChromeInk) -> Style {
    crate::palette::grammar::chrome_style(theme, ink)
}

fn tput(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    buf.set_stringn(x, y, text, text.width(), style);
}

/// Paint the merged footer band (spec §5b: `Constraint::Length(1)`).
///
/// Left half: still-frame echolocation chip, phase word, live detail, the
/// cost ledger, then the posture chips (`mode · permission`) the old header
/// carried. Right half, pinned: the depth line + percent, with the trailing
/// slot going to the live notice if one is owed, else the ≥80% cap warning,
/// else the key legend.
pub fn render_tideline_footer(area: Rect, buf: &mut Buffer, footer: &TidelineFooter<'_>) {
    if area.width < 8 || area.height < 1 {
        return;
    }
    let theme = footer.theme;
    let pct = footer.context_percent.clamp(0, 100);
    let warn = pct >= 80;

    // Right block first (spec §5a: depth·keys is pinned right; the left
    // half's cost truncates against whatever the right half claims).
    let depth = footer.depth_cells();
    let pct_text = format!("{pct}%");
    let mut right = format!("{depth} {pct_text}");
    if warn {
        right = format!("{} {right}", footer.sym("▲"));
    }
    let right_base_w = right.width() as u16;
    let warn_text = footer.sym(DEPTH_WARN);
    let keys = footer.sym(footer.keys_legend);
    let notice_text = footer.notice.map(|(text, ink)| (footer.sym(text), ink));
    let depth_ink = footer.depth_ink();

    // Trailing-slot precedence: a live notice outranks the cap warning,
    // which outranks the posture chips, which outrank the key chorus (the
    // classic bands' own rule that identity outranks hints). A notice was
    // clause-fitted at build time, so the whole phrase lands or the band
    // was too narrow for it anyway.
    let extra: (&str, crate::palette::ChromeInk) = if let Some((text, ink)) = &notice_text {
        (text.as_str(), *ink)
    } else if warn {
        (warn_text.as_str(), crate::palette::ChromeInk::Attention)
    } else if trailing_extra_width(footer, area.width) > 0 {
        (keys.as_str(), crate::palette::ChromeInk::MetadataHint)
    } else {
        // The chorus stands down so the posture chips fit beside the depth
        // line; if even that is not enough, the chips stand down below.
        ("", crate::palette::ChromeInk::MetadataHint)
    };
    let right_width = right_base_w + 1 + extra.0.width() as u16 + 1;

    // Left: still-frame echolocation chip + phase word + live detail + cost.
    let chip = footer.sym("<·>");
    let phase = footer.sym(footer.phase_word);
    let cost = footer.sym(footer.cost_label);
    tput(buf, area.x, area.y, &chip, tchrome(theme, footer.phase_ink));
    let mut x = area.x + chip.width() as u16 + 1;
    tput(
        buf,
        x,
        area.y,
        &phase,
        tchrome(theme, footer.phase_ink).add_modifier(Modifier::BOLD),
    );
    x += phase.width() as u16 + 1;
    if let Some(detail) = footer.live_detail {
        let detail = footer.sym(detail);
        tput(
            buf,
            x,
            area.y,
            &detail,
            tchrome(theme, crate::palette::ChromeInk::Metadata),
        );
        x += detail.width() as u16 + 1;
    }
    let left_edge_end = (area.x + area.width).saturating_sub(right_width + 1);
    if x + 2 <= left_edge_end {
        tput(
            buf,
            x,
            area.y,
            "│",
            tchrome(theme, crate::palette::ChromeInk::MetadataDim),
        );
        x += 2;
    }
    if x < left_edge_end && !footer.cost_label.is_empty() {
        let budget = (left_edge_end - x) as usize;
        let cost = truncate_owned(&cost, budget);
        tput(
            buf,
            x,
            area.y,
            &cost,
            tchrome(theme, crate::palette::ChromeInk::MetadataValue),
        );
        x += cost.width() as u16;
    }
    // Posture chips after the cost, each fitting whole or standing down —
    // a clipped posture word is worse than none (the classic header's rule).
    for chip in [footer.mode_chip, footer.permission_chip]
        .into_iter()
        .flatten()
    {
        let text = footer.sym(chip.0);
        let needs = ITEM_SEPARATOR_WIDTH + text.width();
        if x + needs as u16 <= left_edge_end {
            tput(
                buf,
                x,
                area.y,
                ITEM_SEPARATOR,
                tchrome(theme, crate::palette::ChromeInk::MetadataDim),
            );
            tput(
                buf,
                x + ITEM_SEPARATOR_WIDTH as u16,
                area.y,
                &text,
                tchrome(theme, chip.1),
            );
            x += needs as u16;
        }
    }

    // Paint the right block pinned to the area edge.
    let mut sx = (area.x + area.width)
        .saturating_sub(right_width)
        .max(area.x);
    tput(buf, sx, area.y, &right, tchrome(theme, depth_ink));
    sx += right.width() as u16 + 1;
    let budget = (area.x + area.width).saturating_sub(sx) as usize;
    tput(
        buf,
        sx,
        area.y,
        &truncate_owned(extra.0, budget),
        tchrome(theme, extra.1),
    );
}

fn truncate_owned(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// The trailing right-slot's width at this row width, shared by the render
/// and the depth hitbox so the two can never disagree. A live notice or the
/// cap warning keeps its full width; the key chorus keeps its width only
/// while the posture chips still fit beside it — otherwise it stands down
/// and the width is zero.
fn trailing_extra_width(footer: &TidelineFooter<'_>, area_width: u16) -> usize {
    let pct = footer.context_percent.clamp(0, 100);
    let keys_w = footer.sym(footer.keys_legend).width();
    if let Some((text, _)) = footer.notice {
        return footer.sym(text).width();
    }
    if pct >= 80 {
        return footer.sym(DEPTH_WARN).width();
    }
    let posture_w: usize = [footer.mode_chip, footer.permission_chip]
        .into_iter()
        .flatten()
        .map(|(text, _)| ITEM_SEPARATOR_WIDTH + footer.sym(text).width())
        .sum();
    if posture_w == 0 {
        return keys_w;
    }
    // The left half's standing width before the posture chips: chip, phase
    // word, live detail, divider, cost.
    let chip = footer.sym("<·>");
    let phase = footer.sym(footer.phase_word);
    let detail_w = footer
        .live_detail
        .map(|detail| footer.sym(detail).width() + 1)
        .unwrap_or(0);
    let cost = footer.sym(footer.cost_label);
    let prefix_w = chip.width() + 1 + phase.width() + 1 + detail_w + 2 + cost.width();
    let depth = footer.depth_cells();
    let right_base_w = format!("{depth} {pct}%").width();
    let available = usize::from(area_width)
        .saturating_sub(right_base_w + 1 + keys_w + 1)
        .saturating_sub(1);
    if prefix_w + posture_w <= available {
        keys_w
    } else {
        0
    }
}

/// Depth-segment hitbox → context inspector (spec §6). Returns the rect
/// covering the painted depth line + percentage.
#[must_use]
#[allow(dead_code)] // depth-segment click routing (spec §6) is a follow-up slice
pub fn tideline_footer_depth_hitbox(area: Rect, footer: &TidelineFooter<'_>) -> Rect {
    let pct = footer.context_percent.clamp(0, 100);
    let depth = footer.depth_cells();
    let mut right = format!("{depth} {pct}%");
    if pct >= 80 {
        right = format!("{} {right}", footer.sym("▲"));
    }
    // Mirror the render's right-block arithmetic through the shared
    // trailing-width rule, so the rect always matches the painted cells.
    let extra_w = trailing_extra_width(footer, area.width);
    let total = right.width() as u16 + 1 + extra_w as u16 + 1;
    let x = (area.x + area.width).saturating_sub(total).max(area.x);
    Rect {
        x,
        y: area.y,
        width: right.width() as u16,
        height: 1,
    }
}

/// Owned footer facts, built from real `App` state at render time and lent
/// to [`TidelineFooter`] for painting. Every field names the surface it
/// replaced when slots 6+8 merged:
///
/// - `phase_word`/`phase_ink`/`live_detail` — the activity band's phase verb
///   and working detail (same phase machinery, same inks).
/// - `cost_label` — the activity band's cost chip (`cumulative_usage_chip`).
/// - `keys_legend` — the identity band's key chorus plus agent hints, and
///   the activity band's `Esc to interrupt` while a turn is live.
/// - `mode_chip`/`permission_chip` — the old header's posture lockup
///   (`underwater::posture_chips`, same words, same inks).
/// - `notice` — the activity band's status toast, or the compact MCP/plugin
///   boot chip when no toast is live; it owns the trailing right slot while
///   present.
///
/// Session metrics (turns/steps/TTFT/cache) move behind `/cost` per spec §3.
pub(crate) struct TidelineFooterFacts {
    pub phase_word: String,
    pub phase_ink: crate::palette::ChromeInk,
    pub live_detail: Option<String>,
    pub cost_label: String,
    pub context_percent: u8,
    pub keys_legend: String,
    pub mode_chip: Option<(String, crate::palette::ChromeInk)>,
    pub permission_chip: Option<(String, crate::palette::ChromeInk)>,
    pub notice: Option<(String, crate::palette::ChromeInk)>,
}

impl TidelineFooterFacts {
    /// Borrow the facts as the deterministic footer widget's input.
    pub(crate) fn widget<'a>(
        &'a self,
        theme: &'a crate::palette::UiTheme,
        ascii_safe: bool,
    ) -> TidelineFooter<'a> {
        TidelineFooter::new(
            theme,
            &self.phase_word,
            self.phase_ink,
            &self.cost_label,
            self.context_percent,
            &self.keys_legend,
        )
        .live_detail(self.live_detail.as_deref())
        .mode_chip(
            self.mode_chip
                .as_ref()
                .map(|(text, ink)| (text.as_str(), *ink)),
        )
        .permission_chip(
            self.permission_chip
                .as_ref()
                .map(|(text, ink)| (text.as_str(), *ink)),
        )
        .notice(
            self.notice
                .as_ref()
                .map(|(text, ink)| (text.as_str(), *ink)),
        )
        .ascii_safe(ascii_safe)
    }
}

/// Context window percentage for the depth line — the same snapshot the old
/// header's meter and the Tideline topbar's meter read.
pub(crate) fn context_percent_from_app(app: &App) -> u8 {
    crate::tui::ui::context_usage_snapshot(app)
        .map(|(_, _, percent)| percent.round().clamp(0.0, 100.0) as u8)
        .unwrap_or(0)
}

/// Build the merged footer's facts from live `App` state. `width` is the
/// footer row's width — notices clause-shed against it, never dangle.
pub(crate) fn tideline_footer_from_app(app: &mut App, width: u16) -> TidelineFooterFacts {
    let activity = LiveActivity::from_app(app);
    let phase = ShellPhase::from_app_with_activity(app, activity);
    let tier = ShellTier::for_chrome_width(width);
    let (_, phase_label) = phase_marker_with_activity(app, phase, activity);
    let phase_word = phase_label.clone().into_owned();

    let live_detail = matches!(phase, ShellPhase::Working | ShellPhase::Verifying)
        .then(|| working_detail(app, activity))
        .flatten();

    // Same cost chip the classic activity band spent its ledger on.
    let usage_chip = app.cumulative_usage_chip();
    let cost_label = match &usage_chip {
        crate::route_billing::UsageChip::Money(amount) => Some(amount.clone()),
        crate::route_billing::UsageChip::PricedSubtotal { .. }
        | crate::route_billing::UsageChip::Unknown => {
            crate::route_billing::format_usage_chip(&usage_chip)
        }
        _ => None,
    }
    .unwrap_or_default();

    // While a turn is live the band carries the interrupt affordance the
    // activity band used to render beside the verb; idle keeps the chorus.
    let legend = if tier != ShellTier::Compact
        && matches!(phase, ShellPhase::Working | ShellPhase::Verifying)
    {
        tr(app.ui_locale, MessageId::FooterHintEscInterrupt).into_owned()
    } else {
        keys_legend(app, tier, phase).into_owned()
    };

    let (mode_chip, permission_chip) = crate::tui::underwater::posture_chips(app);
    let map_chip = |chip: Option<(Cow<'static, str>, crate::palette::ChromeInk)>| {
        chip.map(|(text, ink)| (text.into_owned(), ink))
    };

    // The notice: the live status toast if one is owed, else the compact MCP
    // or plugin boot chip. A slow optional server must not look like a hung
    // turn, while plugin diagnostics stay available through `/plugins` rather
    // than taking rows from the transcript. Clause-shed against half the row
    // — the depth line owns the other half.
    let notice_budget = (usize::from(width) / 2).max(8);
    let notice = selected_notice(app.active_status_toast(), phase, &phase_word)
        .map(|(text, ink, _urgent)| (text, ink))
        .or_else(|| {
            let boot = crate::tui::session_boot::SessionBootSurface::from_app(app);
            boot.activity_notice(app.ui_locale, notice_budget)
                .map(|chip| (chip.text, boot_activity_ink(chip.level)))
        })
        .and_then(|(text, ink)| fit_notice(&text, notice_budget).map(|fitted| (fitted, ink)));

    TidelineFooterFacts {
        phase_word,
        phase_ink: crate::tui::underwater::phase_ink(phase),
        live_detail,
        cost_label,
        context_percent: context_percent_from_app(app),
        keys_legend: legend,
        mode_chip: map_chip(mode_chip),
        permission_chip: map_chip(permission_chip),
        notice,
    }
}

#[cfg(test)]
mod tideline_tests;
