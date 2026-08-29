//! In-context plugin reminders: prompt matching, live composer CTA, and idle
//! catalog polling.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Widget};
use unicode_width::UnicodeWidthStr;

use crate::localization::{MessageId, tr};
use crate::plugins::recommend::{
    PluginNextStep, RecommendOptions, load_marketplace_candidates, match_plugin_for_draft,
    recommend_plugins_for_task,
};
use crate::tui::app::{App, StatusToastLevel};

const MAX_PROMPT_SUGGESTS_PER_SESSION: u8 = 2;
const CATALOG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const CTA_DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCtaPhase {
    Hidden,
    Matched { name: String, command: String },
}

impl PluginCtaPhase {
    #[must_use]
    pub fn is_visible(&self) -> bool {
        matches!(self, Self::Matched { .. })
    }

    #[must_use]
    pub fn matched_name(&self) -> Option<&str> {
        match self {
            Self::Hidden => None,
            Self::Matched { name, .. } => Some(name.as_str()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginCtaState {
    pub phase: PluginCtaPhase,
    pub dismissed: HashSet<String>,
    debounce_at: Option<Instant>,
    last_draft: String,
}

impl Default for PluginCtaState {
    fn default() -> Self {
        Self {
            phase: PluginCtaPhase::Hidden,
            dismissed: HashSet::new(),
            debounce_at: None,
            last_draft: String::new(),
        }
    }
}

impl App {
    /// When the user sends a task that matches an installed-but-idle plugin
    /// or a locally added marketplace candidate, toast the next review step
    /// once. Never installs, trusts, or enables anything.
    pub fn maybe_nudge_plugin_for_prompt(&mut self, input: &str) -> bool {
        if self.plugin_prompt_suggest_count >= MAX_PROMPT_SUGGESTS_PER_SESSION {
            return false;
        }
        let marketplace = load_marketplace_candidates(self.plugin_registry.state_path());
        let recommendations = recommend_plugins_for_task(
            input,
            self.plugin_registry.as_ref(),
            &marketplace,
            RecommendOptions::proactive(),
        );
        let Some(recommendation) = recommendations.into_iter().next() else {
            return false;
        };
        if self
            .plugin_prompt_suggest_names
            .contains(&recommendation.name)
        {
            return false;
        }
        let message_id = match recommendation.next_step {
            PluginNextStep::Trust => MessageId::PluginPromptSuggestTrust,
            PluginNextStep::Enable => MessageId::PluginPromptSuggestEnable,
            PluginNextStep::MarketplaceInstall { .. } => MessageId::PluginPromptSuggestMarketplace,
            PluginNextStep::AlreadyActive
            | PluginNextStep::Inspect
            | PluginNextStep::SourceInstall { .. } => return false,
        };
        let mut message = tr(self.ui_locale, message_id).replace("{name}", &recommendation.name);
        if let PluginNextStep::MarketplaceInstall { catalog_id } = &recommendation.next_step {
            message = message.replace("{catalog}", catalog_id);
        }
        self.plugin_prompt_suggest_names.insert(recommendation.name);
        self.plugin_prompt_suggest_count = self.plugin_prompt_suggest_count.saturating_add(1);
        self.push_status_toast(message, StatusToastLevel::Info, Some(8_000));
        true
    }

    /// Cheap idle poll so on-disk plugin changes can surface between turns,
    /// not only on send. Fingerprints directories; never auto-reloads.
    pub fn maybe_poll_plugin_catalog_idle(&mut self) {
        let now = Instant::now();
        if self
            .last_plugin_catalog_poll
            .is_some_and(|seen| now.duration_since(seen) < CATALOG_POLL_INTERVAL)
        {
            return;
        }
        self.last_plugin_catalog_poll = Some(now);
        if let Some(message) = crate::plugins::plugin_reload_nudge(
            self.plugin_registry.as_ref(),
            &mut self.plugin_reload_nudge_stamp,
        ) {
            self.push_status_toast(message, StatusToastLevel::Warning, Some(8_000));
            self.needs_redraw = true;
        }
    }

    /// Arm a short debounce whenever the composer draft changes.
    pub fn notify_plugin_cta_text_changed(&mut self) {
        if self.input == self.plugin_cta.last_draft {
            return;
        }
        self.plugin_cta.last_draft = self.input.clone();
        self.plugin_cta.debounce_at = Some(Instant::now() + CTA_DEBOUNCE);
    }

    /// Recompute the live CTA after the debounce window. One match at a
    /// time; already-active plugins stay hidden; a dismissed name stays
    /// dismissed for the rest of this session. Never auto-installs.
    pub fn handle_plugin_cta_debounce_expired(&mut self) {
        self.plugin_cta.debounce_at = None;
        self.plugin_cta.last_draft = self.input.clone();
        let marketplace = load_marketplace_candidates(self.plugin_registry.state_path());
        let Some(matched) =
            match_plugin_for_draft(&self.input, self.plugin_registry.as_ref(), &marketplace)
        else {
            if self.plugin_cta.phase.is_visible() {
                self.plugin_cta.phase = PluginCtaPhase::Hidden;
                self.needs_redraw = true;
            }
            return;
        };
        if self
            .plugin_cta
            .dismissed
            .contains(&matched.name.to_ascii_lowercase())
        {
            if self.plugin_cta.phase.is_visible() {
                self.plugin_cta.phase = PluginCtaPhase::Hidden;
                self.needs_redraw = true;
            }
            return;
        }
        let command = matched.command();
        let new_phase = PluginCtaPhase::Matched {
            name: matched.name,
            command,
        };
        if self.plugin_cta.phase != new_phase {
            self.plugin_cta.phase = new_phase;
            self.needs_redraw = true;
        }
    }

    /// Poll draft changes and fire the CTA debounce without a dedicated timer
    /// task. The event loop already ticks this often.
    pub fn maybe_poll_plugin_cta(&mut self) {
        self.notify_plugin_cta_text_changed();
        let Some(at) = self.plugin_cta.debounce_at else {
            return;
        };
        if Instant::now() < at {
            return;
        }
        self.handle_plugin_cta_debounce_expired();
    }

    #[must_use]
    pub fn plugin_cta_row_height(&self) -> u16 {
        u16::from(self.plugin_cta.phase.is_visible())
    }

    /// Hide the CTA for this plugin name for the rest of the session.
    pub fn dismiss_plugin_cta(&mut self) -> bool {
        let Some(name) = self.plugin_cta.phase.matched_name().map(str::to_string) else {
            return false;
        };
        self.plugin_cta.dismissed.insert(name.to_ascii_lowercase());
        self.plugin_cta.phase = PluginCtaPhase::Hidden;
        self.needs_redraw = true;
        true
    }

    /// Human-initiated review: return the slash command so the TUI can run
    /// the existing `/plugin trust` / marketplace-install / `/plugin install`
    /// path. Never runs it here.
    #[must_use]
    pub fn accept_plugin_cta_command(&mut self) -> Option<String> {
        let (command, name) = match &self.plugin_cta.phase {
            PluginCtaPhase::Matched { command, name } => (command.clone(), name.clone()),
            PluginCtaPhase::Hidden => return None,
        };
        self.plugin_cta.dismissed.insert(name.to_ascii_lowercase());
        self.plugin_cta.phase = PluginCtaPhase::Hidden;
        self.needs_redraw = true;
        Some(command)
    }

    /// Model-requested review: show the live CTA and a toast. Does not run
    /// the command, so nothing is installed, trusted, or enabled.
    pub fn surface_plugin_review_request(&mut self, name: &str, command: &str) {
        if name.trim().is_empty() || command.trim().is_empty() {
            return;
        }
        self.plugin_cta.phase = PluginCtaPhase::Matched {
            name: name.to_string(),
            command: command.to_string(),
        };
        self.push_status_toast(command.to_string(), StatusToastLevel::Info, Some(8_000));
        self.needs_redraw = true;
    }
}

/// Draw the one-line live CTA above the composer. No-op when hidden.
pub fn draw_plugin_cta(app: &mut App, area: Rect, buf: &mut Buffer) {
    app.viewport.last_plugin_cta_area = None;
    app.viewport.last_plugin_cta_review_area = None;
    app.viewport.last_plugin_cta_dismiss_area = None;
    let PluginCtaPhase::Matched { name, .. } = &app.plugin_cta.phase else {
        return;
    };
    let name = name.clone();
    if area.height == 0 || area.width == 0 {
        return;
    }
    let prompt = tr(app.ui_locale, MessageId::PluginCtaInstallPrompt).replace("{name}", &name);
    let review = tr(app.ui_locale, MessageId::PluginCtaReview);
    let dismiss = tr(app.ui_locale, MessageId::PluginCtaDismiss);
    let review_label = format!("[{review}]");
    let dismiss_label = format!("[{dismiss}]");
    let review_w = review_label.width() as u16;
    let dismiss_w = dismiss_label.width() as u16;
    let gap = 1u16;
    let right_w = review_w.saturating_add(gap).saturating_add(dismiss_w);
    let bg = Style::default().bg(app.ui_theme.composer_bg);
    Block::default().style(bg).render(area, buf);
    let left_budget = if area.width > right_w.saturating_add(1) {
        area.width - right_w - 1
    } else {
        area.width
    };
    let left = Line::from(vec![Span::styled(
        prompt,
        Style::default().fg(app.ui_theme.text_hint),
    )]);
    buf.set_line(area.x, area.y, &left, left_budget);
    if area.width <= right_w {
        app.viewport.last_plugin_cta_area = Some(area);
        return;
    }
    let review_x = area.x + area.width - right_w;
    let dismiss_x = review_x + review_w + gap;
    buf.set_stringn(
        review_x,
        area.y,
        &review_label,
        usize::from(review_w),
        Style::default().fg(app.ui_theme.accent_action),
    );
    buf.set_stringn(
        dismiss_x,
        area.y,
        &dismiss_label,
        usize::from(dismiss_w),
        Style::default().fg(app.ui_theme.text_hint),
    );
    app.viewport.last_plugin_cta_area = Some(area);
    app.viewport.last_plugin_cta_review_area = Some(Rect::new(review_x, area.y, review_w, 1));
    app.viewport.last_plugin_cta_dismiss_area = Some(Rect::new(dismiss_x, area.y, dismiss_w, 1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::tui::app::TuiOptions;
    use std::fs;
    use tempfile::TempDir;

    fn app_with_supabase_plugin() -> (App, TempDir, crate::test_support::EnvVarGuard) {
        let root = TempDir::new().unwrap();
        let home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
        let bundle = root.path().join(".codewhale/plugins/supabase");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("plugin.toml"),
            "schema_version = 1\n[plugin]\nname = \"supabase\"\nversion = \"1.0.0\"\ndescription = \"Hosted Postgres and auth\"\nkeywords = [\"supabase\"]\n",
        )
        .unwrap();
        let temp = TempDir::new().unwrap();
        let options = TuiOptions {
            config_path: Some(temp.path().join("config.toml")),
            skills_dir: temp.path().join("skills"),
            memory_path: temp.path().join("memory.md"),
            notes_path: temp.path().join("notes.txt"),
            mcp_config_path: temp.path().join("mcp.json"),
            ..crate::test_support::test_tui_options(root.path())
        };
        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        let registry = discovery.registry_for_workspace(root.path());
        let mut app = App::new_with_plugin_registry(options, &Config::default(), registry);
        app.ui_locale = Locale::En;
        (app, root, home)
    }

    #[test]
    fn sending_a_supabase_prompt_toasts_trust_for_an_installed_idle_plugin() {
        let _lock = crate::test_support::lock_test_env();
        let (mut app, _root, _home) = app_with_supabase_plugin();

        assert!(app.maybe_nudge_plugin_for_prompt("add supabase auth to login"));
        assert_eq!(app.status_toasts.len(), 1);
        assert!(
            app.status_toasts[0].text.contains("/plugin trust supabase"),
            "{}",
            app.status_toasts[0].text
        );
        assert!(!app.maybe_nudge_plugin_for_prompt("add supabase auth to login"));
    }

    #[test]
    fn live_cta_shows_for_a_matching_idle_plugin() {
        let _lock = crate::test_support::lock_test_env();
        let (mut app, _root, _home) = app_with_supabase_plugin();
        app.input = "add supabase auth to login".to_string();
        app.handle_plugin_cta_debounce_expired();
        assert_eq!(
            app.plugin_cta.phase.matched_name(),
            Some("supabase"),
            "{:?}",
            app.plugin_cta.phase
        );
        assert_eq!(app.plugin_cta_row_height(), 1);
    }

    #[test]
    fn live_cta_hides_when_the_plugin_is_already_active() {
        let _lock = crate::test_support::lock_test_env();
        let (mut app, _root, _home) = app_with_supabase_plugin();
        let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
        registry.trust("supabase").unwrap();
        registry.enable("supabase").unwrap();
        app.input = "add supabase auth to login".to_string();
        app.handle_plugin_cta_debounce_expired();
        assert!(
            !app.plugin_cta.phase.is_visible(),
            "{:?}",
            app.plugin_cta.phase
        );
    }

    #[test]
    fn live_cta_dismiss_stays_dismissed_for_that_name_this_session() {
        let _lock = crate::test_support::lock_test_env();
        let (mut app, _root, _home) = app_with_supabase_plugin();
        app.input = "add supabase auth to login".to_string();
        app.handle_plugin_cta_debounce_expired();
        assert!(app.dismiss_plugin_cta());
        assert!(!app.plugin_cta.phase.is_visible());
        app.handle_plugin_cta_debounce_expired();
        assert!(
            !app.plugin_cta.phase.is_visible(),
            "dismissed names must not reappear this session: {:?}",
            app.plugin_cta.phase
        );
    }
}
