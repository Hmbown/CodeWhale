//! Whale Teams — the Signal Cut whale identities in the terminal.
//!
//! CWC's "Whale Teams / Signal Cut" identity (2026-08-15) gives each agent
//! role a species-led whale and every whale one of six runtime states. This
//! module carries that identity into the TUI as authored glyph art: nothing
//! here is converted from the CWC rasters (there is no vector source), and no
//! CWC file is copied. Colors are Codewhale palette tokens resolved through
//! the live [`UiTheme`], so the whales follow Blue Stage dark/light, the
//! Terminal theme, and ANSI-16 adaptation like every other surface.
//!
//! Contract:
//! - **Role → species is one table** ([`WhaleSpecies::for_role_id`]). Fleet
//!   roles map onto the six species; roles without a species (`worker`,
//!   `general`, `custom`, unknown) render the plain Codewhale whale.
//! - **State is evidence, never decoration.** [`WhaleState`] is derived only
//!   from real runtime facts ([`WhaleState::for_subagent`],
//!   [`WhaleState::for_shell_phase`]). *Working* is asserted only for a child
//!   or turn that is actually running; a portrait rendered without a state
//!   claims nothing about runtime.
//! - **Every state pairs a glyph cue with a word** ([`WhaleState::word`]), so
//!   state never depends on color alone (same rule as `menu_style`).
//! - **Motion is policy-gated.** The Working wake animates only under
//!   [`MotionMode::Full`]; Reduced/Still hold the frame-A poster.
//! - **ASCII-safe by construction.** Every authored glyph has a
//!   [`glyphs::ascii_fallback`] entry, and [`portrait_ascii`] exposes the
//!   narrowed silhouette for tests and text surfaces.
//!
//! Only the *signal-classic* colorway is represented. The three alternate CWC
//! colorways exist only as resting rasters upstream and are not modelled here.

use std::borrow::Cow;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::localization::{Locale, MessageId, tr};
use crate::palette::{self, UiTheme};
use crate::tools::subagent::{AgentWorkerStatus, FleetRole, SubAgentResult, SubAgentStatus};
use crate::tui::glyphs;
use crate::tui::motion::mode::MotionMode;
use crate::tui::underwater::ShellPhase;

/// Portrait canvas: three rows, fourteen columns, including the two-column
/// state cue at the left and the wake lane under the tail at the right.
pub const PORTRAIT_WIDTH: usize = 14;
pub const PORTRAIT_HEIGHT: usize = 3;
/// Cells occupied by a badge (species mark + body).
pub const BADGE_WIDTH: usize = 2;
/// Working wake loop: four frames over 720 ms, as in the CWC GIFs.
pub const WORKING_FRAME_MS: u64 = 180;
pub const WORKING_FRAMES: usize = 4;

/// The six Signal Cut species plus the plain Codewhale whale for roles that
/// have no species of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WhaleSpecies {
    /// Beaked whale — research. The signature whale of the Signal Current mark.
    Scout,
    /// Harbor porpoise — coding.
    Patch,
    /// Humpback whale — coordination.
    Harbor,
    /// Pilot whale — communications.
    Echo,
    /// Sperm whale — operations.
    Keel,
    /// Orca — review.
    Lantern,
    /// The plain Codewhale whale: no species, no accent. Used for `worker`,
    /// `general`, `custom`, and unknown roles rather than guessing.
    Plain,
}

impl WhaleSpecies {
    /// Every species, for exhaustive checks and the test gallery.
    #[cfg(test)]
    pub const ALL: [WhaleSpecies; 7] = [
        Self::Scout,
        Self::Patch,
        Self::Harbor,
        Self::Echo,
        Self::Keel,
        Self::Lantern,
        Self::Plain,
    ];

    /// The single role → species table.
    ///
    /// Accepts Fleet profile ids / role hints (`manager`, `scout`, `builder`,
    /// `reviewer`, `verifier`, `consultant`, `synthesizer`, `general`,
    /// `custom`) and runtime [`FleetRole`] names (`worker`, `planner`).
    /// Anything else is [`WhaleSpecies::Plain`] — never a guess.
    #[must_use]
    pub fn for_role_id(role: &str) -> Self {
        match role.trim().to_ascii_lowercase().as_str() {
            "scout" => Self::Scout,
            "builder" => Self::Patch,
            "manager" | "planner" => Self::Harbor,
            "reviewer" => Self::Lantern,
            "verifier" => Self::Keel,
            "consultant" | "synthesizer" => Self::Echo,
            _ => Self::Plain,
        }
    }

    /// Species for a runtime Fleet role.
    #[must_use]
    pub fn for_fleet_role(role: &FleetRole) -> Self {
        Self::for_role_id(role.as_str())
    }

    /// Product name (a proper noun; not localized).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scout => "Scout",
            Self::Patch => "Patch",
            Self::Harbor => "Harbor",
            Self::Echo => "Echo",
            Self::Keel => "Keel",
            Self::Lantern => "Lantern",
            Self::Plain => "Codewhale",
        }
    }

    /// Localized species (animal) label.
    #[must_use]
    pub fn animal(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                Self::Scout => MessageId::WhaleAnimalScout,
                Self::Patch => MessageId::WhaleAnimalPatch,
                Self::Harbor => MessageId::WhaleAnimalHarbor,
                Self::Echo => MessageId::WhaleAnimalEcho,
                Self::Keel => MessageId::WhaleAnimalKeel,
                Self::Lantern => MessageId::WhaleAnimalLantern,
                Self::Plain => MessageId::WhaleAnimalPlain,
            },
        )
    }

    /// Localized job label.
    #[must_use]
    pub fn job(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                Self::Scout => MessageId::WhaleJobScout,
                Self::Patch => MessageId::WhaleJobPatch,
                Self::Harbor => MessageId::WhaleJobHarbor,
                Self::Echo => MessageId::WhaleJobEcho,
                Self::Keel => MessageId::WhaleJobKeel,
                Self::Lantern => MessageId::WhaleJobLantern,
                Self::Plain => MessageId::WhaleJobPlain,
            },
        )
    }

    /// Species-distinct 1-row mark: a feature glyph plus a body cell. Every
    /// pair narrows to a distinct ASCII pair (`<#`, `#]`, `#\`, `:#`, `#-`,
    /// `*#`, `.#`).
    #[must_use]
    pub const fn badge_glyphs(self) -> (&'static str, &'static str, bool) {
        // (feature, body, feature_first)
        match self {
            Self::Scout => ("◂", "▰", true),   // long beak
            Self::Patch => ("]", "▰", false),  // bracket patch
            Self::Harbor => ("▚", "▰", false), // long winglike flipper
            Self::Echo => (":", "▰", true),    // sonar ticks
            Self::Keel => ("━", "▰", false),   // keel stripe
            Self::Lantern => ("◇", "▰", true), // review lens (Reviewer charter glyph)
            Self::Plain => ("·", "▰", true),   // neutral dot
        }
    }
}

/// The six-state grammar. Priority (highest first) mirrors CWC:
/// waiting > blocked > working > thinking > offline > resting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WhaleState {
    Resting,
    Offline,
    Thinking,
    Working,
    Blocked,
    /// "Waiting for you": a human/parent action is required.
    Waiting,
}

impl WhaleState {
    /// Every state, for exhaustive checks and the test gallery.
    #[cfg(test)]
    pub const ALL: [WhaleState; 6] = [
        Self::Resting,
        Self::Thinking,
        Self::Working,
        Self::Waiting,
        Self::Blocked,
        Self::Offline,
    ];

    /// CWC state priority; higher wins when several facts apply. Public
    /// contract for surfaces that fold several children into one whale.
    #[cfg(test)]
    #[must_use]
    pub const fn priority(self) -> u8 {
        match self {
            Self::Waiting => 60,
            Self::Blocked => 50,
            Self::Working => 40,
            Self::Thinking => 30,
            Self::Offline => 20,
            Self::Resting => 10,
        }
    }

    /// Localized state word — always rendered next to the glyph cue.
    #[must_use]
    pub fn word(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                Self::Resting => MessageId::WhaleStateResting,
                Self::Thinking => MessageId::WhaleStateThinking,
                Self::Working => MessageId::WhaleStateWorking,
                Self::Waiting => MessageId::WhaleStateWaiting,
                Self::Blocked => MessageId::WhaleStateBlocked,
                Self::Offline => MessageId::WhaleStateOffline,
            },
        )
    }

    /// State from a real child record. Evidence rules:
    /// - a pending question for the parent/user → Waiting;
    /// - the live worker status when present (`WaitingForUser` → Waiting,
    ///   `ModelWait`/`Queued`/`Starting` → Thinking, `Running`/`RunningTool`
    ///   → Working, `Failed` → Blocked, `Interrupted` → Waiting, `Cancelled`
    ///   → Offline, `Completed` → Resting);
    /// - otherwise the durable status (`Running` → Working, `Completed` →
    ///   Resting, `Interrupted` → Waiting, `Failed`/`BudgetExhausted` →
    ///   Blocked, `Cancelled` → Offline).
    ///
    /// Working is therefore only ever asserted for a child the runtime says
    /// is running — never inferred from timestamps.
    #[must_use]
    pub fn for_subagent(agent: &SubAgentResult) -> Self {
        if agent.needs_input.is_some() {
            return Self::Waiting;
        }
        if let Some(status) = agent.worker_status {
            return match status {
                AgentWorkerStatus::WaitingForUser | AgentWorkerStatus::Interrupted => Self::Waiting,
                AgentWorkerStatus::Queued
                | AgentWorkerStatus::Starting
                | AgentWorkerStatus::ModelWait => Self::Thinking,
                AgentWorkerStatus::Running | AgentWorkerStatus::RunningTool => Self::Working,
                AgentWorkerStatus::Failed => Self::Blocked,
                AgentWorkerStatus::Cancelled => Self::Offline,
                AgentWorkerStatus::Completed => Self::Resting,
            };
        }
        match agent.status {
            SubAgentStatus::Running => Self::Working,
            SubAgentStatus::Completed => Self::Resting,
            SubAgentStatus::Interrupted(_) => Self::Waiting,
            SubAgentStatus::Failed(_) | SubAgentStatus::BudgetExhausted => Self::Blocked,
            SubAgentStatus::Cancelled => Self::Offline,
        }
    }

    /// State from the operator session phase. Public contract for the shell
    /// header / Fleet setup role pane (no consumer in this lane yet).
    #[allow(dead_code)]
    #[must_use]
    pub const fn for_shell_phase(phase: ShellPhase) -> Self {
        match phase {
            ShellPhase::Idle | ShellPhase::Done => Self::Resting,
            ShellPhase::Typing => Self::Thinking,
            ShellPhase::Working | ShellPhase::Verifying => Self::Working,
            ShellPhase::Waiting | ShellPhase::Approval => Self::Waiting,
            ShellPhase::Failed => Self::Blocked,
        }
    }
}

/// Resolved inks for one theme. Accents are contrast-enforced against the
/// theme surface (≥ 3:1, the secondary-chrome floor) so a Blue Stage Light or
/// custom surface never swallows a role mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhaleInk {
    /// Signal Gold body (the theme's `accent_action` slot, as the idle mark).
    pub body: Color,
    /// Orca body: ink/white patches read as muted text ink, saddle stays gold.
    pub lantern_body: Color,
    /// Eye, fluke centre.
    pub detail: Color,
    /// Bounded cyan: thinking ticks and the working wake.
    pub current: Color,
    /// Waiting ring — Signal Gold, the human-attention role.
    pub human: Color,
    /// Blocked obstruction bar.
    pub bar: Color,
    /// Offline outline.
    pub dim: Color,
    pub scout: Color,
    pub patch: Color,
    pub harbor: Color,
    pub echo: Color,
    pub keel: Color,
    pub lantern: Color,
}

impl WhaleInk {
    #[must_use]
    pub fn from_theme(theme: &UiTheme) -> Self {
        let surface = theme.surface_bg;
        let lift = |color: Color| {
            palette::enforce_contrast(color, surface, palette::SECONDARY_CHROME_CONTRAST)
        };
        let rgb = |(r, g, b): (u8, u8, u8)| Color::Rgb(r, g, b);
        Self {
            body: lift(theme.accent_action),
            lantern_body: lift(theme.text_muted),
            detail: theme.text_body,
            current: lift(rgb(palette::WHALE_CYAN_RGB)),
            human: lift(theme.accent_action),
            bar: lift(theme.text_muted),
            dim: theme.text_dim,
            scout: lift(rgb(palette::WHALE_CYAN_RGB)),
            patch: lift(theme.accent_secondary),
            harbor: lift(rgb(palette::WHALE_BRAND_ORANGE_RGB)),
            echo: lift(rgb(palette::WHALE_BRAND_MAGENTA_RGB)),
            keel: lift(theme.warning),
            lantern: lift(theme.mode_operate),
        }
    }

    #[must_use]
    pub const fn accent(&self, species: WhaleSpecies) -> Color {
        match species {
            WhaleSpecies::Scout => self.scout,
            WhaleSpecies::Patch => self.patch,
            WhaleSpecies::Harbor => self.harbor,
            WhaleSpecies::Echo => self.echo,
            WhaleSpecies::Keel => self.keel,
            WhaleSpecies::Lantern => self.lantern,
            WhaleSpecies::Plain => self.body,
        }
    }

    #[must_use]
    pub const fn body_for(&self, species: WhaleSpecies) -> Color {
        match species {
            WhaleSpecies::Lantern => self.lantern_body,
            _ => self.body,
        }
    }
}

/// Which ink a cell takes. Authored art carries one code per cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ink {
    None,
    Body,
    Accent,
    Detail,
    Cue,
    Human,
    Bar,
}

impl Ink {
    fn from_code(code: char) -> Self {
        match code {
            'b' => Self::Body,
            'a' => Self::Accent,
            'd' => Self::Detail,
            'c' => Self::Cue,
            'h' => Self::Human,
            'i' => Self::Bar,
            _ => Self::None,
        }
    }
}

/// Authored art: three rows of glyphs and a parallel ink map. Column 0–1 is
/// the state-cue lane (kept free of body glyphs on rows 0 and 2; row 1 may
/// hold a species feature at column 1 that a state cue overrides). Columns
/// 10–13 of row 2 are the wake lane. Head at the left, crown fluke `▚△▞` at
/// the right, matching the idle Codewhale mark in `underwater.rs`.
struct Art {
    rows: [&'static str; PORTRAIT_HEIGHT],
    inks: [&'static str; PORTRAIT_HEIGHT],
}

const fn art(species: WhaleSpecies) -> Art {
    match species {
        // Beaked whale: long beak, swept forehead, high tail, research lens.
        WhaleSpecies::Scout => Art {
            rows: ["    ▗▄▄▖   ▚△▞", " ━▐█○██▙━━━▞  ", "   ▝▀▀▀▘      "],
            inks: ["    bbbb   bdb", " bbbabbbbbbb  ", "   bbbbb      "],
        },
        // Harbor porpoise: compact torpedo, blunt forehead, small dorsal,
        // short tail, bracket patch.
        WhaleSpecies::Patch => Art {
            rows: ["  ▗▄▟▄▄▖  ▚△▞ ", "  ▐█·[]▙━━▞   ", "  ▝▀▀▀▀▘      "],
            inks: ["  bbbbbb  bdb ", "  bbdaabbbb   ", "  bbbbbb      "],
        },
        // Humpback: knobbled forehead, long winglike flipper, arched back,
        // broad tail, mooring-loop marking.
        WhaleSpecies::Harbor => Art {
            rows: ["  ▗▄▄▄▄▄▖  ▚△▞", "  ▐▒·█○█▙━━▞  ", "  ▝▀▚▀▀▀▘     "],
            inks: ["  bbbbbbb  bdb", "  bbdbabbbbb  ", "  bbbbbbb     "],
        },
        // Pilot whale: bulbous forehead, swept dorsal, sickle flipper, three
        // acoustic cheek ticks.
        WhaleSpecies::Echo => Art {
            rows: ["  ▄▄▄▄▟▄▖  ▚△▞", " :▐█·███▙━━▞  ", " ·▝▀▞▀▀▀▘     "],
            inks: ["  bbbbbbb  bdb", " abbdbbbbbbb  ", " abbbbbbb     "],
        },
        // Sperm whale: squared forehead, low dorsal ridge, heavy tail stock,
        // keel stripe.
        WhaleSpecies::Keel => Art {
            rows: ["  ▛▀▀▀▀▄▖  ▚△▞", "  ▐█·███▙▄▄▞  ", "  ▝▀━━━▀▘     "],
            inks: ["  bbbbbbb  bdb", "  bbdbbbbbbb  ", "  bbaaabb     "],
        },
        // Orca: tall dorsal, compact body, gold saddle patch on an ink body,
        // review-lens ring.
        WhaleSpecies::Lantern => Art {
            rows: ["  ▗▄▐▄▄▖  ▚△▞ ", "  ▐█○█▘▙━━▞   ", "  ▝▀▀▀▀▘      "],
            inks: ["  bbbbbb  bdb ", "  bbabhbbbb   ", "  bbbbbb      "],
        },
        // The plain Codewhale whale.
        WhaleSpecies::Plain => Art {
            rows: ["  ▗▄▄▄▄▄▖  ▚△▞", "  ▐█·███▙━━▞  ", "  ▝▀▀▀▀▀▘     "],
            inks: ["  bbbbbbb  bdb", "  bbdbbbbbbb  ", "  bbbbbbb     "],
        },
    }
}

/// One rendered cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    glyph: &'static str,
    ink: Ink,
}

type Canvas = [[Cell; PORTRAIT_WIDTH]; PORTRAIT_HEIGHT];

fn blank() -> Canvas {
    [[Cell {
        glyph: " ",
        ink: Ink::None,
    }; PORTRAIT_WIDTH]; PORTRAIT_HEIGHT]
}

/// Slice a static row into single-glyph `&'static str` cells.
fn glyph_at(row: &'static str, col: usize) -> Option<&'static str> {
    let mut indices = row.char_indices().skip(col);
    let (start, ch) = indices.next()?;
    Some(&row[start..start + ch.len_utf8()])
}

fn compose(species: WhaleSpecies, state: Option<WhaleState>, frame: usize) -> Canvas {
    let art = art(species);
    let mut canvas = blank();
    for (canvas_row, (row, inks)) in canvas.iter_mut().zip(art.rows.iter().zip(art.inks.iter())) {
        for (c, cell) in canvas_row.iter_mut().enumerate() {
            let glyph = glyph_at(row, c).unwrap_or(" ");
            let ink = inks.chars().nth(c).map(Ink::from_code).unwrap_or(Ink::None);
            *cell = Cell { glyph, ink };
        }
    }
    let Some(state) = state else {
        return canvas;
    };
    match state {
        WhaleState::Resting => {}
        WhaleState::Thinking => {
            // Two compact current ticks above the head.
            canvas[0][0] = Cell {
                glyph: "˚",
                ink: Ink::Cue,
            };
            canvas[0][1] = Cell {
                glyph: "˚",
                ink: Ink::Cue,
            };
        }
        WhaleState::Working => {
            // Bounded wake under the tail: anticipation, power stroke, glide,
            // recovery. Frame 0 is the reduced-motion poster.
            const WAKE: [[&str; 4]; WORKING_FRAMES] = [
                ["·", " ", " ", " "],
                ["·", "˚", " ", " "],
                [" ", "·", "˚", "·"],
                [" ", " ", "˚", "·"],
            ];
            let wake = WAKE[frame % WORKING_FRAMES];
            for (i, glyph) in wake.iter().enumerate() {
                canvas[2][10 + i] = Cell {
                    glyph,
                    ink: Ink::Cue,
                };
            }
        }
        WhaleState::Waiting => {
            // Surfaced head inside a Signal Gold ring cue.
            for (r, (left, right)) in [("╭", "─"), ("│", " "), ("╰", "─")].iter().enumerate()
            {
                canvas[r][0] = Cell {
                    glyph: left,
                    ink: Ink::Human,
                };
                if *right != " " {
                    canvas[r][1] = Cell {
                        glyph: right,
                        ink: Ink::Human,
                    };
                }
            }
        }
        WhaleState::Blocked => {
            // Nose against a vertical ink obstruction bar; wake and cues off.
            for row in canvas.iter_mut() {
                row[1] = Cell {
                    glyph: "▌",
                    ink: Ink::Bar,
                };
            }
        }
        WhaleState::Offline => {
            // Intact anatomy as a quiet open outline: fill drains, no wake.
            for row in canvas.iter_mut() {
                for cell in row.iter_mut() {
                    if cell.glyph == "█" {
                        cell.glyph = "░";
                    }
                }
            }
        }
    }
    canvas
}

fn cell_color(
    cell: Cell,
    species: WhaleSpecies,
    state: Option<WhaleState>,
    ink: &WhaleInk,
) -> Color {
    if state == Some(WhaleState::Offline) && cell.ink != Ink::None {
        return ink.dim;
    }
    match cell.ink {
        Ink::None => ink.dim,
        Ink::Body => ink.body_for(species),
        Ink::Accent => ink.accent(species),
        Ink::Detail => ink.detail,
        Ink::Cue => ink.current,
        Ink::Human => ink.human,
        Ink::Bar => ink.bar,
    }
}

/// Which working frame to show now. Anything but [`MotionMode::Full`] holds
/// the frame-A poster; callers must also only pass `Some(Working)` for a
/// child that is really running.
#[must_use]
pub const fn working_frame(now_ms: u64, mode: MotionMode) -> usize {
    match mode {
        MotionMode::Full => ((now_ms / WORKING_FRAME_MS) % WORKING_FRAMES as u64) as usize,
        MotionMode::Reduced | MotionMode::Still => 0,
    }
}

/// Three-row portrait as styled lines. `state == None` renders identity only
/// (no cue, no claim); `frame` selects the working wake frame.
#[must_use]
pub fn portrait(
    species: WhaleSpecies,
    state: Option<WhaleState>,
    frame: usize,
    theme: &UiTheme,
) -> Vec<Line<'static>> {
    let ink = WhaleInk::from_theme(theme);
    let canvas = compose(species, state, frame);
    canvas
        .iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut run = String::new();
            let mut run_color: Option<Color> = None;
            let flush = |run: &mut String, color: Option<Color>, spans: &mut Vec<Span<'static>>| {
                if run.is_empty() {
                    return;
                }
                let style = match color {
                    Some(color) => Style::default().fg(color),
                    None => Style::default(),
                };
                spans.push(Span::styled(std::mem::take(run), style));
            };
            let mut leading = true;
            for cell in row {
                let color = if cell.ink == Ink::None {
                    None
                } else {
                    Some(cell_color(*cell, species, state, &ink))
                };
                if color != run_color && !run.is_empty() {
                    flush(&mut run, run_color, &mut spans);
                }
                run_color = color;
                // Leading blanks become braille blanks so a trimming
                // `Paragraph` cannot shear the rows out of alignment (the
                // spinner already relies on U+2800 rendering blank; the
                // ASCII-safe backend narrows it back to a space).
                if leading && cell.glyph == " " {
                    run.push('\u{2800}');
                } else {
                    leading = false;
                    run.push_str(cell.glyph);
                }
            }
            flush(&mut run, run_color, &mut spans);
            Line::from(spans)
        })
        .collect()
}

/// The portrait narrowed through the glyph charter's ASCII fallback — what an
/// `CODEWHALE_ASCII_SAFE=1` terminal draws. Pure text, for tests and
/// text-only surfaces.
#[cfg(test)]
#[must_use]
pub fn portrait_ascii(
    species: WhaleSpecies,
    state: Option<WhaleState>,
    frame: usize,
) -> [String; 3] {
    let canvas = compose(species, state, frame);
    let mut rows: [String; 3] = Default::default();
    for (r, row) in canvas.iter().enumerate() {
        for cell in row {
            let glyph = cell.glyph;
            rows[r].push_str(if glyph.is_ascii() {
                glyph
            } else {
                glyphs::ascii_fallback(glyph).unwrap_or("?")
            });
        }
    }
    rows
}

/// The portrait as plain Unicode rows (no color), for tests and snapshots.
#[cfg(test)]
#[must_use]
pub fn portrait_text(
    species: WhaleSpecies,
    state: Option<WhaleState>,
    frame: usize,
) -> [String; 3] {
    let canvas = compose(species, state, frame);
    let mut rows: [String; 3] = Default::default();
    for (r, row) in canvas.iter().enumerate() {
        for cell in row {
            rows[r].push_str(cell.glyph);
        }
    }
    rows
}

/// Two-cell species badge: feature glyph in the role accent, body in Signal
/// Gold (Lantern's body in orca ink).
#[must_use]
pub fn badge(species: WhaleSpecies, theme: &UiTheme) -> Vec<Span<'static>> {
    let ink = WhaleInk::from_theme(theme);
    let (feature, body, feature_first) = species.badge_glyphs();
    let feature_span = Span::styled(
        feature,
        Style::default()
            .fg(ink.accent(species))
            .add_modifier(Modifier::BOLD),
    );
    let body_span = Span::styled(body, Style::default().fg(ink.body_for(species)));
    if feature_first {
        vec![feature_span, body_span]
    } else {
        vec![body_span, feature_span]
    }
}

/// Badge followed by the state word (glyph + word: never color alone). The
/// word takes the state's tone; when `state` is `None` only the badge renders.
#[cfg(test)]
#[must_use]
pub fn badge_with_state(
    species: WhaleSpecies,
    state: Option<WhaleState>,
    theme: &UiTheme,
    locale: Locale,
) -> Vec<Span<'static>> {
    badge_with_state_frame(species, state, 0, theme, locale)
}

/// [`badge_with_state`] with an explicit working-wake frame (see
/// [`working_frame`]); every non-working state ignores `frame`.
#[must_use]
pub fn badge_with_state_frame(
    species: WhaleSpecies,
    state: Option<WhaleState>,
    frame: usize,
    theme: &UiTheme,
    locale: Locale,
) -> Vec<Span<'static>> {
    let mut spans = badge(species, theme);
    if let Some(state) = state {
        let ink = WhaleInk::from_theme(theme);
        let (cue, tone) = state_cue(state, frame, &ink, theme);
        spans.push(Span::raw(" "));
        if !cue.is_empty() {
            spans.push(Span::styled(format!("{cue} "), Style::default().fg(tone)));
        }
        spans.push(Span::styled(
            state.word(locale).into_owned(),
            Style::default().fg(tone),
        ));
    }
    spans
}

/// One-cell state cue paired with its tone: the same grammar the portrait
/// draws, folded to a single glyph for badge rows.
fn state_cue(
    state: WhaleState,
    frame: usize,
    ink: &WhaleInk,
    theme: &UiTheme,
) -> (&'static str, Color) {
    /// One-cell wake: the same four-beat loop as the portrait, never blank.
    const WAKE_CUE: [&str; WORKING_FRAMES] = ["·", "˚", "·", "˚"];
    match state {
        WhaleState::Resting => ("", theme.text_muted),
        WhaleState::Thinking => ("˚", ink.current),
        WhaleState::Working => (WAKE_CUE[frame % WORKING_FRAMES], ink.current),
        WhaleState::Waiting => (glyphs::ATTENTION, ink.human),
        WhaleState::Blocked => ("▌", ink.bar),
        WhaleState::Offline => ("░", ink.dim),
    }
}

/// The badge as ASCII text (`<#`, `#]`, ...), for tests and text surfaces.
#[cfg(test)]
#[must_use]
pub fn badge_ascii(species: WhaleSpecies) -> String {
    let (feature, body, feature_first) = species.badge_glyphs();
    let narrow = |glyph: &'static str| -> &'static str {
        if glyph.is_ascii() {
            glyph
        } else {
            glyphs::ascii_fallback(glyph).unwrap_or("?")
        }
    };
    if feature_first {
        format!("{}{}", narrow(feature), narrow(body))
    } else {
        format!("{}{}", narrow(body), narrow(feature))
    }
}

/// Whether a portrait fits beside its caption: below the Compact tier width
/// callers should fall back to the badge.
#[must_use]
pub const fn portrait_fits(width: u16) -> bool {
    width as usize >= 60
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::contrast_ratio;
    use unicode_width::UnicodeWidthStr;

    fn theme_dark() -> UiTheme {
        palette::UI_THEME
    }

    #[test]
    fn role_table_is_total_and_never_guesses() {
        assert_eq!(WhaleSpecies::for_role_id("scout"), WhaleSpecies::Scout);
        assert_eq!(WhaleSpecies::for_role_id("builder"), WhaleSpecies::Patch);
        assert_eq!(WhaleSpecies::for_role_id("manager"), WhaleSpecies::Harbor);
        assert_eq!(WhaleSpecies::for_role_id("planner"), WhaleSpecies::Harbor);
        assert_eq!(WhaleSpecies::for_role_id("reviewer"), WhaleSpecies::Lantern);
        assert_eq!(WhaleSpecies::for_role_id("verifier"), WhaleSpecies::Keel);
        assert_eq!(WhaleSpecies::for_role_id("consultant"), WhaleSpecies::Echo);
        assert_eq!(WhaleSpecies::for_role_id("synthesizer"), WhaleSpecies::Echo);
        for plain in [
            "worker",
            "general",
            "custom",
            "",
            "mystery-role",
            "Scouting",
        ] {
            assert_eq!(
                WhaleSpecies::for_role_id(plain),
                WhaleSpecies::Plain,
                "{plain}"
            );
        }
        assert_eq!(
            WhaleSpecies::for_role_id("  Reviewer "),
            WhaleSpecies::Lantern
        );
        for role in [
            FleetRole::Worker,
            FleetRole::Scout,
            FleetRole::Planner,
            FleetRole::Reviewer,
            FleetRole::Builder,
            FleetRole::Verifier,
            FleetRole::Consultant,
            FleetRole::Custom,
        ] {
            // Every runtime role resolves without panicking.
            let _ = WhaleSpecies::for_fleet_role(&role);
        }
        assert_eq!(
            WhaleSpecies::for_fleet_role(&FleetRole::Custom),
            WhaleSpecies::Plain
        );
    }

    #[test]
    fn every_species_and_state_renders_inside_the_canvas() {
        for species in WhaleSpecies::ALL {
            let states = std::iter::once(None).chain(WhaleState::ALL.into_iter().map(Some));
            for state in states {
                for frame in 0..WORKING_FRAMES {
                    let rows = portrait_text(species, state, frame);
                    for row in &rows {
                        assert_eq!(
                            UnicodeWidthStr::width(row.as_str()),
                            PORTRAIT_WIDTH,
                            "{species:?} {state:?} f{frame}: {row:?}"
                        );
                    }
                    let lines = portrait(species, state, frame, &theme_dark());
                    assert_eq!(lines.len(), PORTRAIT_HEIGHT);
                    for line in &lines {
                        assert_eq!(line.width(), PORTRAIT_WIDTH, "{species:?} {state:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn state_priority_orders_attention_before_work() {
        assert!(
            WhaleState::Waiting.priority() > WhaleState::Working.priority()
                && WhaleState::Working.priority() > WhaleState::Resting.priority()
        );
    }

    #[test]
    fn authored_art_rows_and_ink_maps_agree() {
        for species in WhaleSpecies::ALL {
            let art = art(species);
            for (row, inks) in art.rows.iter().zip(art.inks.iter()) {
                assert_eq!(row.chars().count(), PORTRAIT_WIDTH, "{species:?} {row:?}");
                assert_eq!(inks.chars().count(), PORTRAIT_WIDTH, "{species:?} {inks:?}");
                for (glyph, code) in row.chars().zip(inks.chars()) {
                    assert_eq!(
                        glyph == ' ',
                        code == ' ',
                        "{species:?}: glyph {glyph:?} vs ink {code:?} in {row:?}"
                    );
                }
            }
            // The cue lane (cols 0-1) stays clear on rows 0 and 2 so state
            // cues never collide with anatomy; the wake lane (row 2, cols
            // 10-13) stays clear so the working wake never overprints.
            assert!(art.rows[0].starts_with("  "), "{species:?} row0 cue lane");
            assert!(art.rows[2].starts_with(' '), "{species:?} row2 cue lane");
            assert!(
                art.rows[2].chars().skip(10).all(|c| c == ' '),
                "{species:?} wake lane"
            );
        }
    }

    #[test]
    fn every_glyph_has_an_ascii_fallback_and_ascii_silhouettes_are_pure() {
        for species in WhaleSpecies::ALL {
            let states = std::iter::once(None).chain(WhaleState::ALL.into_iter().map(Some));
            for state in states {
                for frame in 0..WORKING_FRAMES {
                    let unicode = portrait_text(species, state, frame);
                    for row in &unicode {
                        for ch in row.chars() {
                            let glyph = ch.to_string();
                            assert!(
                                ch.is_ascii() || glyphs::ascii_fallback(&glyph).is_some(),
                                "{species:?} {state:?}: {glyph:?} has no ASCII fallback"
                            );
                        }
                    }
                    let ascii = portrait_ascii(species, state, frame);
                    for row in &ascii {
                        assert!(row.is_ascii(), "{species:?} {state:?}: {row:?}");
                        assert_eq!(row.chars().count(), PORTRAIT_WIDTH);
                        assert!(!row.contains('?'), "{species:?} {state:?}: {row:?}");
                    }
                }
            }
            let badge = badge_ascii(species);
            assert!(
                badge.is_ascii() && badge.chars().count() == BADGE_WIDTH,
                "{badge:?}"
            );
        }
        // Distinct species read distinctly even without Unicode.
        let mut badges: Vec<String> = WhaleSpecies::ALL.iter().map(|s| badge_ascii(*s)).collect();
        badges.sort();
        badges.dedup();
        assert_eq!(badges.len(), WhaleSpecies::ALL.len(), "{badges:?}");
    }

    #[test]
    fn plain_whale_matches_the_codewhale_mark_vocabulary() {
        let rows = portrait_text(WhaleSpecies::Plain, None, 0);
        assert_eq!(rows[0], "  ▗▄▄▄▄▄▖  ▚△▞");
        assert_eq!(rows[1], "  ▐█·███▙━━▞  ");
        assert_eq!(rows[2], "  ▝▀▀▀▀▀▘     ");
        let ascii = portrait_ascii(WhaleSpecies::Plain, None, 0);
        assert_eq!(ascii[0], "  .#####.  \\^/");
        assert_eq!(ascii[1], "  |#.####--/  ");
        assert_eq!(ascii[2], "  .#####.     ");
    }

    #[test]
    fn state_cues_follow_the_visual_brief() {
        let thinking = portrait_text(WhaleSpecies::Scout, Some(WhaleState::Thinking), 0);
        assert!(thinking[0].starts_with("˚˚"), "{thinking:?}");
        let working0 = portrait_text(WhaleSpecies::Scout, Some(WhaleState::Working), 0);
        assert!(working0[2].ends_with("·   "), "{working0:?}");
        let working2 = portrait_text(WhaleSpecies::Scout, Some(WhaleState::Working), 2);
        assert_ne!(working0[2], working2[2]);
        let waiting = portrait_text(WhaleSpecies::Harbor, Some(WhaleState::Waiting), 0);
        assert!(
            waiting[0].starts_with("╭─") && waiting[1].starts_with('│'),
            "{waiting:?}"
        );
        assert!(waiting[2].starts_with("╰─"), "{waiting:?}");
        let blocked = portrait_text(WhaleSpecies::Keel, Some(WhaleState::Blocked), 0);
        assert!(
            blocked.iter().all(|r| r.chars().nth(1) == Some('▌')),
            "{blocked:?}"
        );
        let offline = portrait_text(WhaleSpecies::Echo, Some(WhaleState::Offline), 0);
        assert!(!offline.iter().any(|r| r.contains('█')), "{offline:?}");
        assert!(offline[1].contains('░'), "{offline:?}");
        // Resting and identity-only carry no cue in the cue lane.
        for state in [None, Some(WhaleState::Resting)] {
            let rows = portrait_text(WhaleSpecies::Plain, state, 0);
            assert!(rows.iter().all(|r| r.starts_with("  ")), "{rows:?}");
        }
    }

    #[test]
    fn working_wake_only_animates_under_full_motion() {
        assert_eq!(working_frame(0, MotionMode::Full), 0);
        assert_eq!(working_frame(180, MotionMode::Full), 1);
        assert_eq!(working_frame(540, MotionMode::Full), 3);
        assert_eq!(working_frame(720, MotionMode::Full), 0);
        for now in [0, 180, 360, 540, 90_000] {
            assert_eq!(working_frame(now, MotionMode::Reduced), 0);
            assert_eq!(working_frame(now, MotionMode::Still), 0);
        }
    }

    #[test]
    fn every_state_pairs_a_cue_with_a_word_in_every_shipped_locale() {
        let theme = theme_dark();
        let ink = WhaleInk::from_theme(&theme);
        for state in WhaleState::ALL {
            for locale in Locale::shipped_complete() {
                let word = state.word(*locale);
                assert!(!word.trim().is_empty(), "{state:?} {locale:?}");
                let spans = badge_with_state(WhaleSpecies::Scout, Some(state), &theme, *locale);
                let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(text.contains(word.as_ref()), "{state:?} {locale:?}: {text}");
            }
            let (cue, _) = state_cue(state, 0, &ink, &theme);
            if state != WhaleState::Resting {
                assert!(!cue.is_empty(), "{state:?} needs a glyph cue");
            }
        }
        // Identity-only rows carry the badge and nothing that reads as state.
        let spans = badge_with_state(WhaleSpecies::Patch, None, &theme, Locale::En);
        assert_eq!(spans.len(), BADGE_WIDTH);
    }

    #[test]
    fn subagent_state_is_derived_from_runtime_facts_only() {
        let mut agent = SubAgentResult {
            name: "child-1".into(),
            agent_id: "child-1".into(),
            context_mode: "fresh".into(),
            fork_context: false,
            workspace: None,
            git_branch: None,
            agent_type: FleetRole::Builder,
            assignment: crate::tools::subagent::SubAgentAssignment {
                objective: "objective".into(),
                role: None,
            },
            model: String::new(),
            nickname: None,
            status: SubAgentStatus::Running,
            worker_status: None,
            runtime_permissions: None,
            parent_run_id: None,
            spawn_depth: 0,
            child_route: None,
            result: None,
            steps_taken: 0,
            checkpoint: None,
            needs_input: None,
            duration_ms: 0,
            started_at: None,
            from_prior_session: false,
        };
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Working);
        agent.status = SubAgentStatus::Completed;
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Resting);
        agent.status = SubAgentStatus::Interrupted("parent".into());
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Waiting);
        agent.status = SubAgentStatus::Failed("boom".into());
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Blocked);
        agent.status = SubAgentStatus::BudgetExhausted;
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Blocked);
        agent.status = SubAgentStatus::Cancelled;
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Offline);
        // Live worker status refines the durable status.
        agent.status = SubAgentStatus::Running;
        agent.worker_status = Some(AgentWorkerStatus::ModelWait);
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Thinking);
        agent.worker_status = Some(AgentWorkerStatus::RunningTool);
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Working);
        agent.worker_status = Some(AgentWorkerStatus::WaitingForUser);
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Waiting);
        // A pending question for the parent wins over everything.
        agent.worker_status = Some(AgentWorkerStatus::Running);
        agent.needs_input = Some(crate::tools::subagent::SubAgentNeedsInput {
            question: "which branch?".into(),
        });
        assert_eq!(WhaleState::for_subagent(&agent), WhaleState::Waiting);
        // Priorities match CWC.
        assert!(WhaleState::Waiting.priority() > WhaleState::Blocked.priority());
        assert!(WhaleState::Blocked.priority() > WhaleState::Working.priority());
        assert!(WhaleState::Working.priority() > WhaleState::Thinking.priority());
        assert!(WhaleState::Thinking.priority() > WhaleState::Offline.priority());
        assert!(WhaleState::Offline.priority() > WhaleState::Resting.priority());
    }

    #[test]
    fn shell_phase_maps_without_inventing_work() {
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Idle),
            WhaleState::Resting
        );
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Done),
            WhaleState::Resting
        );
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Typing),
            WhaleState::Thinking
        );
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Working),
            WhaleState::Working
        );
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Verifying),
            WhaleState::Working
        );
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Waiting),
            WhaleState::Waiting
        );
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Approval),
            WhaleState::Waiting
        );
        assert_eq!(
            WhaleState::for_shell_phase(ShellPhase::Failed),
            WhaleState::Blocked
        );
    }

    #[test]
    fn badge_accents_meet_secondary_chrome_contrast_on_dark_and_light() {
        for theme in [palette::UI_THEME, palette::LIGHT_UI_THEME] {
            let ink = WhaleInk::from_theme(&theme);
            for species in WhaleSpecies::ALL {
                for color in [ink.accent(species), ink.body_for(species)] {
                    let ratio = contrast_ratio(color, theme.surface_bg)
                        .unwrap_or_else(|| panic!("{species:?} unresolvable on {}", theme.name));
                    assert!(
                        ratio >= palette::SECONDARY_CHROME_CONTRAST,
                        "{species:?} {color:?} on {} = {ratio:.2}",
                        theme.name
                    );
                }
            }
            for color in [ink.current, ink.human, ink.bar] {
                let ratio = contrast_ratio(color, theme.surface_bg).unwrap();
                assert!(
                    ratio >= palette::SECONDARY_CHROME_CONTRAST,
                    "{color:?} {ratio:.2}"
                );
            }
        }
        // Terminal theme surfaces are terminal-owned; enforcement must pass
        // named colors through untouched rather than inventing RGB.
        let terminal = palette::TERMINAL_UI_THEME;
        let ink = WhaleInk::from_theme(&terminal);
        assert_eq!(ink.body, terminal.accent_action);
        assert_eq!(ink.keel, terminal.warning);
    }

    #[test]
    fn species_labels_are_localized_in_every_shipped_pack() {
        for species in WhaleSpecies::ALL {
            for locale in Locale::shipped_complete() {
                assert!(!species.animal(*locale).trim().is_empty());
                assert!(!species.job(*locale).trim().is_empty());
            }
        }
        assert_eq!(WhaleSpecies::Scout.name(), "Scout");
        assert_eq!(WhaleSpecies::Plain.name(), "Codewhale");
    }

    /// Rendered gallery — kept as a test so the whole set can be eyeballed
    /// with `--nocapture` without adding a slash command.
    #[test]
    #[allow(clippy::print_stdout)]
    fn gallery_renders_every_species_in_every_state() {
        for species in WhaleSpecies::ALL {
            println!("== {} ({}) ==", species.name(), badge_ascii(species));
            for state in std::iter::once(None).chain(WhaleState::ALL.into_iter().map(Some)) {
                let rows = portrait_text(species, state, 1);
                let ascii = portrait_ascii(species, state, 1);
                println!("-- {state:?}");
                for (u, a) in rows.iter().zip(ascii.iter()) {
                    println!("{u}    {a}");
                }
            }
        }
    }
}
