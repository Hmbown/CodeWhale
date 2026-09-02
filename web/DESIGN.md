---
name: Codewhale
description: Quiet, dense, navy-and-blue documentation-grade site for a terminal coding agent.
colors:
  # brand constants (brand/*.svg, shared with the TUI palette)
  brand-black: "#000000"
  brand-ink: "#070c1d"
  brand-navy: "#0c1531"
  brand-stage: "#142352"
  brand-ivory: "#ffffff"
  brand-ice: "#ddeef9"
  brand-cobalt: "#0b48bb"
  brand-blue: "#1e8fd8"
  brand-cyan: "#78bce8"
  ombre-start: "#1535B2"
  ombre-end: "#6AA6DC"
  # web surface tokens (app/tokens.css, generated — never edit by hand)
  bg: "#03070d"
  chrome: "#08111c"
  panel: "#0e1729"
  composer: "#162238"
  elevated: "#182742"
  border: "#263e5c"
  text-body: "#f6f2e8"
  text-soft: "#b6c0d4"
  text-muted: "#93a0b8"
  action: "#6aaef2"
  action-hover: "#8fc4f8"
  ice: "#d1ebf4"
  cyan: "#48d7ff"
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

Codewhale is a coding agent that lives in a terminal, and the site is built to
feel like the tool: a dark navy field, one blue action colour, white type, thin
hairlines, and nothing decorative. The direction in one line: **it doesn't need
to look special — it needs to look like Codewhale.** Density over drama, facts
over claims, documentation-grade restraint on every page including the landing.

Anti-references: purple/violet gradients, glassmorphism, glowing cards, fake
terminal transcripts, fake reasoning traces, stock "AI" imagery, emoji, and
status-chip soup. None of it belongs here.

## Colors

The web colours are the TUI palette exported to `app/tokens.css` (generated
from `crates/tui/src/palette/tokens.rs`; regenerate, never edit). `globals.css`
maps them to semantic names: `--paper` (bg), `--paper-deep` (panel), `--ink`
(text), `--ink-soft`, `--ink-mute`, `--indigo` (action), `--hairline` (action at
20 % alpha).

- **Field:** `bg` for the page, `panel` for cards and code, `elevated` only for
  raised inputs. Never stack more than two surface steps.
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
`panel` is 6.8:1, on `bg` 7.7:1).

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

## Layout

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

## Components

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
