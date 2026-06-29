use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use crate::config::Config;
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::App;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

use super::actions::{
    HotbarActionCategory, HotbarActionMetadata, HotbarArgsBehavior, HotbarRecommendationOptions,
    recommend_hotbar_actions,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarSetupActionRow {
    pub metadata: HotbarActionMetadata,
    pub disabled_reason: Option<String>,
}

impl HotbarSetupActionRow {
    fn status_label(&self, locale: Locale) -> String {
        tr(
            locale,
            if self.disabled_reason.is_some() {
                MessageId::HotbarSetupStatusDisabled
            } else if matches!(self.metadata.args, HotbarArgsBehavior::Required) {
                MessageId::HotbarSetupStatusPrefill
            } else {
                MessageId::HotbarSetupStatusReady
            },
        )
        .into_owned()
    }
}

fn hotbar_setup_source_label(locale: Locale, source: HotbarActionCategory) -> String {
    tr(
        locale,
        match source {
            HotbarActionCategory::App => MessageId::HotbarSetupSourceApp,
            HotbarActionCategory::Slash => MessageId::HotbarSetupSourceSlash,
            HotbarActionCategory::Mcp => MessageId::HotbarSetupSourceMcp,
            HotbarActionCategory::Skill => MessageId::HotbarSetupSourceSkill,
            HotbarActionCategory::Plugin => MessageId::HotbarSetupSourcePlugin,
        },
    )
    .into_owned()
}

fn tr_hotbar_setup(locale: Locale, id: MessageId, replacements: &[(&str, String)]) -> String {
    let mut message = tr(locale, id).into_owned();
    for (placeholder, value) in replacements {
        message = message.replace(placeholder, value);
    }
    message
}

fn hotbar_setup_dirty_label(locale: Locale, is_dirty: bool) -> String {
    tr(
        locale,
        if is_dirty {
            MessageId::HotbarSetupDirtyModified
        } else {
            MessageId::HotbarSetupDirtyClean
        },
    )
    .into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarSetupView {
    locale: Locale,
    sources: Vec<HotbarActionCategory>,
    actions: Vec<HotbarSetupActionRow>,
    selected_source_idx: usize,
    selected_action_idx_by_source: BTreeMap<HotbarActionCategory, usize>,
    selected_slot: u8,
    original_bindings: BTreeMap<u8, codewhale_config::HotbarBindingToml>,
    draft_bindings: BTreeMap<u8, codewhale_config::HotbarBindingToml>,
    recommended_action_ids: BTreeSet<String>,
    validation_errors: Vec<String>,
    help_visible: bool,
}

impl HotbarSetupView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        let mut actions = app
            .hotbar_actions
            .iter()
            .map(|action| {
                let metadata = action.metadata(app.ui_locale);
                let disabled_reason = action.disabled_reason(app);
                HotbarSetupActionRow {
                    metadata,
                    disabled_reason,
                }
            })
            .collect::<Vec<_>>();
        actions.sort_by(|a, b| {
            a.metadata
                .category
                .cmp(&b.metadata.category)
                .then_with(|| {
                    a.metadata
                        .display_name
                        .to_ascii_lowercase()
                        .cmp(&b.metadata.display_name.to_ascii_lowercase())
                })
                .then_with(|| a.metadata.id.cmp(&b.metadata.id))
        });

        let sources = actions
            .iter()
            .map(|row| row.metadata.category)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let recommended_action_ids =
            recommend_hotbar_actions(app, HotbarRecommendationOptions::for_setup_wizard())
                .into_iter()
                .map(|entry| entry.metadata.id)
                .collect::<BTreeSet<_>>();

        let known_action_ids = app
            .hotbar_actions
            .iter()
            .map(|action| action.id())
            .collect::<Vec<_>>();
        let original_bindings = config
            .resolve_hotbar_bindings(&known_action_ids)
            .bindings
            .into_iter()
            .map(|binding| {
                (
                    binding.slot,
                    codewhale_config::HotbarBindingToml {
                        slot: binding.slot,
                        action: binding.action,
                        label: binding.label,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        Self {
            locale: app.ui_locale,
            sources,
            actions,
            selected_source_idx: 0,
            selected_action_idx_by_source: BTreeMap::new(),
            selected_slot: 1,
            draft_bindings: original_bindings.clone(),
            original_bindings,
            recommended_action_ids,
            validation_errors: Vec::new(),
            help_visible: false,
        }
    }

    #[must_use]
    #[cfg(test)]
    pub fn source_categories(&self) -> &[HotbarActionCategory] {
        &self.sources
    }

    #[must_use]
    pub fn selected_source(&self) -> Option<HotbarActionCategory> {
        self.sources.get(self.selected_source_idx).copied()
    }

    #[must_use]
    #[cfg(test)]
    pub fn selected_slot(&self) -> u8 {
        self.selected_slot
    }

    #[must_use]
    pub fn selected_action(&self) -> Option<&HotbarSetupActionRow> {
        let source = self.selected_source()?;
        self.actions_for_source(source)
            .get(self.selected_action_idx(source))
            .copied()
    }

    #[must_use]
    #[cfg(test)]
    pub fn binding_for_slot(&self, slot: u8) -> Option<&codewhale_config::HotbarBindingToml> {
        self.draft_bindings.get(&slot)
    }

    #[must_use]
    #[cfg(test)]
    pub fn checked_action_ids(&self) -> BTreeSet<String> {
        self.draft_bindings
            .values()
            .map(|binding| binding.action.clone())
            .collect()
    }

    #[must_use]
    #[cfg(test)]
    pub fn recommended_action_ids(&self) -> &BTreeSet<String> {
        &self.recommended_action_ids
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.draft_bindings != self.original_bindings
    }

    #[must_use]
    #[cfg(test)]
    pub fn validation_errors(&self) -> &[String] {
        &self.validation_errors
    }

    #[must_use]
    pub fn status_text(&self) -> String {
        if let Some(error) = self.validation_errors.last() {
            return error.clone();
        }
        let dirty = hotbar_setup_dirty_label(self.locale, self.is_dirty());
        let action = self
            .selected_action()
            .map(|row| {
                format!(
                    "{} ({})",
                    row.metadata.display_name,
                    row.status_label(self.locale)
                )
            })
            .unwrap_or_else(|| tr(self.locale, MessageId::HotbarSetupNoAction).into_owned());
        tr_hotbar_setup(
            self.locale,
            MessageId::HotbarSetupStatusLine,
            &[
                ("{slot}", self.selected_slot.to_string()),
                ("{action}", action),
                ("{dirty}", dirty),
            ],
        )
    }

    #[cfg(test)]
    pub fn select_action_by_id(&mut self, action_id: &str) -> bool {
        let Some(row) = self
            .actions
            .iter()
            .find(|row| row.metadata.id == action_id)
            .cloned()
        else {
            return false;
        };
        let Some(source_idx) = self
            .sources
            .iter()
            .position(|source| *source == row.metadata.category)
        else {
            return false;
        };
        self.selected_source_idx = source_idx;
        let index = self
            .actions_for_source(row.metadata.category)
            .iter()
            .position(|candidate| candidate.metadata.id == action_id)
            .unwrap_or(0);
        self.selected_action_idx_by_source
            .insert(row.metadata.category, index);
        self.validation_errors.clear();
        true
    }

    pub fn select_slot(&mut self, slot: u8) -> bool {
        if !(1..=codewhale_config::HOTBAR_SLOT_COUNT).contains(&slot) {
            self.validation_errors = vec![tr_hotbar_setup(
                self.locale,
                MessageId::HotbarSetupSlotOutOfRange,
                &[
                    ("{slot}", slot.to_string()),
                    ("{max}", codewhale_config::HOTBAR_SLOT_COUNT.to_string()),
                ],
            )];
            return false;
        }
        self.selected_slot = slot;
        self.validation_errors.clear();
        true
    }

    pub fn assign_selected_action(&mut self) -> bool {
        let Some(row) = self.selected_action().cloned() else {
            self.validation_errors =
                vec![tr(self.locale, MessageId::HotbarSetupNoActionSelected).into_owned()];
            return false;
        };
        if let Some(reason) = row.disabled_reason {
            self.validation_errors = vec![tr_hotbar_setup(
                self.locale,
                MessageId::HotbarSetupCannotAssign,
                &[
                    ("{action}", row.metadata.display_name),
                    ("{reason}", reason),
                ],
            )];
            return false;
        }
        self.draft_bindings.insert(
            self.selected_slot,
            codewhale_config::HotbarBindingToml {
                slot: self.selected_slot,
                action: row.metadata.id,
                label: None,
            },
        );
        self.validation_errors.clear();
        true
    }

    pub fn toggle_selected_action(&mut self) -> bool {
        let selected_id = self
            .selected_action()
            .map(|row| row.metadata.id.clone())
            .unwrap_or_default();
        if self
            .draft_bindings
            .get(&self.selected_slot)
            .is_some_and(|binding| binding.action == selected_id)
        {
            self.clear_selected_slot();
            true
        } else {
            self.assign_selected_action()
        }
    }

    pub fn clear_selected_slot(&mut self) {
        self.draft_bindings.remove(&self.selected_slot);
        self.validation_errors.clear();
    }

    #[must_use]
    pub fn save_bindings(&self) -> Vec<codewhale_config::HotbarBindingToml> {
        self.draft_bindings.values().cloned().collect()
    }

    fn actions_for_source(&self, source: HotbarActionCategory) -> Vec<&HotbarSetupActionRow> {
        self.actions
            .iter()
            .filter(|row| row.metadata.category == source)
            .collect()
    }

    fn selected_action_idx(&self, source: HotbarActionCategory) -> usize {
        let len = self.actions_for_source(source).len();
        if len == 0 {
            return 0;
        }
        self.selected_action_idx_by_source
            .get(&source)
            .copied()
            .unwrap_or(0)
            .min(len.saturating_sub(1))
    }

    fn set_selected_action_idx(&mut self, source: HotbarActionCategory, idx: usize) {
        let len = self.actions_for_source(source).len();
        if len == 0 {
            self.selected_action_idx_by_source.insert(source, 0);
        } else {
            self.selected_action_idx_by_source
                .insert(source, idx.min(len.saturating_sub(1)));
        }
    }

    fn move_source(&mut self, delta: isize) {
        if self.sources.is_empty() {
            return;
        }
        self.selected_source_idx = wrap_index(self.selected_source_idx, self.sources.len(), delta);
        self.validation_errors.clear();
    }

    fn move_action(&mut self, delta: isize) {
        let Some(source) = self.selected_source() else {
            return;
        };
        let len = self.actions_for_source(source).len();
        if len == 0 {
            return;
        }
        let next = wrap_index(self.selected_action_idx(source), len, delta);
        self.set_selected_action_idx(source, next);
        self.validation_errors.clear();
    }

    fn move_slot(&mut self, delta: isize) {
        let len = usize::from(codewhale_config::HOTBAR_SLOT_COUNT);
        let next = wrap_index(usize::from(self.selected_slot - 1), len, delta) + 1;
        self.selected_slot = u8::try_from(next).expect("hotbar slot fits in u8");
        self.validation_errors.clear();
    }

    fn save_action(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::HotbarSetupSaved {
            bindings: self.save_bindings(),
        })
    }

    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let tabs = self
            .sources
            .iter()
            .map(|source| {
                let label = hotbar_setup_source_label(self.locale, *source);
                if Some(*source) == self.selected_source() {
                    format!("[{label}]")
                } else {
                    label
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(Line::from(vec![Span::styled(
            tabs,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(""));

        let Some(source) = self.selected_source() else {
            lines.push(Line::from(
                tr(self.locale, MessageId::HotbarSetupNoActions).into_owned(),
            ));
            return lines;
        };

        for (idx, row) in self.actions_for_source(source).iter().enumerate() {
            let marker = if idx == self.selected_action_idx(source) {
                ">"
            } else {
                " "
            };
            let checked = if self
                .draft_bindings
                .values()
                .any(|binding| binding.action == row.metadata.id)
            {
                "*"
            } else {
                " "
            };
            let recommended = if self.recommended_action_ids.contains(&row.metadata.id) {
                tr(self.locale, MessageId::HotbarSetupRecommended).into_owned()
            } else {
                String::new()
            };
            let mut text = format!(
                "{marker}{checked} {:<3} {:<20} {:<8} {}",
                recommended,
                row.metadata.display_name,
                row.status_label(self.locale),
                row.metadata.description
            );
            if let Some(reason) = row.disabled_reason.as_deref() {
                text.push_str(" (");
                text.push_str(reason);
                text.push(')');
            }
            lines.push(Line::from(text));
        }

        lines.push(Line::from(""));
        let slots = (1..=codewhale_config::HOTBAR_SLOT_COUNT)
            .map(|slot| {
                let label = self
                    .draft_bindings
                    .get(&slot)
                    .map(|binding| compact_action_id(&binding.action))
                    .unwrap_or_else(|| {
                        tr(self.locale, MessageId::HotbarSetupEmptySlot).into_owned()
                    });
                if slot == self.selected_slot {
                    format!("[{slot}:{label}]")
                } else {
                    format!("{slot}:{label}")
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(Line::from(slots));
        lines.push(Line::from(self.status_text()));
        if self.help_visible {
            lines.push(Line::from(
                tr(self.locale, MessageId::HotbarSetupHelp).into_owned(),
            ));
        }
        lines
    }
}

impl ModalView for HotbarSetupView {
    fn kind(&self) -> ModalKind {
        ModalKind::HotbarSetup
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Char('q') | KeyCode::Char('Q') if key.modifiers.is_empty() => {
                ViewAction::Close
            }
            KeyCode::Tab => {
                self.move_source(1);
                ViewAction::None
            }
            KeyCode::BackTab => {
                self.move_source(-1);
                ViewAction::None
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                self.move_source(-1);
                ViewAction::None
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                self.move_source(1);
                ViewAction::None
            }
            KeyCode::Left => {
                self.move_slot(-1);
                ViewAction::None
            }
            KeyCode::Right => {
                self.move_slot(1);
                ViewAction::None
            }
            KeyCode::Up => {
                self.move_action(-1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_action(1);
                ViewAction::None
            }
            KeyCode::Enter => {
                self.assign_selected_action();
                ViewAction::None
            }
            KeyCode::Char(' ') => {
                self.toggle_selected_action();
                ViewAction::None
            }
            KeyCode::Backspace | KeyCode::Delete => {
                self.clear_selected_slot();
                ViewAction::None
            }
            KeyCode::Char(ch) if ('1'..='8').contains(&ch) => {
                let slot = ch.to_digit(10).expect("digit") as u8;
                self.select_slot(slot);
                ViewAction::None
            }
            KeyCode::Char('s') | KeyCode::Char('S') if key.modifiers.is_empty() => {
                self.save_action()
            }
            KeyCode::Char('?') => {
                self.help_visible = !self.help_visible;
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 118.min(area.width.saturating_sub(4)).max(72);
        let popup_height = 28.min(area.height.saturating_sub(4)).max(12);
        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        Clear.render(popup_area, buf);
        let block = Block::default()
            .title(Line::from(Span::styled(
                tr(self.locale, MessageId::HotbarSetupTitle),
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1)])
            .split(inner);
        Paragraph::new(self.render_lines())
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .render(chunks[0], buf);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn wrap_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = isize::try_from(len).expect("len fits in isize");
    let current = isize::try_from(current).expect("current fits in isize");
    usize::try_from((current + delta).rem_euclid(len)).expect("wrapped index fits")
}

fn compact_action_id(action_id: &str) -> String {
    let suffix = action_id.rsplit('.').next().unwrap_or(action_id);
    crate::tui::ui_text::truncate_line_to_width(suffix, 7)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::localization::{Locale, MessageId, tr};
    use crate::tui::app::TuiOptions;
    use crossterm::event::KeyModifiers;
    use std::path::PathBuf;

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = Locale::En;
        app
    }

    fn test_app_with_locale(locale: Locale) -> App {
        let mut app = test_app();
        app.ui_locale = locale;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn rendered_text(view: &HotbarSetupView) -> String {
        let area = Rect::new(0, 0, 140, 36);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn wizard_sources_follow_registered_action_categories() {
        let app = test_app();
        let view = HotbarSetupView::new(&app, &Config::default());

        assert_eq!(
            view.source_categories(),
            &[HotbarActionCategory::App, HotbarActionCategory::Slash]
        );
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::App));
        assert!(view.recommended_action_ids().contains("mode.agent"));
        assert!(view.checked_action_ids().contains("mode.plan"));
    }

    #[test]
    fn wizard_chrome_uses_non_english_locale() {
        let app = test_app_with_locale(Locale::ZhHant);
        let mut view = HotbarSetupView::new(&app, &Config::default());
        view.clear_selected_slot();
        view.handle_key(key(KeyCode::Char('?')));

        let status = view.status_text();
        assert!(status.contains("槽位 1"), "status was {status:?}");
        assert!(
            status.contains(tr(Locale::ZhHant, MessageId::HotbarSetupDirtyModified).as_ref()),
            "status was {status:?}"
        );
        assert!(!status.contains("slot 1 |"), "status was {status:?}");
        assert!(!status.contains("modified"), "status was {status:?}");

        let rendered = rendered_text(&view);
        let compact_rendered = rendered.replace(' ', "");
        for expected in [
            "Hotbar設定",
            "[應用]",
            "命令",
            "就緒",
            "槽位",
            "來源",
            "分配",
            "儲存",
            "取消",
            "Agent模式",
            "命令面板",
            "切換側邊欄",
        ] {
            assert!(
                compact_rendered.contains(expected),
                "missing {expected:?} in render:\n{rendered}"
            );
        }
        assert!(
            compact_rendered.contains(":空"),
            "missing localized empty slot:\n{rendered}"
        );

        for leaked in [
            "Hotbar setup",
            "Tab/Shift+Tab source",
            "Enter assign",
            "Esc cancel",
            "slot 1 |",
            "ready",
            "modified",
            "empty",
            "Agent mode",
            "Command palette",
            "Toggle sidebar",
            "Switch the conversation",
        ] {
            assert!(
                !rendered.contains(leaked),
                "leaked {leaked:?} in render:\n{rendered}"
            );
        }
    }

    #[test]
    fn wizard_assigns_replaces_toggles_and_clears_slots() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert!(view.select_slot(1));
        assert!(view.select_action_by_id("mode.plan"));
        assert!(view.assign_selected_action());
        assert_eq!(
            view.binding_for_slot(1)
                .map(|binding| binding.action.as_str()),
            Some("mode.plan")
        );

        assert!(view.select_action_by_id("mode.agent"));
        assert!(view.assign_selected_action());
        assert_eq!(
            view.binding_for_slot(1)
                .map(|binding| binding.action.as_str()),
            Some("mode.agent")
        );
        assert!(view.is_dirty());

        assert!(view.toggle_selected_action());
        assert!(view.binding_for_slot(1).is_none());
        view.clear_selected_slot();
        assert!(view.binding_for_slot(1).is_none());
    }

    #[test]
    fn wizard_save_emits_bindings_but_escape_only_closes() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());
        assert!(view.select_slot(8));
        assert!(view.select_action_by_id("sidebar.toggle"));
        assert!(view.assign_selected_action());

        match view.handle_key(key(KeyCode::Char('s'))) {
            ViewAction::EmitAndClose(ViewEvent::HotbarSetupSaved { bindings }) => {
                assert!(
                    bindings
                        .iter()
                        .any(|binding| { binding.slot == 8 && binding.action == "sidebar.toggle" })
                );
            }
            other => panic!("expected HotbarSetupSaved, got {other:?}"),
        }

        let mut view = HotbarSetupView::new(&app, &Config::default());
        assert!(view.select_slot(1));
        assert!(view.select_action_by_id("mode.agent"));
        assert!(view.assign_selected_action());
        assert!(matches!(
            view.handle_key(key(KeyCode::Esc)),
            ViewAction::Close
        ));
    }

    #[test]
    fn disabled_actions_are_visible_but_not_assignable() {
        let mut app = test_app();
        app.auto_model = true;
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert!(view.select_slot(2));
        assert!(view.select_action_by_id("reasoning.cycle"));
        assert!(!view.assign_selected_action());

        assert_ne!(
            view.binding_for_slot(2)
                .map(|binding| binding.action.as_str()),
            Some("reasoning.cycle")
        );
        assert!(
            view.validation_errors()
                .last()
                .is_some_and(|error| error.contains("cannot be assigned"))
        );
        assert!(view.status_text().contains("cannot be assigned"));
    }

    #[test]
    fn args_required_slash_actions_are_visible_and_assignable_as_prefill() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert!(view.select_action_by_id("slash.rename"));
        assert!(
            view.status_text().contains("prefill"),
            "required-arg commands must be labeled as prefill actions"
        );
        assert!(view.select_slot(3));
        assert!(view.assign_selected_action());

        assert_eq!(
            view.binding_for_slot(3)
                .map(|binding| binding.action.as_str()),
            Some("slash.rename")
        );
    }

    #[test]
    fn keyboard_controls_navigate_source_action_and_slot() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert_eq!(view.selected_source(), Some(HotbarActionCategory::App));
        view.handle_key(key(KeyCode::Tab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::Slash));
        view.handle_key(key(KeyCode::BackTab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::App));

        let first = view
            .selected_action()
            .map(|row| row.metadata.id.clone())
            .expect("first action");
        view.handle_key(key(KeyCode::Down));
        let second = view
            .selected_action()
            .map(|row| row.metadata.id.clone())
            .expect("second action");
        assert_ne!(first, second);

        view.handle_key(key(KeyCode::Char('8')));
        assert_eq!(view.selected_slot(), 8);
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.selected_slot(), 7);
    }
}
