//! `/provider` picker modal — pick a provider (DeepSeek / NVIDIA NIM /
//! hosted providers / self-hosted providers) and, if it lacks credentials, type the API key
//! inline before completing the switch (#52).
//!
//! The picker is intentionally a single modal with two visible states:
//!
//! 1. **List** — pick a provider; each row shows the active provider arrow
//!    and an "API key configured" / "needs API key" hint. Enter on a
//!    configured provider applies the switch immediately
//!    ([`ViewEvent::ProviderPickerApplied`]). Enter on an un-configured one
//!    transitions the same modal into the key-entry state.
//! 2. **Key entry** — masked input box pre-filled with the provider's
//!    canonical env-var name as a hint. Enter submits
//!    [`ViewEvent::ProviderPickerApiKeySubmitted`], which the UI handler
//!    persists via `save_api_key_for` before switching.
//!
//! Pressing Esc backs out: from key entry returns to the list; from the
//! list closes the modal without changes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use crate::config::{ApiProvider, Config, has_api_key_for};
use crate::palette;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    KeyEntry,
}

pub struct ProviderPickerView {
    providers: Vec<(ApiProvider, bool)>,
    active_provider: ApiProvider,
    selected_idx: usize,
    stage: Stage,
    api_key_input: String,
}

impl ProviderPickerView {
    #[must_use]
    pub fn new(active: ApiProvider, config: &Config) -> Self {
        let providers: Vec<(ApiProvider, bool)> = ApiProvider::all()
            .iter()
            .map(|p| (*p, has_api_key_for(config, *p)))
            .collect();
        let selected_idx = providers
            .iter()
            .position(|(p, _)| *p == active)
            .unwrap_or(0);
        Self {
            providers,
            active_provider: active,
            selected_idx,
            stage: Stage::List,
            api_key_input: String::new(),
        }
    }

    fn move_up(&mut self) {
        if self.selected_idx > 0 {
            self.selected_idx -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.selected_idx + 1 < self.providers.len() {
            self.selected_idx += 1;
        }
    }

    fn selected_provider(&self) -> ApiProvider {
        self.providers[self.selected_idx].0
    }

    fn selected_has_key(&self) -> bool {
        self.providers[self.selected_idx].1
    }

    fn env_var_for(provider: ApiProvider) -> &'static str {
        match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => "DEEPSEEK_API_KEY",
            ApiProvider::NvidiaNim => "NVIDIA_API_KEY",
            ApiProvider::Openai => "OPENAI_API_KEY",
            ApiProvider::Atlascloud => "ATLASCLOUD_API_KEY",
            ApiProvider::Openrouter => "OPENROUTER_API_KEY",
            ApiProvider::Novita => "NOVITA_API_KEY",
            ApiProvider::Fireworks => "FIREWORKS_API_KEY",
            ApiProvider::Sglang => "SGLANG_API_KEY",
            ApiProvider::Vllm => "VLLM_API_KEY",
            ApiProvider::Ollama => "OLLAMA_API_KEY",
        }
    }

    fn provider_hint(provider: ApiProvider, has_key: bool) -> String {
        match provider {
            ApiProvider::Ollama => "self-hosted; defaults to http://localhost:11434".to_string(),
            ApiProvider::Sglang | ApiProvider::Vllm if has_key => {
                "(configured; optional key)".to_string()
            }
            ApiProvider::Sglang | ApiProvider::Vllm => "(optional key)".to_string(),
            _ if has_key => "(configured)".to_string(),
            _ => "(needs API key)".to_string(),
        }
    }

    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        let outer = Block::default()
            .title(Line::from(Span::styled(
                " Provider ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" ↑↓ ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("move "),
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("apply "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("cancel "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default());
        let inner = outer.inner(area);
        outer.render(area, buf);

        let mut lines: Vec<Line> = Vec::with_capacity(self.providers.len());
        for (idx, (provider, has_key)) in self.providers.iter().enumerate() {
            let is_selected = idx == self.selected_idx;
            let is_active = *provider == self.active_provider;
            let arrow = if is_selected { "▸" } else { " " };
            let active_dot = if is_active { " *" } else { "  " };
            let label_style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let hint_style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else if *has_key {
                Style::default().fg(palette::TEXT_MUTED)
            } else {
                Style::default().fg(palette::STATUS_WARNING)
            };
            let hint = Self::provider_hint(*provider, *has_key);
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(arrow, label_style),
                Span::raw(" "),
                Span::styled(provider.display_name().to_string(), label_style),
                Span::styled(active_dot, label_style),
                Span::raw("  "),
                Span::styled(hint, hint_style),
            ]));
        }
        Paragraph::new(lines).render(inner, buf);
    }

    fn render_key_entry(&self, area: Rect, buf: &mut Buffer) {
        let provider = self.selected_provider();
        let outer = Block::default()
            .title(Line::from(Span::styled(
                format!(" API key — {} ", provider.display_name()),
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("save & switch "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("back "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default());
        let inner = outer.inner(area);
        outer.render(area, buf);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);

        let masked = mask_key(&self.api_key_input);
        let display = if masked.is_empty() {
            "(paste key here)".to_string()
        } else {
            masked
        };
        let key_lines = vec![Line::from(vec![
            Span::styled("Key: ", Style::default().fg(palette::TEXT_MUTED)),
            Span::styled(
                display,
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
        Paragraph::new(key_lines).render(layout[0], buf);

        let hint = format!(
            "Or set the {} environment variable and re-open /provider.",
            Self::env_var_for(provider),
        );
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(palette::TEXT_MUTED),
        )))
        .render(layout[1], buf);
    }
}

fn mask_key(input: &str) -> String {
    let trimmed = input.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return String::new();
    }
    if len <= 4 {
        return "*".repeat(len);
    }
    let visible: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}{}", "*".repeat(len - 4), visible)
}

impl ModalView for ProviderPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ProviderPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if self.stage == Stage::KeyEntry {
            let sanitized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if !sanitized.is_empty() {
                self.api_key_input.push_str(&sanitized);
            }
            true
        } else {
            false
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.stage {
            Stage::List => match key.code {
                KeyCode::Esc => ViewAction::Close,
                KeyCode::Up => {
                    self.move_up();
                    ViewAction::None
                }
                KeyCode::Down => {
                    self.move_down();
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let provider = self.selected_provider();
                    if self.selected_has_key() {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider })
                    } else {
                        self.stage = Stage::KeyEntry;
                        self.api_key_input.clear();
                        ViewAction::None
                    }
                }
                _ => ViewAction::None,
            },
            Stage::KeyEntry => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    self.api_key_input.clear();
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    self.api_key_input.pop();
                    ViewAction::None
                }
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.api_key_input.pop();
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let key = self.api_key_input.trim().to_string();
                    if key.is_empty() {
                        // Stay in key-entry; the user can press Esc to abort.
                        ViewAction::None
                    } else {
                        let provider = self.selected_provider();
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                            provider,
                            api_key: key,
                        })
                    }
                }
                KeyCode::Char(c) => {
                    // Reject ASCII whitespace so a stray space/tab doesn't slip
                    // into a credential; bracketed paste happens via the input
                    // path that already trims on submit.
                    if !c.is_whitespace() {
                        self.api_key_input.push(c);
                    }
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 64.min(area.width.saturating_sub(4)).max(40);
        let popup_height = match self.stage {
            Stage::List => 12,
            Stage::KeyEntry => 10,
        }
        .min(area.height.saturating_sub(4))
        .max(8);
        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        match self.stage {
            Stage::List => self.render_list(popup_area, buf),
            Stage::KeyEntry => self.render_key_entry(popup_area, buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn move_to_provider(picker: &mut ProviderPickerView, provider: ApiProvider) {
        let max_steps = picker.providers.len();
        for _ in 0..max_steps {
            if picker.selected_provider() == provider {
                return;
            }
            picker.handle_key(key(KeyCode::Down));
        }
        panic!("provider {provider:?} not found in picker");
    }

    #[test]
    fn picker_lists_all_providers() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let names: Vec<_> = picker
            .providers
            .iter()
            .map(|(p, _)| p.display_name())
            .collect();
        assert_eq!(
            names,
            vec![
                "DeepSeek",
                "NVIDIA NIM",
                "OpenAI-compatible",
                "AtlasCloud",
                "OpenRouter",
                "Novita AI",
                "Fireworks AI",
                "SGLang",
                "vLLM",
                "Ollama"
            ]
        );
    }

    #[test]
    fn ollama_is_selectable_without_key() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Ollama);
        assert_eq!(picker.selected_provider(), ApiProvider::Ollama);
        assert!(picker.selected_has_key());
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider }) => {
                assert_eq!(provider, ApiProvider::Ollama);
            }
            other => panic!("expected ProviderPickerApplied, got {other:?}"),
        }
    }

    #[test]
    fn picker_marks_active_provider_as_initial_selection() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Openrouter, &config);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        assert_eq!(picker.active_provider, ApiProvider::Openrouter);
    }

    #[test]
    fn enter_with_no_key_transitions_to_key_entry_stage() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Move to OpenRouter, which has no key in default config.
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        let action = picker.handle_key(key(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::KeyEntry);
    }

    #[test]
    fn enter_with_existing_key_emits_apply_and_closes() {
        let config = Config {
            api_key: Some("existing-deepseek-key".to_string()),
            ..Config::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::NvidiaNim, &config);
        // Move up twice to DeepSeek (index 0), which has a key from the config.
        picker.handle_key(key(KeyCode::Up));
        picker.handle_key(key(KeyCode::Up));
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider }) => {
                assert_eq!(provider, ApiProvider::Deepseek);
            }
            other => panic!("expected ProviderPickerApplied, got {other:?}"),
        }
    }

    #[test]
    fn key_entry_enter_submits_after_typing() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Navigate to Novita and trigger key entry.
        move_to_provider(&mut picker, ApiProvider::Novita);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        for c in "novita-key".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                provider,
                api_key,
            }) => {
                assert_eq!(provider, ApiProvider::Novita);
                assert_eq!(api_key, "novita-key");
            }
            other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
        }
    }

    #[test]
    fn key_entry_esc_returns_to_list_without_emitting() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        picker.handle_key(key(KeyCode::Char('a')));
        let action = picker.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::List);
        assert!(picker.api_key_input.is_empty());
    }

    #[test]
    fn list_esc_closes_without_emitting() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let action = picker.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn key_entry_strips_whitespace_chars() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        for c in "abc def".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.api_key_input, "abcdef");
    }
}
