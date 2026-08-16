# Locale agent guidance

`en.json` is the reference pack. Tests define which other packs require exact
parity; read those tests instead of maintaining a list here.

For every new string:

1. Add the `MessageId` variant, `ALL_MESSAGE_IDS` entry, and English key.
2. Translate every complete pack. Do not satisfy parity by copying English.
3. If an English value changes, update its translations as well.

Keep `{named}` placeholders literal. Commands, key names, URLs, product terms,
and glyphs follow the conventions enforced by localization tests; ordinary
prose should be natural and compact. Preserve intentional edge whitespace.

Adding a locale also requires its `Locale` registry/display/parse entries,
onboarding picker entry, `UiLocale` schema value, and setup/config match arms.
Registry agreement tests are the source of truth. Translated root READMEs are a
separate surface guarded by `scripts/check-readme-translations.py`.
