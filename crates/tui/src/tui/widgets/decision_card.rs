//! Decision-card widget for structured user input.
//!
//! When Brother Whale needs input, it surfaces a decision card: a labelled
//! question followed by numbered options, with the default option highlighted.
//! The user navigates with 1-9 keys (or j/k / Up/Down) and confirms with
//! Enter. Every decision is logged so the user can inspect the choice later.
//!
//! This replaces vague "what should I do?" prompts with a structured choice
//! surface — acceptance criterion from the v0.8.43 truth-surface tracker.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, Widget},
};

use super::renderable::Renderable;
use crate::palette;

/// A single option in a decision card.
#[derive(Debug, Clone)]
pub struct DecisionOption {
    /// Short label for the option (e.g. "Apply the patch").
    pub label: String,
    /// Optional longer description shown below the label.
    pub description: Option<String>,
}

/// A decision card surfacing a structured choice to the user.
#[derive(Debug, Clone)]
pub struct DecisionCard {
    /// The question or prompt the user is answering.
    pub question: String,
    /// The available options. Each is numbered 1..N.
    pub options: Vec<DecisionOption>,
    /// Index into `options` of the default (highlighted) choice.
    pub default_index: usize,
    /// Index of the currently selected option.
    pub selected_index: usize,
    /// Whether the card has been submitted (Enter pressed).
    pub confirmed: bool,
    /// The index that was confirmed, if any.
    pub confirmed_index: Option<usize>,
}

impl DecisionCard {
    pub fn new(question: String, options: Vec<DecisionOption>, default_index: usize) -> Self {
        let default = default_index.min(options.len().saturating_sub(1));
        Self {
            question,
            options,
            default_index: default,
            selected_index: default,
            confirmed: false,
            confirmed_index: None,
        }
    }

    /// Number of options.
    pub fn option_count(&self) -> usize {
        self.options.len()
    }

    /// Move selection up (wrap around).
    pub fn select_prev(&mut self) {
        if self.option_count() == 0 {
            return;
        }
        self.selected_index = self
            .selected_index
            .checked_sub(1)
            .unwrap_or(self.option_count() - 1);
    }

    /// Move selection down (wrap around).
    pub fn select_next(&mut self) {
        if self.option_count() == 0 {
            return;
        }
        self.selected_index = (self.selected_index + 1) % self.option_count();
    }

    /// Select by number key (1-based).
    pub fn select_number(&mut self, n: usize) {
        if n > 0 && n <= self.option_count() {
            self.selected_index = n - 1;
        }
    }

    /// Confirm the current selection.
    pub fn confirm(&mut self) {
        self.confirmed = true;
        self.confirmed_index = Some(self.selected_index);
    }

    /// Get the label of the confirmed option, if any.
    pub fn confirmed_label(&self) -> Option<&str> {
        self.confirmed_index
            .and_then(|i| self.options.get(i))
            .map(|opt| opt.label.as_str())
    }
}

impl Default for DecisionCard {
    fn default() -> Self {
        Self::new(String::new(), Vec::new(), 0)
    }
}

impl Renderable for DecisionCard {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 3 {
            return;
        }

        // Palette constants (not raw RGBs) so the light-mode and community
        // theme remaps in `crate::palette` restyle the card per theme.
        let border_style = Style::default().fg(palette::DEEPSEEK_SKY);
        let question_style = Style::default()
            .fg(palette::TEXT_SOFT)
            .add_modifier(Modifier::BOLD);
        let dim_style = Style::default().fg(palette::TEXT_HINT);
        let selected_style = Style::default()
            .fg(palette::TEXT_ACCENT)
            .add_modifier(Modifier::BOLD);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Decision Required ")
            .title_style(question_style);
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 2 || inner.height < 2 {
            return;
        }

        let mut y = inner.y;

        // Question line
        let question = truncate_to_width(&self.question, inner.width as usize);
        buf.set_string(inner.x, y, &question, question_style);
        y += 1;

        if y >= inner.y + inner.height {
            return;
        }

        // Separator
        let sep = "─".repeat(inner.width as usize);
        buf.set_string(inner.x, y, &sep, dim_style);
        y += 1;

        // Options
        let max_options = (inner.y + inner.height).saturating_sub(y) as usize;
        for (i, option) in self.options.iter().enumerate().take(max_options) {
            if y >= inner.y + inner.height {
                break;
            }

            let num = format!("{}.", i + 1);
            let is_selected = i == self.selected_index;
            let style = if is_selected {
                selected_style
            } else {
                dim_style
            };

            // "1. Label (default)" or "1. Label"
            let mut label = format!("{} {}", num, option.label);
            if i == self.default_index {
                label.push_str(" (default)");
            }
            label = truncate_to_width(&label, inner.width.saturating_sub(1) as usize);

            let prefix = if is_selected { "▸ " } else { "  " };
            let full_label = format!("{prefix}{label}");
            buf.set_string(inner.x, y, &full_label, style);
            y += 1;

            // Description line if present
            if let Some(ref desc) = option.description
                && y < inner.y + inner.height
            {
                let desc = format!(
                    "    {}",
                    truncate_to_width(desc, inner.width.saturating_sub(5) as usize)
                );
                buf.set_string(inner.x, y, &desc, dim_style);
                y += 1;
            }
        }

        // Footer hint
        if y < inner.y + inner.height {
            let hint = "1-9 select  ·  j/k navigate  ·  Enter confirm";
            let hint = truncate_to_width(hint, inner.width as usize);
            buf.set_string(inner.x, y, &hint, dim_style);
        }
    }

    fn desired_height(&self, _width: u16) -> u16 {
        // question + separator + options + footer
        let option_lines: u16 = self
            .options
            .iter()
            .map(|o| if o.description.is_some() { 2 } else { 1 })
            .sum();
        // 2 for borders, 1 question, 1 separator, options, 1 footer
        2 + 1 + 1 + option_lines + 1
    }
}

fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let truncated: String = chars.into_iter().take(max_width - 1).collect();
    format!("{truncated}…")
}
