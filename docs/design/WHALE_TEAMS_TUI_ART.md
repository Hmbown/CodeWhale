# Whale Teams TUI — archived portrait-art reference

> **Status — retired reference, not current brand authority.** This record
> preserves the removed hand-drawn portrait/crown treatment for historical
> comparison. Do not use its `▚△▞` treatment as a Codewhale mark or infer that
> `underwater.rs` renders it. The canonical mark is a raster asset and has no
> approved ASCII or block-glyph substitute.

How the former Signal Cut mascots became terminal glyph art. The remaining
live runtime contract (role → species, state → evidence, inks) is
[`WHALE_TEAMS_TUI.md`](WHALE_TEAMS_TUI.md); this file is historical drawing
evidence only, not a specification for the startup hero or roster.

Source of truth (CWC repo, read-only from here):
`docs/design/whale-teams/VISUAL-BRIEF.md`, the Signal Cut concept boards and
the six-role × six-state avatar matrix beside it, and the runtime contract
`apps/web/lib/product/whale-identity.ts`. Nothing is copied; the terminal art
is authored fresh from the same silhouette grammar.

## What the mascots actually say

Read off the matrix and the working keyframes:

- **Head at the left**, blunt or beaked nose, eye well forward.
- **Gold upper body, ink lower body**, with the role accent on the flipper.
- **Swept dorsal fin** about a third of the way back on an arched back.
- **The tail is attached.** A short tail stock leaves the body and rises
  straight into the two-lobe fluke; the fluke sits *above* the body line.
- **One cyan cut** rides the belly. Cyan is the only water colour.
- **Working** adds bounded cyan wake dots *behind* the tail — never a change
  of pose, expression, or scale.

## (a) Historical empty-state portrait

Former `crates/tui/src/tui/underwater.rs` idle-art constants. The current
startup hero no longer uses this artwork and does not approximate the canonical
raster mark.

```
    ˚
  ▗▄▄▟▄▄▄▄▄▖  ▚△▞
 ▐█·████████▙▄▄▞
  ▝▀▀▀▀▀▀▀▀▘
```

uwu variant — same skeleton, one body cell shorter, blush and sparkle:

```
    ˚✦
  ▗▄▄▟▄▄▄▄▖  ▚△▞
 ▐█░·░█████▙▄▄▞
  ▝▀▀▀▀▀▀▀▘
```

What changed against the previous mark, and why:

| Before | After | Reason |
| --- | --- | --- |
| Fluke `▚△▞` floated six columns clear of the body, one column off the stock | The fluke's notch `△` sits directly above the rising stock tip `▞` | The mascots' tail is attached and rises out of the body; a detached fluke reads as debris |
| `▙━━━━▞` — a four-cell rule leaving the flank at eye height | `▙▄▄▞` — the stock stays body mass and falls away before it rises | A long thin horizontal bar reads as a spear; the hero now uses no rule glyph at all |
| Flat rectangular back | `▗▄▄▟▄▄▄▄▄▖` — swept dorsal at ~35 % of the body | Dorsal fin is the roster's strongest shared silhouette cue |
| Belly closed in gold | Belly row is the **cyan current cut** | The mark's second colour finally appears; gold back over ink/cyan belly matches the mascots |
| 24 columns wide | 17 columns | Quieter at 60 columns; leaves the composer the room |

The glyph vocabulary is deliberately the same one `whales::art` uses for the
six role portraits (`▗▄…▖` back, `▐█…▙` body, `▝▀…▘` belly, `▄` stock,
`▚△▞` crown fluke), so the hero and the `/fleet` roster are one animal at two
scales. Every glyph has a `glyphs::ascii_fallback` entry; with
`CODEWHALE_ASCII_SAFE=1` the hero narrows to

```
    o
  .########.  \^/
 |#.###########/
  .########.
```

## (b) Six role badges

Already shipped — no new work in this pass. `whales::badge` renders a two-cell
species badge (feature glyph in the role accent + body cell) on
`/fleet` roster rows (`views/fleet_roster.rs`) and sub-agent worker rows
(`views/mod.rs`, `SubAgentsView`). Six full block-art whales in a list row was
never the plan; the badge plus the name and state word carries the identity.
The one open consumer named in `WHALE_TEAMS_TUI.md` is the Fleet setup role
pane.

## (c) State → the TUI's existing status vocabulary

Also already shipped in `whales.rs`; restated here so the art has one home.

| Signal Cut state | TUI cue | Evidence |
| --- | --- | --- |
| Resting | level float, no wake | completed child, idle/done session |
| Thinking | two cyan ticks over the head, badge `˚` | queued / starting / model wait |
| Working | bounded cyan wake under the tail, badge `·`/`˚` 4-frame 720 ms loop | running / running-tool receipt |
| Waiting for you | Signal Gold ring at the head, badge `◆` | pending question or approval |
| Blocked | still, nose against a vertical ink bar, badge `▌` | failed / budget exhausted |
| Offline | open outline, drained fill, badge `░` | cancelled |

The hero is a **resting** whale and claims nothing: it renders only in the
empty state, carries no state cue, and its caustic sweep is decoration bound
to the motion policy, not a runtime signal. Glyph is always paired with a
word elsewhere; the hero has no word because it has no state.

## (d) Colorways → themes

Only **Signal Classic** is represented. The hero resolves its inks through the
live `UiTheme`: gold body from `accent_action` (Signal Gold `#F6C453` on the
whale theme), and the spout and belly cut from
`whales::WhaleInk::from_theme(..).current` — brand cyan `#48D7FF` lifted to the
secondary-chrome contrast floor against the theme surface. That is a change:
the spout previously took each theme's `accent_secondary`. Using the Whale
Teams ink means the empty-state hero and the `/fleet` portraits speak the same
cyan in every theme, and light themes stay legible.

`uwu` keeps its own art variant and its sakura eye/blush channel.

Deep Current, Reef Shift, and Night Signal are **deferred**. They are colour
variation only — never role or state meaning — so a terminal that already
derives every ink from the active theme gains nothing from a second colour
axis. Revisit only if the TUI grows a whale-specific theme picker.

`scripts/preview-whale-art.py` renders this comparison in true colour, in
plain text (`docs/design/whale-art-preview.txt`), and as a terminal-cell PNG
(`docs/design/whale-art-preview.png`). It reads the art constants straight out
of `underwater.rs`, so the preview cannot drift from the build.

## Not represented

The four-frame working loop on the hero (it is a resting mark), the ambient
cameo glyphs in `ambient_life.rs`, `notifications.rs` `TITLE_WHALE_FRAMES`, the
legacy WhalePet atlas, and any raster asset.
