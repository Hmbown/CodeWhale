# Tideline → Ratatui translation spec

**Status:** spec + scaffolding. Not merged, no PR opened.
**Branch:** `design/tideline-ratatui-translation-20260828` (worktree
`cw-main-0912-integrator-20260828`), based on `origin/main` `dde9cdb044cf7625b78eeb31e53a479ae07b16f2`
(rebase-past #5699/#5700 verified). Branch point ancestry: `e5a6046cb` → #5699 → #5700.
**Implemented & green:** topbar (`crates/tui/src/tui/topbar.rs`) + golden-buffer
harness, 6/6 via `./scripts/dev-test.sh tui topbar` (11,329 skipped).

Authority order used: the three approved screenshots > this document's
prose > the recovered motion sketch (motion language) > `tideline-redesign.html`
(interaction-registry idea only, superseded on layout).

---

## 1. What the three approved screens actually contain

Cell-inventory read of the references (startup, work+fleet, settings/appearance):

- **Topbar (all three).** One row: `CODEWHALE` wordmark; contextual
  segments (`run …`, `fleet …`, `3/4 whales`, `model …`, `theme …`,
  `Settings / Appearance`, `folder …`); pinned right = `context NN% ▰▰▱▱▱` +
  full clock. Segment set varies per screen; brand/meter/clock never move.
- **Startup.** Centered hero: "What are we working on?", one dim
  subtitle; `QUICK ACTIONS` band with 3 rows (icon · label · description ·
  command + `›`); a 4-column option strip (New worktree / Chat only / Theme /
  Help); whale-outline composer; footer with route · cost · keys.
- **Work + Fleet.** Left rail (RUNS / WHALES / FLEET / WORK / CONTEXT, then
  help/settings, `«` collapse); receipt stream (user + assistant turns, a
  `├──/└──` pod-formation tree, state-marked receipt rows with timestamps and
  receipt counts, an indented conclusion block, a legend row); `FLEET LEDGER`
  table (WHALE/ASSIGNMENT/STATE/ELAPSED/RECEIPTS/LAST UPDATE, selected row
  marker `▶`); composer; footer with cost and keys.
- **Settings.** 3 panes: category rail (Appearance → Advanced + help/file/
  feedback); theme list (13 rows: 4 mode rows + 9 presets, selected row boxed
  + `✓`) and MOTION (OPTIONAL) toggles; live Deepsea preview pane replaying a
  transcript + mini ledger + composer, with translucent whale silhouettes in
  the background; footer with 6 key bindings.

## 2. HTML lies — resolutions (non-negotiable)

| In the reference | Why it cannot ship | Decision |
|---|---|---|
| Composer drawn as a stroked whale outline | No bezier strokes; only box-drawing glyphs | **Rounded border.** `╭─╮│╰╯` border (dim at rest, Info on focus); the old fluke cap is retired and no brand glyph is hand-drawn into the composer. The send `↑` becomes a 3-cell hitbox `[↑]` right-aligned inside the border. The terminal never approximates the canonical raster mark. |
| Translucent whale silhouettes behind text (Deepsea preview) | No alpha; painting over text destroys it | **Empty-cells-only compositing**, the `ambient_life.rs` rule verbatim: write only cells that are open water (`is_open_water` + `TEXT_CLEARANCE_ROWS = 1` clearance from `occupied_text_bounds`). Eviction order when water is scarce: bubbles first, then fish school, then jellyfish; the whale cameo is evicted last (highest identity value). Caustic-style tinting stays bg-only on `cell.symbol() == " "`. Deepsea ambient runs only under `MotionMode::Full`. |
| SVG icons (plug, clock, folder, palette, chat) | Not renderable | One glyph per action, added to `glyphs.rs` with declared ASCII fallbacks via `ascii_fallback`: plug `⌁`→`+`, resume `↺`→`<`, folder `▤`→`=`, palette `◐`→`*`, chat `◌`→`o`, worktree `⑂`→`y`, help `?`→`?` (identity). Each is 1 cell, no wide glyphs. |
| Ledger cells wrapping to two lines | Table columns are exact integer cells | Fixed column widths + per-column truncation: WHALE 10 (never truncates — names are short by contract), ASSIGNMENT = remainder (truncate with `…`, never wrap), STATE 12 (glyph + word), ELAPSED 8, RECEIPTS 8, LAST UPDATE 8 (`HH:MM:SS`). **At 80 columns** the rail is hidden and ledger sheds to `WHALE │ ASSIGNMENT │ STATE` — ELAPSED, RECEIPTS, LAST UPDATE drop in that order before ASSIGNMENT loses cells. |
| Meter bars `▰▱`, tree `├── └──`, state marks `●○✓!` | Fine — they are glyphs | Keep. Declared ASCII fallbacks already in `glyphs::ascii_fallback` (`▰→#`, `▱→-`, `├→+`, `└→+`, `●→.`, `○→.`, `✓→Y`, `!→!`). Verified single-width in the ascii-safe golden test. |

**Rule restated:** if it cannot be expressed as cells, it is not in the design.
Nothing else in the three screens requires non-cell expression.

## 3. Lineage — what each component replaces (frame.rs slot map)

The live shell renders ten slots (`tui/ui/frame.rs` header split ~:818, body
constraints ~:928). The references collapse the bottom into one footer:

| Slot today | Disposition |
|---|---|
| header (`underwater::render_header`) | **Replaced** by `topbar::Topbar` (implemented here). Facts survive: mode/permission chips move to the footer activity segment; route/model stays a topbar segment. |
| 0 work strip (`work_surface::render`) | **Extends** — becomes the rail's WORK group (owned by #5699; do not fight their files). |
| 1 chat (+rail via `split_chat`) | **Extends** — receipt stream + fleet ledger attach to the transcript column; rail reuses `work_surface::split_chat`. |
| 2 workflow panel | **Kept unchanged** (drill-in above composer). |
| 3 pending input preview | **Merged into the composer** as a one-row crumb above the input line — the reference shows queued messages as composer content, not a band. |
| 4 background-work chip | **Deleted as a band**; the fact moves to the topbar `fleet n/m` segment and the rail WORK group (one surface owns each fact). |
| 5 session boot receipt | **Deleted as a band**; boot lines become ordinary transcript receipts. |
| 6 activity band | **Merged into the footer** (left half: phase chip + echolocation + cost). |
| 7 composer | **Extends** — rounded border + `[↑]` hitbox; composer authority logic untouched. |
| 8 identity band | **Merged into the footer** (right half: depth line + key legend). `phase_strip::render_identity` is the merge target; `render_footer` delegates today already. |

Orphaned facts, each with exactly one home: cost/token ledger → footer;
boot receipts → transcript; background-work → topbar fleet segment + WORK rail;
permission/mode chips → footer activity segment; session metrics detail →
`/cost` (the sketch's rule: the ledger row moves behind `/cost`).

## 4. Founder directive resolutions

1. **Blue stage vs `Color::Reset`.** Smallest honest resolution: Dark and
   Light keep `Color::Reset` (the host-terminal inheritance rule and sketch
   principle 1 are load-bearing). The blue stage ships as **Deepsea, promoted
   to a first-class one-keystroke toggle** (topbar Theme segment opens the
   picker with Deepsea preselected; the theme strip on startup), plus a
   blue-biased ink ramp for Dark so the stage reads cool even on Reset.
   **Tradeoff named, not papered over:** a user on Dark who wants the painted
   field without Deepsea's underwater life cannot have it — with no alpha,
   "inherit host background" and "painted blue field" are mutually exclusive;
   anything else is a third theme pretending not to be one.
2. **Continuous activity vs "no ambient loops".** Resolution: *state is always
   shown; decoration is opt-in.* The footer's phase segment (echolocation chip
   + phase word + live detail) is persistent in every phase, matching
   `phase_strip::activity_height()`'s existing always-reserved row. Ambient
   life (fish/jelly/bubbles/cameo) stays `MotionMode::Full`-gated. Nothing
   new loops for decoration.
3. **Mature and original.** One surface owns each fact (§3 map). The braille
   spinner is **retired** in favor of echolocation (`<·>` family) at the same
   420 ms cadence; `spinner.rs` constants remain for non-Tideline call sites
   until the migration slice deletes them.
4. **Motion gating + still frames.** Every motion below declares a still frame
   that carries the same information and rides `MotionMode`
   (`MotionPolicy::from_settings(low_motion, fancy_animations, …)`), wall-clock
   keyed, never frame-count keyed.

## 5a. Component inventory

Data sources are real `App` fields/methods (verified at HEAD). "Hitbox" =
where the `Rect` is stored for `mouse_ui` (existing pattern:
`viewport.last_*_area` / `work_surface.hitboxes` / `launch.row_areas`).

| Component | What it does | States | Data source | Replaces | Owning file | Keys | Mouse hitbox | Golden name |
|---|---|---|---|---|---|---|---|---|
| Topbar | One-row status surface | per-screen segment set; hover; shed | `effective_route_identity_display()`, run/fleet summaries, `context_budget` pct, injected clock | `underwater::render_header` | `tui/topbar.rs` ✅ | Tab⇄, Enter activate | brand/menu + per-segment rects → `viewport.last_topbar_hitboxes` | `topbar_{startup,work,settings}_{w}x{h}` ✅ |
| Hero (startup) | Centered prompt + subtitle | first-run vs returning | `LaunchState`, `workspace_session_count` | `render_launch_screen` | `tui/underwater.rs` | — | none | `startup_{w}x{h}` |
| Quick actions | 3 command rows | selected/hover/disabled (no model) | `LaunchAction`, provider state | launch menu rows | `tui/underwater.rs` + `mouse_ui.rs:441` | ↑/↓, Enter, Esc | row rects (exists) | `startup_*` |
| Option strip | 4 columns (worktree/chat/theme/help) | hover/selected | `LaunchState` | launch options row | same | Tab, Enter | 4 col rects | `startup_*` |
| Rail | Left column, 5 groups + collapse | expanded/collapsed/focused | `WorkSurfaceState`, `subagent_cache`, run list, git status | work strip + `sidebar` remnants | `tui/work_surface/` (#5699 territory) | Tab, ↑/↓, Enter, `«` | `WorkHitbox{WorkRowId,row_y}` (exists) | `work_{w}x{h}` |
| Receipt stream | Turn + receipt rows, pod tree | streaming/settled; selected | `history` cells, pod formation receipt | transcript rail | `tui/history.rs`, `work_surface/render` | ↑/↓, Enter inspect | row rects (transcript click path) | `work_*` |
| Fleet ledger | Whale table | row selected; state per whale | `subagent_cache` + worker runtime states | workflow-panel duplicate | `tui/work_surface/panels.rs` | ↑/↓, Enter/click inspect | row rects → inspector | `ledger_{w}x{h}` |
| Theme list | 13 themes + motion toggles | selected/preview/applying | `ThemeId`, `ocean_treatment`, `low_motion`, `fancy_animations` | `theme_picker.rs` | `tui/theme_picker.rs`, `views/` | ↑/↓, Enter preview/apply | row rects | `settings_{w}x{h}` |
| Live preview | Projection of a real screen in chosen theme | mirrors screen state; never a second store | same render fns, `TestBackend`-style projection into the pane | settings preview | `tui/views/` settings | — | none (passive) | `settings_*` |
| Settings rail | 8 categories + meta rows | selected | `ConfigView` | `ConfigView` nav | `tui/views/mod.rs` | ↑/↓, Tab | category rects | `settings_*` |
| Composer | Input with rounded border + send hitbox | focus, pending crumb, approval-replaced | `ComposerState`, pending preview | composer_ui/chrome (extends) | existing composer files | Enter, ⇧Enter, Esc | `[↑]` submit rect; border focus click | `composer_{w}x{h}` |
| Footer | One band: phase·cost (left), depth line·keys (right) | per-phase ink; 80% warn | `SessionState` cost, phase, `context_budget` | slots 6+8 merged | `tui/phase_strip.rs` | — | depth segment → context inspector | `footer_{w}x{h}` |
| Notifications inbox | Attention rows (gold ◆) | unread/read; per-kind | `status_toasts`/`sticky_status` → typed records | toast soup | `tui/notifications.rs` | Enter, `r`, Esc | row rects | `notifications_{w}x{h}` |

No component lacks lineage; the two deletions (slots 4, 5) and two merges
(3, 6+8) are stated above.

## 5b. Layout contract (copy-pasteable)

Screen shell — all three screens:

```rust
let [topbar, stage, footer] = Layout::vertical([
    Constraint::Length(1),  // topbar (implemented)
    Constraint::Min(1),     // stage: the screen
    Constraint::Length(1),  // footer: phase·cost · depth·keys (merged 6+8)
]).areas(area);
```

Startup stage:

```rust
let [hero, rule_a, quick, rule_b, strip, spacer] = Layout::vertical([
    Constraint::Percentage(38), // hero: heading + subtitle
    Constraint::Length(1),      // wave rule `⋯ ∼∼∼ ⋯` (dim, static)
    Constraint::Length(3 + 2),  // QUICK ACTIONS: label row + 3 rows + margins
    Constraint::Length(1),      // wave rule
    Constraint::Length(3),      // option strip: 4 columns × 2 rows
    Constraint::Min(1),
]).areas(stage);
// options strip:
let [wt, chat, theme, help] = Layout::horizontal([
    Constraint::Ratio(1, 4); 4]).areas(strip);
// composer+footer below stage per screen shell (composer Length(4) incl. border)
```

Work + Pod stage:

```rust
let rail_w = if width >= 120 { 22 } else if width >= 100 { 16 } else { 0 };
let [rail, main] = Layout::horizontal([Constraint::Length(rail_w), Constraint::Min(1)])
    .areas(stage);
let [stream, ledger, composer] = Layout::vertical([
    Constraint::Min(6),                    // receipt stream
    Constraint::Length(1 + 1 + rows.min(4)), // POD LEDGER: header + rows
    Constraint::Length(4),                 // composer
]).areas(main);
// ledger columns at main width w (rows are 1 line, never wrap):
let cols = if w >= 130 {
    [Length(10), Min(8), Length(12), Length(8), Length(8), Length(8)] // +LAST UPDATE
} else if w >= 110 {
    [Length(10), Min(8), Length(12), Length(8), Length(8)]            // +RECEIPTS
} else {
    [Length(10), Min(8), Length(12)]                                  // core three
};
```

Settings stage (3-pane):

```rust
if width >= 100 {
    let [nav, form, preview] = Layout::horizontal([
        Constraint::Length(18), Constraint::Min(30), Constraint::Percentage(38),
    ]).areas(stage);
} else {
    let [nav, form] = Layout::horizontal([Constraint::Length(16), Constraint::Min(20)])
        .areas(stage); // preview sheds
}
```

**Shed order (global, what collapses first):** ① topbar Theme segment →
② topbar Workspace → ③ topbar Whales → ④ Pod → ⑤ Run/breadcrumb →
⑥ settings preview pane → ⑦ rail (100 → 0) → ⑧ ledger time columns
(LAST UPDATE → RECEIPTS → ELAPSED) → ⑨ clock sheds date prefix →
⑩ option strip 4→2 columns. Transcript and composer never shed below their
existing floors (`MIN_CHAT_HEIGHT = 3`, `MIN_COMPOSER_HEIGHT = 2` + border).
Topbar ①–⑤ and ⑨ are implemented and tested.

## 5c. Golden buffers

12 files, cell-exact, under `crates/tui/src/tui/goldens/`:
`topbar_{startup,work,settings}_{80x24,100x30,120x32,160x40}.txt` ✅
(`BLOCKER_SIZES` from `views/status_picker.rs:423`). The remaining 8 names in
§5a are reserved; each lands with its component. Re-bless:

```sh
CODEWHALE_BLESS_GOLDENS=1 ./scripts/dev-test.sh tui topbar
```

The harness (`topbar/tests.rs`) renders through `TestBackend`, dumps
`Buffer` cell symbols, and asserts byte-exact equality; a missing golden
fails the test unless blessing. Goldens are the design contract — a visual
change that cannot show as a golden diff did not happen.

## 5d. Color contract

Semantic roles only — `ChromeInk` (17 variants, `palette/grammar.rs:49`) via
`chrome_style`; **no hex in widgets**, presets stay in `palette/`. Grammar
families obey `STATUS_BAR_COLOR_GRAMMAR.md`:

| Element | ChromeInk | Family |
|---|---|---|
| Wordmark | `Attention` (gold) | Cognition — restrained text identity |
| Segment labels / separators / clock | `Metadata` / `MetadataDim` / `MetadataHint` | Metadata |
| Route · model · run | `Identity` | Identity |
| Pod (live) / `3/4` / context meter / theme name | `Active` / `Info` | Active / Identity |
| `not connected` / ≥80% meter | `Waiting` / `Attention` | Cognition |
| Receipt working / done / caution / failed | `Active` / `Outcome` / `Attention` / `Failure` | Active/Outcome/Cognition/Failure |
| Ledger `failed` (preview error row) | `Failure` | Failure — red stays failure-only |

Resolution paths: truecolor presets → ANSI-256 → ANSI-16 injective role
matrix (`palette/adapt.rs:598`, `role.ansi16()`), ANSI-16 backgrounds drop to
`Color::Reset` (`adapt_bg`). **Gap found and required by this spec:** add
`NO_COLOR` detection to `ColorDepth::detect` forcing the mono/ascii-safe path
(currently only `child_env.rs:214` passes it through; the TUI itself ignores
it). Dark/Light/Deepsea all resolve because widgets never name a color.

## 5e. Motion table (wall-clock keyed; `MotionMode`-gated; still frames carry the facts)

| Motion | Trigger state | Cadence | Cell footprint | Still frame | Gate | Mechanism |
|---|---|---|---|---|---|---|
| Echolocation | turn active (any phase) | 420 ms × 6 frames (`widgets/header.rs:13`) | 7 cells, band; 1 cell chip; never shifts adjacent text | static `<·>` (chip: `·`) | Reduced freezes frame 0; Still static | pure fn of `(turn_started_at, Instant::now)` beside `header_status_indicator_frame`; replaces braille `spinner.rs` frames |
| Spout | turn success, once (suppressed if next turn <400 ms) | one-shot 900 ms, ease-out, droplets +60–110 ms | 1 col × 3 rows above activity band's right edge; ≤4 cells | skipped — `surfaced ✓ …` summary line carries it | `Full` only | `surfaced_at: Instant` + `FrameRequester::request_at` for the 900 ms window, direct `Buffer` writes (`paint_marks` technique); failure surfaces flat `✗`, no motion |
| Bioluminescent tail | tokens streaming | decay 1.5 s (existing), 12 graphemes | ≤12 cells restyled, no new draws | frozen settled brightness (current behavior) | Reduced/Still keep freeze semantics | extend `hot_tail.rs`: keep `HOT_TAIL_GRAPHEMES=12`, `breath_luminance` period; add accent-tint step truecolor, bold-only ANSI-16 |
| Depth line | token count changes only (turn boundaries + stream ticks) | no private clock | ≤16 cells in footer right | identical (static per count) | always visible, all modes | hand-rolled span builder (not `Gauge`): `▁▂▄▆∿▄▄▄▄ 61%`; ≥80% cap `▲` Attention + `surface soon — /compact` |
| Idle breath | idle, nothing else moving | 4.2 s cosine luminance, 1 cell | header bit `·` dim→bright→dim | dim `·` | `Full` only | same cell, luminance-only function of `Instant`; wave rules `⋯ ∼∼∼ ⋯` are static `Span`s |

Cadence law: everything is a function of `Instant`/wall clock (echolocation
reuses the 420 ms heartbeat), never a frame counter; `FrameRequester` stays
the only additional-frame source and the poll loop the only draw emitter.

## 6. Clickability contract (the grokbuild bar)

Every selectable target follows the registry discipline from
`tideline-redesign.html` ("Every navigates"): visible focus, recorded hitbox,
keyboard + mouse parity, an inspect action, no decorative glyph asks to be
clicked. Mechanically this repo already has the pattern — `WorkHitbox`
storage, `viewport.last_workflow_panel_area`/`last_workflow_cancel_area`,
`launch.row_areas`, `sidebar_hover.sections` row matching
(`mouse_ui.rs:270,441,695`). This spec adds **one** new registry:
`viewport.last_topbar_hitboxes: Vec<(TopbarSegmentId, Rect)>` filled by
`topbar::topbar_hitboxes` at render (same shapes as painted cells — asserted
by test) and consumed in `mouse_ui` exactly like the workflow-panel cancel
zone. Hover = value ink brightens + underline (tested); press = the segment's
action (`Model→` provider inspector, `Pod→` ledger, `Context→` context
inspector, `Theme→` theme picker, `Run→` run dashboard, `Brand→` menu).
Keyboard parity: Tab cycles topbar → rail → stream → ledger → composer;
Enter activates the focused target; Esc backs out. Ledger rows, quick
actions, theme rows, and option-strip tiles reuse the row-rect pattern above —
no second mouse system is introduced.

## 7. Moment map (craft lens; surprise budget ≤3 per screen)

Ladder per screen — enter → orient → hover → press → action begins → waiting
→ completion → failure → recovery → exit — with the moments that must feel
authored, everything else staying calm:

- **Startup.** *Enter:* hero breathes once (idle breath) — the quietest
  "I'm here". *Waiting (connect):* echolocation on the model segment; the
  word `not connected` itself stays still. *Completion (first connect):*
  **spout** — the one payoff moment; suppressed if work resumes instantly.
  *Failure:* flat `✗` + the model segment says what to do next (microcopy:
  "no route yet — /connect"), never "Error". *Exit into work:* composer
  keeps focus — no modal re-orientation. Surprise budget: spout and hero
  breath; composer focus changes only its border and send affordance.
- **Work + Pod.** *Orient:* the pod-formation tree draws its `├──└──` rows in
  one ≤600 ms top-down reveal, then is forever still (continuity: the tree is
  the same object in the ledger below). *Waiting:* receipt rows carry typed
  state marks (`●○✓!`) + echolocation only on the live row — no duplicated
  status anywhere (footer owns phase, ledger owns whales, stream owns events).
  *Press:* ledger row `▶` + inspect opens beside the evidence, never
  replaces it. *Failure:* the caution/failed receipt is Attention/Failure ink
  **plus a word** — color never invents state. Surprise budget: formation
  reveal, the legend row that teaches the marks in place.
- **Settings.** *Orient:* the live preview is a real projection of the real
  renderers — what you see is what ships. *Press (apply theme):* one depth-
  line ripple across the preview's context meter, one-shot, ≤16 cells —
  change you can feel without a transition festival. *Hover:* theme rows
  preview on hover-delay (300 ms) only in `Full` motion. *Recovery:* Esc
  restores the pre-preview theme with no flicker (projection, not mutation).
  Surprise budget: the apply ripple, Deepsea's silent ambient life (empty-
  cells-only, opt-in, settles under low motion).

Accessibility is non-negotiable throughout: every motion above has a still
frame; ANSI-16 and NO_COLOR carry meaning in glyph/intensity, not hue; every
target is keyboard-reachable with visible focus; red stays failure-only.

## 8. Acceptance status

- ✅ Topbar implemented end-to-end: deterministic widget, shed order, hitboxes,
  ascii-safe, meter warn ink, hover — **6/6 tests green** via
  `./scripts/dev-test.sh tui topbar` (6 run, 11,329 skipped; nextest).
- ✅ 12/12 topbar goldens written and asserted at the four blocker sizes.
- ✅ Spec covers every region of all three screens with lineage (§5a), real
  constraint lists (§5b), semantic color only (§5d), gated wall-clock motion
  with still frames (§5e), clickability registry (§6), moment map (§7).
- **Partial (named):** the remaining components (hero, quick actions, option
  strip, rail, receipt stream, ledger, theme list, live preview, settings
  rail, composer restyle, footer merge, notifications) are specified but not
  implemented; their 8 reserved golden names are empty until each lands.
  Wiring `topbar` into `frame.rs` is deliberately **not** done here — it
  collides with live writers #5698/#5699 and belongs to the first landing
  slice after their bases settle.

Not authorized / not done: no merge, no PR, no deploy, no `BASE_PROMPT` or
`motion/ethos.rs` edits, no billing, no writes into other agents' worktrees.
