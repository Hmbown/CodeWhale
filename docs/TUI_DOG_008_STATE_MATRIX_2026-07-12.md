# TUI-DOG-008 — reference-state matrix

> **2026-07-13 overnight update.** The color grammar and several motion gaps
> below were closed on the same dirty lane:
> live/done pairing now matches the implemented release grammar (seafoam `#4fd1c5` = live work,
> working green `#9bd66f` = settled ✓; the old sky-blue working /
> teal-success inversion is gone); waiting/approval read coral and failed
> reads rose; the whale mark carries the seafoam spout + ivory eye; ambient
> fish are two sky-blue shades so teal stays exclusively "live"; the ombre
> shimmer runs on the ~90 s ambient cadence and provably freezes on
> waiting/approval/failed; completion takes the one-shot 800 ms field
> brightness breath; and a first-class `ShellPhase::Verifying` renders the
> metered braille tick (distinct from the working bubble) for live
> test/check runs. Hunter's live eyeball remains the acceptance gate — the
> "Still needs Hunter eyeball" list below still applies, now with the new
> color grammar and verifying tick added to it.

Dated receipt: 2026-07-12
Lane: `codex/underwater-tui-20260711` @ worktree `codewhale-underwater-tui`
Reference basis: current TUI implementation plus real-terminal release QA.
Private design prototypes are intentionally not part of this public receipt.
Scope: visual metamorphosis / shell grammar (not Fleet, keybinding, or hitbox siblings)

Status values: `implemented` | `partial` | `missing`
A hierarchy design aid is retained in the private workspace archive (not
acceptance proof). No dogfood reinstall on this pass. Ownership is code truth;
Hunter eyeball remains the acceptance gate.

### Focused tests run (2026-07-12)

- `tui::phase_strip::tests::*` — 2 passed
- `tui::composer_chrome::tests::*` — 4 passed
- `composer_height_uses_quiet_rule_when_panel_is_not_needed` — passed (quiet baseline now 4)
- `underwater::tests::phase_markers_make_motion_and_attention_explicit` — passed
- `cargo check -p codewhale-tui --bin codewhale-tui` — clean at time of check
  (siblings concurrently dirty other modules; do not treat full-suite green)

## State matrix

| State | Release interaction contract | Status | Owning modules | Notes / gap |
| --- | --- | --- | --- | --- |
| **idle** | Whale empty water; roomy `❯` hint; quiet `idle` phase; ambient fish only in empty water | **partial** | `underwater.rs`, `ocean.rs`, `widgets` ambient, `composer_chrome.rs` | Empty-state whale + fish exist. Composer baseline was a cramped 1-line quiet rule; chrome policy now reserves multi-line breathing room and sheds padding before content at compact heights. Ambient life still needs live PTY proof under `env -u NO_COLOR`. |
| **typing** | Fish flee; `›` / draft phase; composer owns attention | **partial** | `underwater.rs` (`ShellPhase::Typing`), `composer_ui.rs`, `widgets::ComposerWidget` | Phase + flee-on-engage exist. Focus/shortcut ownership is sibling TUI-DOG-002/003 — not claimed here. |
| **working** | Ledger: prompt → short narration → settled `✓` receipts → **one** live row; phase band **above** `❯`; quiet composer | **partial → landing** | `phase_strip.rs`, `active_cell.rs`, `history` tool cells, `widgets::ChatWidget` | Before this pass: phase was the bottom footer under the composer (the 2026-07-12 audit had the order reversed). Now live phases place the strip above the composer. Ledger density/narration still needs Hunter eyeball; remove any remaining classic sidebar status echo in classic-only path. |
| **tool receipt** | Settled rows dim; live mark only on the active tool | **partial** | `active_cell.rs`, `history.rs`, `tool_routing.rs` | One active cell + settled flush is correct ownership. Visual “settled vs live” contrast and the single live mark need real-terminal comparison. |
| **workers open** | Top/side work surface: Tasks/To-do rows with open/stop; one focus owner | **partial** | `work_surface/*` | Surface + placements exist. Selection/scroll/Stop arm are sibling TUI-DOG-004/005/006. |
| **waiting for user** | Coral still `◆` / `?`; ambient frozen; phase above composer | **partial → landing** | `phase_strip.rs`, `underwater.rs` | Marker/color/stillness typed. Placement now follows the live-phase-above-composer rule. Attention band copy still needs real-terminal review. |
| **approval** | Modal/approval owns the decision; shell phase coral | **partial** | `approval.rs`, `widgets::ApprovalWidget`, `phase_strip.rs` | Approval modal exists; phase maps from `ModalKind`. Not a visual parity claim for the card itself. |
| **failed** | Coral `✕`; persists until acknowledged; no jitter | **partial** | `underwater.rs`, `phase_strip.rs` | Marker + sticky failed phase exist. Persistence-until-ack UX needs live confirm. |
| **done** | One `finishing → ✓ done` breath; then stable; composer bottom | **partial → landing** | `underwater.rs` completion breath, `phase_strip.rs` | Footer-only exhale exists. With phase-above placement, completion now sits on the strip above `❯`, not under it. |

## Layout grammar (this pass)

| Concern | Before | After (this issue) |
| --- | --- | --- |
| Live phase vs composer | Composer above; phase footer always bottom | Idle/typing: composer above, quiet phase below. Working/waiting/approval/failed/done: **phase strip above**, composer is the final bottom object |
| Composer baseline | Min rows only when enclosed panel; empty → ~1 content row | `composer_chrome` always budgets density min rows when height allows; compact sheds padding/chrome before content |
| Phase ownership | `underwater::render_footer` only | `phase_strip` owns placement + band; underwater footer remains the render entry for classic/compat callers |
| Duplicate facts | Classic footer/sidebar can restyle the same busy label | Ocean path: phase strip owns phase/cost/detail keys; header owns route/ctx; work surface owns Tasks/To-do |

## Module map

| Module | Role |
| --- | --- |
| `crates/tui/src/tui/phase_strip.rs` | Phase placement + band render (above/below composer) |
| `crates/tui/src/tui/composer_chrome.rs` | Roomier baseline, padding-shed-before-content |
| `crates/tui/src/tui/underwater.rs` | `ShellPhase`, markers, header/empty water |
| `crates/tui/src/tui/ocean.rs` | Ombre/flat treatment + ambient ink |
| `crates/tui/src/tui/work_surface/` | Tasks / To-do / workers strip & rails |
| `crates/tui/src/tui/active_cell.rs` | One live tool group |
| `crates/tui/src/tui/ui.rs` | Layout wiring only (preserve dirty Fleet hunks) |

## Still needs Hunter eyeball

1. Working turn reads as a ledger (not a dashboard) at 80×24 and 100×32.
2. Phase strip sits directly above `❯` while a turn is live; idle keeps quiet phase under the prompt.
3. Composer feels roomy at comfortable density and does not collapse to a one-line afterthought at 60×16 / 40×12 (padding sheds first).
4. State change is legible with `low_motion` on (markers/color/placement carry meaning; braille is enhancement only).
5. No duplicate “working” facts between strip, transcript, and work surface.

## Conflict notes

- Dirty Fleet files (`fleet/profile.rs`, `fleet/roster.rs`, Fleet identity hunks in `ui.rs`) are untouched by this issue.
- Siblings own TUI-DOG-001–007 / 009–011. Do not merge their selection/keybinding work into this receipt.
- A prior internal audit claimed footer-below-composer as finished; live dogfood
  (TUI-DOG-008) overrides that for acceptance.
- No push / commit / install on this pass.
