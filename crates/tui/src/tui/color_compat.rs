//! Terminal color compatibility shim.
//!
//! Ratatui's crossterm backend emits truecolor SGR for every `Color::Rgb`
//! cell. That is correct for truecolor terminals, but macOS Terminal.app often
//! advertises only `xterm-256color`; sending `38;2` / `48;2` there can render
//! as stray green/cyan backgrounds. This backend adapts every cell to the
//! detected color depth before handing it to crossterm.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};

use ratatui::{
    backend::{Backend, ClearType, CrosstermBackend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
};

use crate::palette::{self, ColorDepth, PaletteMode, ThemeId, UiTheme};

const RENDER_DEBUG_ENV: &str = "CODEWHALE_TUI_DEBUG";
const RENDER_DEBUG_SAMPLE_LIMIT: usize = 24;

#[derive(Debug)]
pub(crate) struct ColorCompatBackend<W: Write> {
    inner: CrosstermBackend<W>,
    depth: ColorDepth,
    palette_mode: PaletteMode,
    /// Currently active named theme. `System`/`Whale`/`WhaleLight` make the
    /// theme remap a no-op (those rely on the dark/light pipeline); the
    /// community presets (Catppuccin, Tokyo Night, Dracula, Gruvbox) trigger
    /// a per-cell rewrite of dark-palette constants → preset slots.
    theme_id: ThemeId,
    /// Resolved active `UiTheme`, *including* any user `background_color`
    /// override (`UiTheme::with_background_color`). The cell remap reads
    /// target slots from this struct, not from `theme_id.ui_theme()`, so
    /// `theme = "tokyo-night"` + `background_color = "#000000"` lands as a
    /// pure-black surface instead of being overwritten back to
    /// tokyo-night's `#16161e` by the remap.
    active_ui_theme: UiTheme,
    /// During a resize event the terminal emulator may report stale dimensions
    /// for a brief window (observed on macOS Terminal.app and Windows ConHost).
    /// Forcing the expected size prevents ratatui's internal `autoresize` from
    /// shrinking the viewport back to the stale dimension inside `draw()`.
    forced_size: Option<Size>,
    /// Cached terminal size from `crossterm::terminal::size()`, set after
    /// re-entering alt-screen to avoid stale buffer dimensions on Windows.
    /// Used as the primary fallback in `size()` before falling through to
    /// the live crossterm query.
    terminal_size: Option<Size>,
    render_debug: Option<RenderDebugLog>,
}

impl<W: Write> ColorCompatBackend<W> {
    pub(crate) fn new(writer: W, depth: ColorDepth, palette_mode: PaletteMode) -> Self {
        Self {
            inner: CrosstermBackend::new(writer),
            depth,
            palette_mode,
            theme_id: ThemeId::System,
            // Default to whatever System resolves to right now — it stays a
            // no-op for the remap since `theme_id` is also System, so this
            // initial value only matters once `set_theme` flips both fields
            // to a community preset.
            active_ui_theme: UiTheme::detect(),
            forced_size: None,
            terminal_size: None,
            render_debug: RenderDebugLog::from_env(),
        }
    }

    pub(crate) fn force_size(&mut self, size: Size) {
        self.forced_size = Some(size);
    }

    pub(crate) fn clear_forced_size(&mut self) {
        self.forced_size = None;
    }

    pub(crate) fn set_terminal_size(&mut self, size: Size) {
        self.terminal_size = Some(size);
    }

    pub(crate) fn set_palette_mode(&mut self, palette_mode: PaletteMode) {
        self.palette_mode = palette_mode;
    }

    pub(crate) fn set_theme(&mut self, theme_id: ThemeId, ui_theme: UiTheme) {
        self.theme_id = theme_id;
        self.active_ui_theme = ui_theme;
    }
}

impl<W: Write> Write for ColorCompatBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

impl<W: Write> Backend for ColorCompatBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let adapted = content
            .map(|(x, y, cell)| {
                let mut cell = cell.clone();
                adapt_cell_colors(
                    &mut cell,
                    self.depth,
                    self.palette_mode,
                    self.theme_id,
                    &self.active_ui_theme,
                );
                (x, y, cell)
            })
            .collect::<Vec<_>>();
        let viewport = if self.render_debug.is_some() {
            self.size().ok()
        } else {
            None
        };
        if let Some(render_debug) = &mut self.render_debug {
            render_debug.record(viewport, &adapted);
        }
        // #3029: Emit OSC 8 hyperlinks out-of-band through the backend's
        // Write impl.  ratatui's buffer pipeline strips ESC bytes, so the
        // open/close sequences must be interleaved with the cell stream
        // here.  OSC 8 is stateful and last-writer-wins: every cell painted
        // between an open and the next close links to that open's target,
        // so each region's cells must be bracketed by their OWN open/close
        // pair — never batched.
        let mut frame_links = crate::tui::osc8::take_frame_links();
        if frame_links.is_empty() || !crate::tui::osc8::enabled() {
            self.inner
                .draw(adapted.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
            return Ok(());
        }
        // Deterministic region lookup when regions are adjacent/overlapping:
        // the first (top-left-most) region wins.
        frame_links.sort_unstable_by_key(|link| (link.row, link.col_start));
        let region_for = |x: u16, y: u16| -> Option<usize> {
            frame_links
                .iter()
                .position(|link| y == link.row && x >= link.col_start && x <= link.col_end)
        };

        // Walk the diff in its original order and split it into runs at
        // region boundaries, so the visible byte stream stays identical to
        // a no-link render apart from the inserted OSC 8 sequences.
        let mut idx = 0;
        while idx < adapted.len() {
            let current_region = region_for(adapted[idx].0, adapted[idx].1);
            let run_start = idx;
            while idx < adapted.len()
                && region_for(adapted[idx].0, adapted[idx].1) == current_region
            {
                idx += 1;
            }
            let run = &adapted[run_start..idx];
            if let Some(region_idx) = current_region {
                crate::tui::osc8::write_osc8_open(self, &frame_links[region_idx].target)?;
                self.inner
                    .draw(run.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
                crate::tui::osc8::write_osc8_close(self)?;
            } else {
                self.inner
                    .draw(run.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
            }
        }
        Ok(())
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        // forced_size takes priority: it is set during resize events to prevent
        // ratatui's autoresize from shrinking the viewport back to a stale
        // dimension. terminal_size is the cached real terminal size used as a
        // fallback after alt-screen re-entry (Windows buffer width workaround).
        if let Some(size) = self.forced_size.or(self.terminal_size) {
            return Ok(size);
        }
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

#[derive(Debug)]
struct RenderDebugLog {
    file: File,
    frame: u64,
}

impl RenderDebugLog {
    fn from_env() -> Option<Self> {
        if !render_debug_enabled_from_value(std::env::var(RENDER_DEBUG_ENV).ok().as_deref()) {
            return None;
        }

        let log_dir = crate::runtime_log::log_directory()?;
        if let Err(err) = fs::create_dir_all(&log_dir) {
            tracing::debug!(?err, "failed to create TUI render debug log directory");
            return None;
        }
        let path = log_dir.join("tui-render.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| {
                tracing::debug!(?err, path = %path.display(), "failed to open TUI render debug log");
                err
            })
            .ok()?;

        Some(Self { file, frame: 0 })
    }

    fn record(&mut self, viewport: Option<Size>, diff: &[(u16, u16, Cell)]) {
        self.frame = self.frame.saturating_add(1);
        let sample = diff
            .iter()
            .take(RENDER_DEBUG_SAMPLE_LIMIT)
            .map(|(x, y, _)| (*x, *y))
            .collect::<Vec<_>>();
        let line = render_debug_line(self.frame, viewport, diff.len(), &sample);
        let _ = self.file.write_all(line.as_bytes());
    }
}

fn render_debug_enabled_from_value(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn render_debug_line(
    frame: u64,
    viewport: Option<Size>,
    diff_cells: usize,
    sample: &[(u16, u16)],
) -> String {
    let mut line = String::new();
    match viewport {
        Some(size) => {
            let _ = write!(
                &mut line,
                "frame={frame} size={}x{} diff_cells={diff_cells} sample=",
                size.width, size.height
            );
        }
        None => {
            let _ = write!(
                &mut line,
                "frame={frame} size=unknown diff_cells={diff_cells} sample="
            );
        }
    }
    for (index, (x, y)) in sample.iter().enumerate() {
        if index > 0 {
            line.push(',');
        }
        let _ = write!(&mut line, "{x}:{y}");
    }
    line.push('\n');
    line
}

fn adapt_cell_colors(
    cell: &mut Cell,
    depth: ColorDepth,
    palette_mode: PaletteMode,
    theme_id: ThemeId,
    ui_theme: &UiTheme,
) {
    // Stage 1: community-theme remap (dark palette → preset slots). No-op
    // for System / Whale / WhaleLight so legacy dark/light flows are
    // untouched. Runs *before* the palette-mode remap so a light terminal
    // running e.g. Catppuccin still routes the preset colors through the
    // light adaptation below (rare combo, but the sequencing is the same).
    cell.fg = palette::adapt_fg_for_theme(cell.fg, theme_id, ui_theme);
    cell.bg = palette::adapt_bg_for_theme(cell.bg, theme_id, ui_theme);
    // Stage 2: legacy dark↔light remap.
    let original_bg = cell.bg;
    cell.fg = palette::adapt_fg_for_palette_mode(cell.fg, original_bg, palette_mode);
    cell.bg = palette::adapt_bg_for_palette_mode(cell.bg, palette_mode);
    // Stage 3: depth (truecolor / 256 / 16) downsampling.
    cell.fg = palette::adapt_color(cell.fg, depth);
    cell.bg = palette::adapt_bg(cell.bg, depth);
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, env, ffi::OsString, fs, io::Write, rc::Rc};

    use ratatui::backend::Backend;
    use ratatui::{buffer::Cell, style::Color};

    use super::*;
    use crate::test_support::lock_test_env;

    #[derive(Clone, Default)]
    struct SharedWriter(Rc<RefCell<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct EnvRestore {
        key: &'static str,
        value: Option<OsString>,
    }

    impl EnvRestore {
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                value: env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            // SAFETY: environment mutation is serialized by lock_test_env.
            unsafe {
                match &self.value {
                    Some(value) => env::set_var(self.key, value),
                    None => env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn adapts_rgb_cells_to_indexed_on_ansi256() {
        let mut cell = Cell::default();
        cell.set_fg(Color::Rgb(53, 120, 229));
        cell.set_bg(Color::Rgb(11, 21, 38));

        adapt_cell_colors(
            &mut cell,
            ColorDepth::Ansi256,
            PaletteMode::Dark,
            ThemeId::System,
            &palette::UI_THEME,
        );

        assert!(matches!(cell.fg, Color::Indexed(_)));
        assert!(matches!(cell.bg, Color::Indexed(_)));
    }

    #[test]
    fn leaves_truecolor_cells_unchanged() {
        let mut cell = Cell::default();
        cell.set_fg(Color::Rgb(53, 120, 229));
        cell.set_bg(Color::Rgb(11, 21, 38));

        adapt_cell_colors(
            &mut cell,
            ColorDepth::TrueColor,
            PaletteMode::Dark,
            ThemeId::System,
            &palette::UI_THEME,
        );

        assert_eq!(cell.fg, Color::Rgb(53, 120, 229));
        assert_eq!(cell.bg, Color::Rgb(11, 21, 38));
    }

    #[test]
    fn ansi256_backend_output_does_not_emit_truecolor_sgr() {
        let writer = SharedWriter::default();
        let capture = writer.0.clone();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::Ansi256, PaletteMode::Dark);
        let mut cell = Cell::default();
        cell.set_symbol("x")
            .set_fg(Color::Rgb(53, 120, 229))
            .set_bg(Color::Rgb(11, 21, 38));

        backend.draw(std::iter::once((0, 0, &cell))).unwrap();

        let output = String::from_utf8_lossy(&capture.borrow()).to_string();
        assert!(!output.contains("38;2;"), "{output:?}");
        assert!(!output.contains("48;2;"), "{output:?}");
    }

    #[test]
    fn light_palette_maps_dark_cells_before_depth_adaptation() {
        let mut cell = Cell::default();
        cell.set_fg(Color::White);
        cell.set_bg(palette::WHALE_BG);

        adapt_cell_colors(
            &mut cell,
            ColorDepth::TrueColor,
            PaletteMode::Light,
            ThemeId::WhaleLight,
            &palette::LIGHT_UI_THEME,
        );

        assert_eq!(cell.fg, palette::LIGHT_TEXT_BODY);
        assert_eq!(cell.bg, palette::LIGHT_SURFACE);
    }

    #[test]
    fn grayscale_palette_maps_hued_cells_before_depth_adaptation() {
        let mut cell = Cell::default();
        cell.set_fg(palette::WHALE_INFO);
        cell.set_bg(palette::WHALE_BG);

        adapt_cell_colors(
            &mut cell,
            ColorDepth::TrueColor,
            PaletteMode::Grayscale,
            ThemeId::Grayscale,
            &palette::GRAYSCALE_UI_THEME,
        );

        assert_eq!(cell.fg, palette::GRAYSCALE_TEXT_SOFT);
        assert_eq!(cell.bg, palette::GRAYSCALE_SURFACE);
    }

    #[test]
    fn community_theme_remap_honors_background_color_override() {
        // Tokyo Night + a custom black surface: the remap must rewrite
        // `palette::WHALE_BG` to the *active* UiTheme's overridden
        // surface, not to tokyo-night's default surface.
        let active = palette::TOKYO_NIGHT_UI_THEME.with_background_color(Color::Rgb(0, 0, 0));
        let mut cell = Cell::default();
        cell.set_bg(palette::WHALE_BG);

        adapt_cell_colors(
            &mut cell,
            ColorDepth::TrueColor,
            PaletteMode::Dark,
            ThemeId::TokyoNight,
            &active,
        );

        assert_eq!(cell.bg, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn backend_palette_mode_can_follow_runtime_theme_changes() {
        let writer = SharedWriter::default();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);

        assert_eq!(backend.palette_mode, PaletteMode::Dark);
        backend.set_palette_mode(PaletteMode::Light);
        assert_eq!(backend.palette_mode, PaletteMode::Light);
        backend.set_palette_mode(PaletteMode::Grayscale);
        assert_eq!(backend.palette_mode, PaletteMode::Grayscale);
    }

    #[test]
    fn render_debug_env_parser_accepts_truthy_values_only() {
        assert!(!render_debug_enabled_from_value(None));
        assert!(!render_debug_enabled_from_value(Some("")));
        assert!(!render_debug_enabled_from_value(Some("0")));
        assert!(!render_debug_enabled_from_value(Some("false")));
        assert!(render_debug_enabled_from_value(Some("1")));
        assert!(render_debug_enabled_from_value(Some("true")));
        assert!(render_debug_enabled_from_value(Some("YES")));
        assert!(render_debug_enabled_from_value(Some("on")));
    }

    #[test]
    fn render_debug_line_records_frame_size_and_diff_sample() {
        let line = render_debug_line(7, Some(Size::new(80, 24)), 42, &[(0, 0), (12, 3), (79, 23)]);

        assert_eq!(
            line,
            "frame=7 size=80x24 diff_cells=42 sample=0:0,12:3,79:23\n"
        );
    }

    #[test]
    fn backend_writes_render_debug_log_when_enabled() {
        let _lock = lock_test_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let _home = EnvRestore::capture("HOME");
        let _userprofile = EnvRestore::capture("USERPROFILE");
        let _debug = EnvRestore::capture(RENDER_DEBUG_ENV);

        // SAFETY: environment mutation is serialized by lock_test_env.
        unsafe {
            env::set_var("HOME", tmp.path());
            env::set_var("USERPROFILE", "");
            env::set_var(RENDER_DEBUG_ENV, "1");
        }

        let writer = SharedWriter::default();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);
        let mut cell = Cell::default();
        cell.set_symbol("x");
        backend.draw(std::iter::once((3, 4, &cell))).unwrap();

        let log_path = tmp
            .path()
            .join(".codewhale")
            .join("logs")
            .join("tui-render.log");
        let body = fs::read_to_string(log_path).expect("render debug log");
        assert!(body.contains("frame=1"), "{body}");
        assert!(body.contains("diff_cells=1"), "{body}");
        assert!(body.contains("sample=3:4"), "{body}");
    }

    #[test]
    fn size_returns_terminal_size_when_set() {
        let writer = SharedWriter::default();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);

        backend.set_terminal_size(Size::new(120, 40));
        assert_eq!(backend.size().unwrap(), Size::new(120, 40));
    }

    #[test]
    fn forced_size_takes_priority_over_terminal_size() {
        let writer = SharedWriter::default();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);

        // forced_size is set during resize events to temporarily override the
        // cached terminal_size — it must win to prevent viewport shrinking.
        backend.set_terminal_size(Size::new(120, 40));
        backend.force_size(Size::new(80, 25));
        assert_eq!(backend.size().unwrap(), Size::new(80, 25));
    }

    #[test]
    fn size_falls_back_to_forced_size_when_terminal_size_unset() {
        let writer = SharedWriter::default();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);

        backend.force_size(Size::new(80, 25));
        assert_eq!(backend.size().unwrap(), Size::new(80, 25));
    }

    // ── #3029: OSC 8 emission through the backend byte stream ──────────────

    fn row_cells(symbols: &str) -> Vec<(u16, u16, Cell)> {
        symbols
            .chars()
            .enumerate()
            .map(|(i, ch)| {
                let mut cell = Cell::default();
                cell.set_symbol(&ch.to_string());
                (u16::try_from(i).unwrap(), 0u16, cell)
            })
            .collect()
    }

    #[test]
    fn osc8_open_close_bracket_only_their_region_cells() {
        use crate::tui::osc8::LinkRegion;

        // Baseline: identical cells, no link regions.
        let baseline_writer = SharedWriter::default();
        let baseline_capture = baseline_writer.0.clone();
        let mut baseline =
            ColorCompatBackend::new(baseline_writer, ColorDepth::TrueColor, PaletteMode::Dark);
        let cells = row_cells("ABCDE");
        baseline
            .draw(cells.iter().map(|(x, y, cell)| (*x, *y, cell)))
            .unwrap();
        let baseline_out = String::from_utf8_lossy(&baseline_capture.borrow()).to_string();

        // Linked render: columns 2..=3 ("CD") carry one link region.
        crate::tui::osc8::set_frame_links(vec![LinkRegion {
            row: 0,
            col_start: 2,
            col_end: 3,
            target: "https://example.test/1".to_string(),
        }]);
        let writer = SharedWriter::default();
        let capture = writer.0.clone();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);
        let cells = row_cells("ABCDE");
        backend
            .draw(cells.iter().map(|(x, y, cell)| (*x, *y, cell)))
            .unwrap();
        let out = String::from_utf8_lossy(&capture.borrow()).to_string();

        let open = "\x1b]8;;https://example.test/1\x1b\\";
        let close = "\x1b]8;;\x1b\\";
        assert_eq!(out.matches(open).count(), 1, "exactly one open: {out:?}");
        assert_eq!(out.matches(close).count(), 1, "exactly one close: {out:?}");

        // The open must precede the first linked glyph and the close must sit
        // between the last linked glyph and the first glyph after the region.
        let open_at = out.find(open).expect("open present");
        let close_at = out.find(close).expect("close present");
        let c_at = out.find('C').expect("glyph C");
        let d_at = out.find('D').expect("glyph D");
        let e_at = out.find('E').expect("glyph E");
        assert!(open_at < c_at, "open before linked cells: {out:?}");
        assert!(d_at < close_at, "close after linked cells: {out:?}");
        assert!(
            close_at < e_at,
            "cells after the region must not inherit the link: {out:?}"
        );

        // Visible glyph stream is unchanged by link insertion.
        let mut baseline_visible = String::new();
        crate::tui::osc8::strip_ansi_into(&baseline_out, &mut baseline_visible);
        let mut linked_visible = String::new();
        crate::tui::osc8::strip_ansi_into(&out, &mut linked_visible);
        assert_eq!(
            baseline_visible, linked_visible,
            "link emission must not move or alter visible cells"
        );
    }

    #[test]
    fn osc8_two_regions_link_to_their_own_targets() {
        use crate::tui::osc8::LinkRegion;

        crate::tui::osc8::set_frame_links(vec![
            LinkRegion {
                row: 0,
                col_start: 0,
                col_end: 1,
                target: "https://example.test/first".to_string(),
            },
            LinkRegion {
                row: 0,
                col_start: 3,
                col_end: 4,
                target: "https://example.test/second".to_string(),
            },
        ]);
        let writer = SharedWriter::default();
        let capture = writer.0.clone();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);
        let cells = row_cells("ABZCD");
        backend
            .draw(cells.iter().map(|(x, y, cell)| (*x, *y, cell)))
            .unwrap();
        let out = String::from_utf8_lossy(&capture.borrow()).to_string();

        let first = "\x1b]8;;https://example.test/first\x1b\\";
        let second = "\x1b]8;;https://example.test/second\x1b\\";
        let close = "\x1b]8;;\x1b\\";
        assert_eq!(out.matches(first).count(), 1, "{out:?}");
        assert_eq!(out.matches(second).count(), 1, "{out:?}");
        assert_eq!(out.matches(close).count(), 2, "{out:?}");

        // Pre-#3029-audit bug: both opens were emitted before any cell, so
        // the whole frame linked to the LAST region's target. Each region's
        // open must close before the next region's open begins.
        let first_at = out.find(first).expect("first open");
        let first_close_at = out[first_at..].find(close).expect("first close") + first_at;
        let second_at = out.find(second).expect("second open");
        assert!(
            first_close_at < second_at,
            "region one must close before region two opens: {out:?}"
        );
        // The unlinked middle glyph sits between the two link spans.
        let z_at = out.find('Z').expect("unlinked glyph");
        assert!(first_close_at < z_at && z_at < second_at, "{out:?}");
    }

    /// #3029 end-to-end: the in-band `wrap_link` payload rendered into a Buffer
    /// by `Paragraph` must (a) be cleaned out of the cells by the extractor and
    /// (b) re-emitted out-of-band by the backend around the label glyph. This
    /// proves producer (`extract_buffer_link_regions` + `set_frame_links`) and
    /// consumer (`ColorCompatBackend::draw`) compose.
    #[test]
    fn osc8_extractor_feeds_backend_out_of_band() {
        use crate::tui::osc8;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Paragraph, Widget};

        // 1. Render an in-band link payload into a buffer, exactly as the
        //    transcript render seam would.
        let target = "https://example.test/e2e";
        let label = "open";
        let wrapped = osc8::wrap_link(target, label);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        Paragraph::new(vec![Line::from(vec![Span::raw(wrapped)])]).render(area, &mut buf);

        // 2. The extractor recovers the region AND blanks the payload cells.
        //    Capture the region once — re-running would find nothing because
        //    the payload is now gone (that's the point).
        let regions = osc8::extract_buffer_link_regions(&mut buf, area);
        assert_eq!(regions.len(), 1, "one link recovered: {regions:?}");
        osc8::set_frame_links(regions);

        // 3. Hand the cleaned cells to the backend and capture the byte stream.
        let writer = SharedWriter::default();
        let capture = writer.0.clone();
        let mut backend = ColorCompatBackend::new(writer, ColorDepth::TrueColor, PaletteMode::Dark);
        let cells: Vec<(u16, u16, ratatui::buffer::Cell)> = (0..area.width)
            .map(|x| {
                let cell = buf[(x, 0)].clone();
                (x, 0u16, cell)
            })
            .collect();
        backend
            .draw(cells.iter().map(|(x, y, cell)| (*x, *y, cell)))
            .unwrap();
        let out = String::from_utf8_lossy(&capture.borrow()).to_string();

        // 4. The backend re-emitted the link out-of-band around the label.
        let open = format!("\x1b]8;;{target}\x1b\\");
        let close = "\x1b]8;;\x1b\\";
        assert_eq!(
            out.matches(open.as_str()).count(),
            1,
            "open emitted once: {out:?}"
        );
        assert_eq!(out.matches(close).count(), 1, "close emitted once: {out:?}");
        let open_at = out.find(open.as_str()).expect("open");
        let close_at = out.find(close).expect("close");
        let label_at = out.find(label).expect("label glyph");
        assert!(open_at < label_at, "open precedes label: {out:?}");
        assert!(label_at < close_at, "close follows label: {out:?}");
    }
}
