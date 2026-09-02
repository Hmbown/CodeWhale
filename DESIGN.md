---
name: Codewhale
description: Quiet, dense, navy-and-blue design system for Codewhale — the terminal shell, the website, and the docs.
colors:
  # brand constants (brand/*.svg, shared with the TUI palette)
  brand-black: "#000000"
  brand-ink: "#070c1d"
  brand-navy: "#0c1531"
  brand-stage: "#142352"
  brand-ivory: "#ffffff"
  brand-ice: "#ddeef9"
  brand-cobalt: "#1535b2"
  brand-blue: "#6aa6dc"
  brand-cyan: "#78bce8"
  ombre-start: "#1535B2"
  ombre-end: "#6AA6DC"
  # surface tokens — crates/tui/src/palette/tokens.rs, exported to web/app/tokens.css (generated, never hand-edit)
  bg: "#070c1d"          # ink
  chrome: "#0c1531"      # navy
  panel: "#101c40"
  composer: "#142352"    # stage — the raised input plate
  elevated: "#1a2c63"
  selection: "#1e3c8f"
  border: "#2a3f72"
  text-body: "#f6f2e8"
  text-soft: "#b6c0d4"
  text-muted: "#93a0b8"
  action: "#6aa6dc"      # sky — the ombre's light end
  action-hover: "#8fc4f8"
  cobalt: "#1535b2"      # the ombre's dark end
  ice: "#ddeef9"
  cyan: "#78bce8"
  success: "#9bd66f"
  warning: "#ff7a59"
  error: "#ff86b2"
  human: "#f6c453"
typography:
  display:
    fontFamily: "IBM Plex Sans Condensed, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.25rem, 4.8vw, 4.25rem)"
    fontWeight: 600
    lineHeight: 1
    letterSpacing: "-0.01em"
  heading:
    fontFamily: "IBM Plex Sans Condensed, ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.45rem, 2.6vw, 2.35rem)"
    fontWeight: 600
    lineHeight: 1.08
    letterSpacing: "-0.01em"
  subheading:
    fontFamily: "IBM Plex Sans Condensed, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.12rem"
    fontWeight: 500
    lineHeight: 1.25
  body:
    fontFamily: "IBM Plex Sans, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1rem"
    fontWeight: 400
    lineHeight: 1.6
  meta:
    fontFamily: "JetBrains Mono, ui-monospace, monospace"
    fontSize: "0.75rem"
    fontWeight: 500
    letterSpacing: "0.08em"
  code:
    fontFamily: "JetBrains Mono, ui-monospace, monospace"
    fontSize: "0.85rem"
    fontWeight: 400
    lineHeight: 1.55
rounded:
  none: "0px"
  sm: "4px"
  md: "6px"
  pill: "999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "16px"
  lg: "24px"
  xl: "40px"
  section: "clamp(3rem, 7vw, 6rem)"
components:
  button-primary:
    backgroundColor: "{colors.action}"
    textColor: "{colors.bg}"
    rounded: "{rounded.sm}"
    padding: "10px 16px"
    typography: "{typography.meta}"
  button-primary-hover:
    backgroundColor: "{colors.action-hover}"
    textColor: "{colors.bg}"
  button-secondary:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.sm}"
    padding: "10px 16px"
  card:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.text-body}"
    rounded: "{rounded.md}"
    padding: "16px 18px"
  nav:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.text-soft}"
    height: "62px"
---

## Overview

Codewhale is an open-source Rust coding agent and TUI. Everything it shows —
the shell, the website, the docs — is built to feel like one tool: a flat navy
field, one blue action colour, white type, thin hairlines, and nothing
decorative. The direction in one line: **it doesn't need to look special — it
needs to look like Codewhale.** Density over drama, facts over claims,
documentation-grade restraint on every surface including the landing page and
the launch hero.

This file is shared design context for every 0.9.12 lane (web, docs, TUI). The
web-specific product truth is in `PRODUCT.md` next to it.

## Anti-slop rules

These are hard rules, not taste notes. A surface that breaks one is wrong even
if it looks fine in a screenshot.

1. **No gradients for decoration.** The ombre `#1535B2 → #6AA6DC` lives in the
   mark and the wordmark only. No gradient backgrounds, gradient text,
   gradient rules, radial "spotlight" glows, or gradient borders anywhere else.
2. **No glassmorphism.** No `backdrop-filter`, no translucent frosted panels,
   no blur. Surfaces are opaque tokens from the palette.
3. **No generic SaaS hero.** No three-column feature grid with icons, no logo
   wall, no testimonials, no floating product mock at an angle, no "trusted by",
   no neon eyebrow line. The landing page states what the tool is, shows one
   real screenshot, and gives the install command.
4. **No fabricated evidence.** No invented terminal transcripts, reasoning
   traces, benchmarks, or screenshots of a UI that has not shipped. Pending
   media is labelled pending.
5. **No shadow theatre.** No drop shadows on cards, buttons, or figures; depth is
   one surface step and one hairline.
6. **No second accent.** One action blue. Cyan is bounded to the composer
   prompt glyph; state colours carry state only; violet is 1px rules, never
   text.
7. **No status-chip soup, no emoji, no all-caps headings.** Labels are
   letterspaced mono at ≥ 0.7rem and there are few of them.
8. **No text below the floors** (12px functional, 11.2px labels) and no
   text/background pair under 4.5:1.

## Colors

One palette, owned by `crates/tui/src/palette/tokens.rs`, exported to
`web/app/tokens.css` by `scripts/export-design-tokens.py` (regenerate, never
edit). The brand-name aliases used in conversation map onto it: black `#000`,
ink `#070c1d` = `bg`, navy `#0c1531` = `chrome`, stage `#142352` = `composer`,
ivory `#fff`, ice `#ddeef9`, cobalt `#1535b2`, blue/sky `#6aa6dc` = `action`,
cyan `#78bce8`. The TUI uses the same tokens through `palette::grammar` for
status colours. `web/app/globals.css` maps them to semantic names: `--paper`
(bg), `--paper-deep` (panel), `--ink` (text), `--ink-soft`, `--ink-mute`,
`--indigo` (action), `--hairline` (action at 20 % alpha).

- **Field:** on the web, `bg` for the page and the transcript and `chrome`
  for bars and the dock; in the TUI the ground is the terminal's own
  background (`Color::Reset`), and the navy field is painted only under the
  opt-in `ocean_treatment = "deepsea"` (alias `underwater`) column. `panel`
  for cards and code, `composer` for the raised input plate, `elevated` only
  for overlays. Never stack more than two surface steps in one view.
- **Type:** `text-body` for copy, `text-soft` for secondary, `text-muted` for
  meta. `text-dim` (#697791) is for borders only — it fails AA on the panels.
- **Action:** one blue (`action`) for links, buttons, and focus rings; hover
  lifts to `action-hover`. `cyan` is a bounded accent (eyebrow chrome, composer
  prompt) — never body text, never fills.
- **Brand ombre** `#1535B2 → #6AA6DC` exists only in the mark and wordmark.
  Do not paint UI with it.
- **State colours** (`success`, `warning`, `error`, `human`) carry meaning; do
  not use them decoratively, and never convey state by colour alone.

Contrast: every text/background pair in use is ≥ 4.5:1 (`text-muted` on
`panel` 6.3:1, on `composer` 5.7:1, on `bg` 7.4:1; `action` on `bg` 7.5:1).

## Typography

Three faces, one job each:

- **IBM Plex Sans Condensed 600** — display and all headings (`--font-display`).
  Also the wordmark: "codewhale" in Plex Sans Condensed SemiBold, outlined to
  paths, letter-spacing −0.01em. Tight leading (1.0–1.08), no all-caps headings.
- **IBM Plex Sans 400/500/600** — body (`--font-body`). Measure ≤ 70ch.
- **JetBrains Mono 400/500** — code and the mono meta rows (`--font-mono`).
  Letterspaced uppercase mono is the only "label" style.

Floors: functional text (links, nav, labels, meta, footer) never below
**12px (0.75rem)**; letterspaced micro-labels never below **11.2px (0.7rem)**;
legal smallprint never below 10px. Heading outline is strict: h1 → h2 → h3, no
skipped levels; use CSS, not a lower heading tag, to make something smaller.

## Shell (TUI) direction

The redesigned 0.9.12 shell is the reference the website mirrors. Top to
bottom (founder decisions, 2026-09-01/02 — see
`codewhale-ops/design/SHELL-DESIGN-20260901.md` for the dated record):

1. **Transcript first.** The conversation owns the screen: no permanent side
   chrome, no top bar. Turns are plain text on the terminal's own ground;
   tool output is `text-soft` on `panel`; reasoning is the reasoning tint,
   collapsed by default.
2. **Launch is a card, not a hero.** A thin top line (`⑂ branch  path`);
   a centred bordered card: the whale mark at left, `Codewhale` + version,
   one announcement line only when it is true, then the menu **New
   worktree · Resume session · Changelog · Quit** with real chords
   right-aligned. Enter runs the highlighted entry; typing goes straight to
   the composer; the card dissolves on the first keystroke or command.
3. **Composer.** The raised `composer` plate directly under the transcript,
   cyan `❯` prompt glyph. While the card is up its bottom rule carries
   `model (effort) · permission` — the route's one launch reading.
4. **Posture bar + metrics line** under the composer, once a session
   exists: `▶▶ ask (Shift+Tab) · work (Tab) · 2 agents · Esc to interrupt …`
   and `model · ctx NN% · $cost · ttft · tok/s · ↓ tokens … Ctrl+/ help`.
   Context % lives in the metrics line only.
5. **Bottom views, not a dock.** One region under the composer cycles
   through agents → tasks → background → files → notepad → context → git →
   price (Ctrl+Tab forward, Ctrl+Shift+Tab back where the terminal
   delivers it). Zero rows when idle with nothing to show. There is no
   rail, no sidebar, no tab band.

## Layout (web)

- Single content column, `.product-container` max 72rem, 1rem side padding at
  390px. The landing has no permanent side chrome; docs have a left contents
  rail at ≥ 1050px that collapses into a top list below.
- Sections are separated by one hairline and generous vertical space
  (`spacing.section`), not by background colour changes.
- Prose measure ≤ 70ch on docs; wide code blocks scroll horizontally inside
  their panel rather than widening the column.
- Breakpoints in use: 640px (hero stacks, h1 drops to `clamp(2rem, 10vw, 3rem)`),
  900px (hero two-column), 1050px (docs rail).
- No horizontal overflow at 390px, ever.

## Elevation & Depth

Flat. Depth is expressed by one surface step (`bg` → `panel`) and one hairline.
No drop shadows on cards or buttons; the only shadow is the raised composer
plate in the TUI screenshot itself. No blur, no glass, no glow.

## Shapes

Small radii: 4px on controls, 6px on cards and code panels, 999px only on the
GitHub-stars pill. No rounded-2xl, no circles as decoration. Whale mark is the
only curved form.

## Components (web)

- **Nav:** 62px bar, `bg`, hairline below. Left: whale mark (22px) + wordmark
  (20px) as one link labelled "Codewhale home". Centre: text links in body
  face. Right: theme, locale, stars pill, sign in / register (mono meta), one
  filled Install button. Collapses to a menu button below 900px.
- **Buttons:** filled action blue (primary), panel with hairline (secondary),
  text-only (ghost). Mono meta type, uppercase, 0.08em tracking. Visible focus
  ring in `action` on every control.
- **Cards / steps:** `panel` bg, hairline border, 16–18px padding, number in
  `action` mono, h3 in display face, body in `text-soft`.
- **Code blocks:** `panel` bg, JetBrains Mono 0.85rem, copy button top-right,
  scroll-x inside.
- **Eyebrow:** mono, uppercase, 0.7rem, `cyan` or `text-muted`.
- **Footer:** `bg`, hairline top, inverted wordmark, column links at 0.75rem
  mono; legal line at 0.7rem.
- **Media slots:** real assets only; a `pending` slot renders a labelled empty
  panel, never a mock.

## Do's and Don'ts

Do
- Derive every fact from the repo; one owner per number.
- Keep the whale mark and wordmark together in the nav; wordmark alone in the footer.
- Use the display face for headings and the wordmark, nothing else.
- Meet AA and the 12px floor before shipping any new surface.

Don't
- Fabricate terminal output, reasoning traces, testimonials, or screenshots.
- Introduce a second accent colour, gradients on UI, shadows, or glass.
- Add a UI library or page-local copy; extend `lib/content/` and the dictionaries.
- Use `text-dim` for text, skip heading levels, or shrink type below the floors.
