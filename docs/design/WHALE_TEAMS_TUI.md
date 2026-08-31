# Whale Teams in the TUI

Codewhale Cloud's **Whale Teams / Signal Cut** identity (CWC, 2026-08-15)
gives every agent role a species-led whale and every whale one of six runtime
states. This document is the contract for how that identity appears in the
Codewhale terminal UI. The implementation is `crates/tui/src/tui/whales.rs`.

> **Status — runtime badges current; portrait reference retired.** The
> canonical Codewhale product mark is a raster asset outside this terminal-art
> vocabulary. Neither the old hand-drawn crown/fluke treatment nor an
> approximate block-glyph silhouette is a current product mark, and neither
> renders in `underwater.rs`.

The source artwork (six species × six states × four colorways of 384×192
rasters, plus concept boards and the visual brief) lives in the CWC repository.
No CWC file is copied here: the terminal whales are authored fresh as glyph
art, using the same state grammar and the same palette tokens.

## Role → species (single table)

| Codewhale role id / `FleetRole` | Whale | Species | Job |
| --- | --- | --- | --- |
| `scout` | Scout | beaked whale | research |
| `builder` | Patch | harbor porpoise | coding |
| `manager`, `planner` | Harbor | humpback whale | coordination |
| `consultant`, `synthesizer` | Echo | pilot whale | communications |
| `verifier` | Keel | sperm whale | operations |
| `reviewer` | Lantern | orca | review |
| `worker`, `general`, `custom`, unknown | *(plain Codewhale whale)* | whale | general work |

`WhaleSpecies::for_role_id` is the only place this mapping exists. Roles the
table does not name render the plain Codewhale whale — the mapping never
guesses from keywords.

The roster resolves species from the profile id first (built-in ids are role
names), then from the resolved worker agent type. Worker rows resolve from the
advisory `assignment.role` when it names a species, otherwise from the runtime
`FleetRole`.

## State grammar and evidence

| State | Word | Cue (portrait) | Cue (badge) | Evidence required |
| --- | --- | --- | --- | --- |
| Resting | Resting | level float, no wake | *(none)* | completed child; idle/done session |
| Thinking | Thinking | two cyan ticks above the head | `˚` | worker `Queued`/`Starting`/`ModelWait`; session typing |
| Working | Working | bounded cyan wake under the tail | `·`/`˚` loop | worker `Running`/`RunningTool`, or durable status `Running`; session working/verifying |
| Waiting for you | Waiting for you | Signal Gold ring around the head | `◆` | pending question for the parent, worker `WaitingForUser`/`Interrupted`, durable `Interrupted`; session waiting/approval |
| Blocked | Blocked | still, nose against a vertical ink bar | `▌` | worker `Failed`; durable `Failed`/`BudgetExhausted`; session failed |
| Offline | Offline | open outline, fill drained, no wake | `░` | worker/durable `Cancelled` |

Rules:

- **State is evidence, never decoration.** `WhaleState::for_subagent` reads
  only the child's real status fields; `WhaleState::for_shell_phase` reads the
  session `ShellPhase`. Working is never inferred from elapsed time or a
  timestamp.
- **A portrait without a state claims nothing.** The `/fleet` roster renders
  members as identity only (no state cue, no state word, no caption) because
  a profile is not a runtime.
- **Glyph + word, always.** Every state pairs a cue glyph with a localized word
  (`WhaleState::word`), so state never depends on color alone — the same rule
  `menu_style::StatusMark` enforces.
- **Priority** when several facts apply mirrors CWC: waiting 60 > blocked 50 >
  working 40 > thinking 30 > offline 20 > resting 10.

## Motion

The Working wake is a four-frame loop over 720 ms (180 ms per frame), the same
timing as the CWC GIFs. `whales::working_frame(now_ms, mode)` returns frame 0
(the poster) for `MotionMode::Reduced` and `MotionMode::Still`; only
`MotionMode::Full` animates. Surfaces that cannot see the motion policy hold
the poster.

## Colors

All inks resolve through the live `UiTheme` (`WhaleInk::from_theme`) and are
contrast-enforced to the secondary-chrome floor (3:1) against the theme
surface. Only the **signal-classic** colorway is represented.

| Ink | Token |
| --- | --- |
| Body | `accent_action` (Signal Gold `#F6C453` on Blue Stage; `#7A5500` on Blue Stage Light) — the idle mark's body |
| Lantern (orca) body | `text_muted`, with a Signal Gold saddle patch |
| Eye / fluke centre | `text_body` |
| Thinking ticks, working wake, Scout research lens | `WHALE_CYAN_RGB` `#48D7FF` (bounded accent) |
| Waiting ring | `accent_action` (Signal Gold, the human-attention role) |
| Blocked bar | `text_muted` |
| Offline | `text_dim` |
| Patch bracket | `accent_secondary` (seafoam `#4FD1C5`) |
| Harbor mooring loop | `WHALE_BRAND_ORANGE_RGB` `#FF8A3D` |
| Echo sonar ticks | `WHALE_BRAND_MAGENTA_RGB` `#F04EB8` |
| Keel stripe | `warning` (coral `#FF7A59`) |
| Lantern review lens | `mode_operate` (violet `#AD88FF`) |

`WHALE_BRAND_ORANGE_RGB` and `WHALE_BRAND_MAGENTA_RGB` are the two CWC brand
palette entries the TUI palette did not already carry. They are identity
accents only and must never be used for status, mode, or permission.

## Art

The current startup intentionally uses direct copy rather than translating the
canonical raster into terminal glyph art. [`WHALE_TEAMS_TUI_ART.md`](WHALE_TEAMS_TUI_ART.md),
its text preview, and its generator are retained as an **archived portrait
reference** for removed artwork; they must not be used as a product-mark source.

The live runtime uses two-cell badges: a species feature glyph in the role
accent plus a body cell. With `CODEWHALE_ASCII_SAFE=1` the seven badges stay
distinct: `<#` `#]` `#\` `:#` `#-` `*#` `.#`.

## Surfaces

- `/fleet` roster: species badge on every member row, with the
  `badge Name · species · job` line — no retired portrait is rendered.
- `/fleet` workers (`SubAgentsView`): badge on every worker row, plus a
  second line with the badge and the state cue and word — nothing else.
- `whales::badge` is public for other surfaces (the Fleet setup role pane is
  the intended next consumer).

Not represented: the deep-current, reef-shift, and night-signal colorways; the
legacy WhalePet atlas and Rive companion contracts (which have no artwork in
either repository); any GIF/raster asset.
