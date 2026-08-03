# crates/tui/locales — agent guidance

UI packs. `en.json` is the reference. Which packs are **complete** (held to
exact raw key parity with English) and which are intentionally partial is
defined by the tests, not by this file — read them rather than a list that goes
stale on the next locale PR.

## Adding or changing a string

1. Add the `MessageId` variant, the `ALL_MESSAGE_IDS` entry, and the `en.json`
   key — all three, or `message_id_list_english_pack_stay_in_exact_sync` fails.
2. Translate into every complete pack, or
   `shipped_complete_packs_have_raw_key_parity_with_english` fails. Do not "fix"
   that test by copying English into a pack — the silent English fallback is
   invisible at runtime, so the gate is the only thing between users and
   untranslated UI.
3. If you change an **existing English value**, retranslate it everywhere. Value
   drift is invisible to the key gates; say what you changed in the commit body.

## Translation conventions

- `{named}` placeholders stay literal; call sites substitute with `.replace()`.
- Product terms stay English per pack convention: Fleet, Plan / Act / Operate,
  Ask / Auto-Review / Full Access. Plain words ("read only", phase words)
  translate naturally and must stay short — footers and row controls render them
  in tight budgets.
- Key names, commands, and glyphs are never in translations; they are composed
  in code.
- Preserve intentional leading/trailing spaces (pane titles, `Rule  `, the
  slash-menu hint).
- Script rules: ru/uk prose is Cyrillic only (uk uses і/ї/є/ґ, never ы/э/ъ); hi
  prose is Devanagari. Latin appears only in product terms, commands, key names,
  placeholders, and URLs. Script-purity fixtures in `localization.rs` enforce
  this for high-visibility strings.

## Adding a locale

Pack JSON with full parity, `Locale` variant + tag/display/parse arms in
`localization.rs`, onboarding picker entry (`language.rs` — a test forces every
shipped locale to be offered), the typed `UiLocale` schema in `config_ui.rs`,
setup-wizard match arms, and locale display arms in the config/change commands.
The `/config` hint and invalid-locale error derive from `Locale::shipped()`
automatically; the schema agreement test keeps `UiLocale` aligned with that
registry. Picker hotkeys run `1..=9` then `a`, `b`, … so more than nine locales
stay single-keystroke selectable.

Translated READMEs (repo root) are separate from these packs but follow the same
discipline: `scripts/check-readme-translations.py` fails when English changes
without the translations being refreshed and restamped.
