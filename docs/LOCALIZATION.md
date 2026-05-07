# Localization Matrix

Status date: 2026-04-29

This document tracks UI localization only. It does not change model output language, provider behavior, or DeepSeek payload support. Media attachments remain local path text references unless native media payload support is added separately.

## Source Audit

The v0.7.6 parity check used live GitHub sources with `/opt/homebrew/bin/gh`.

| Project | Ref | Evidence | Result |
|---|---:|---|---|
| Codex CLI | `openai/codex@df966996a75333add031fca47b72655e9ee504fd` | `gh repo view openai/codex`; recursive tree scan for `locale`, `i18n`, `l10n`, `translation`, `messages`; README language scan | No checked-in CLI UI localization registry found in the audited tree. Treat Codex CLI parity as English-first terminal UI behavior, not a source for shipped locale tags. |
| opencode | `anomalyco/opencode@00bb9836a60f1dcdd0ce5078b05d12f749fdde66` | `packages/console/app/src/lib/language.ts`, `packages/app/src/context/language.tsx`, `packages/web/src/i18n/locales.ts`, `packages/app/src/i18n/parity.test.ts` | opencode ships app/docs locale infrastructure with language detection, locale labels, docs locale aliases, RTL direction for Arabic, and parity tests for targeted keys. |

## v0.7.6 Shipped Core Pack

These locales are supported by `locale` in `settings.toml` and by `LANG` / `LC_ALL` auto-detection.

| Locale | Display | Script | Direction | Fallback | Priority tier | v0.7.6 scope | Notes |
|---|---|---|---|---|---|---|---|
| `en` | English | Latin | LTR | `en` | Baseline | Source strings remain canonical. | English is always available. |
| `ja` | Japanese | Jpan | LTR | `en` | v0.7.6 must-have | Core TUI chrome | Covers composer placeholder/history search, help chrome, and `/config` chrome. |
| `zh-Hans` | Chinese Simplified | Hans | LTR | `en` | v0.7.6 must-have | Core TUI chrome | `zh`, `zh-CN`, and `zh-Hans` resolve here. Traditional Chinese is not shipped. |
| `pt-BR` | Portuguese (Brazil) | Latin | LTR | `en` | v0.7.6 must-have | Core TUI chrome | `pt` and `pt-PT` currently fall back to Brazilian Portuguese; European Portuguese is not separately shipped. |

Selection:

```toml
locale = "auto"     # default; checks LC_ALL, LC_MESSAGES, then LANG
locale = "ja"
locale = "zh-Hans"
locale = "pt-BR"
```

Fallback:

- Missing or unsupported configured locales fall back to English.
- `auto` falls back to English when no supported environment locale is detected.
- The resolved locale is included in the system prompt and used as the default
  natural language for V4 reasoning and replies. Users can still switch
  languages mid-session by writing in a different language.

## Planned Global South QA Matrix

These are not claimed as shipped translations in v0.7.6 unless a later change adds complete message coverage and QA evidence.

| Locale | Display | Script | Direction | Priority tier | Coverage status | Fallback | QA status | Layout risks |
|---|---|---|---|---|---|---|---|---|
| `ar` | Arabic | Arab | RTL | Follow-up | Planned | `en` | Automated renderer sample only; native review required before shipping | RTL ordering, punctuation, key-chord mixing |
| `hi` | Hindi | Deva | LTR | Follow-up | Planned | `en` | Automated renderer sample only; native review preferred before shipping | Combining marks, cursor width, truncation |
| `bn` | Bengali | Beng | LTR | Follow-up | Planned | `en` | Matrix only; native review required before shipping | Combining marks, line wrapping |
| `id` | Indonesian | Latin | LTR | Follow-up | Planned | `en` | Matrix only; automated narrow-width snapshots and reviewer pass required | Longer labels than English |
| `vi` | Vietnamese | Latin | LTR | Follow-up | Planned | `en` | Matrix only; automated width snapshots and reviewer pass required | Diacritics and wrapped labels |
| `sw` | Swahili | Latin | LTR | Follow-up | Planned | `en` | Matrix only; native or fluent review required before shipping | Translation quality, longer command descriptions |
| `ha` | Hausa | Latin | LTR | Follow-up | Planned | `en` | Matrix only; native or fluent review required before shipping | Diacritics and terminology |
| `yo` | Yoruba | Latin | LTR | Follow-up | Planned | `en` | Matrix only; native or fluent review required before shipping | Tone marks and terminology |
| `fil` | Filipino/Tagalog | Latin | LTR | Follow-up | Planned | `en` | Matrix only; source strings required before shipping | Terminology consistency |
| `es-419` | Spanish (Latin America) | Latin | LTR | Follow-up | Planned | `en` | Matrix only; reviewer pass required before shipping | Regional terminology |
| `fr` | French | Latin | LTR | Follow-up | Planned | `en` | Matrix only; reviewer pass required before shipping | African locale terminology varies |

## Message Coverage

The first registry pass covers stable message IDs for high-visibility terminal chrome:

- composer placeholder
- composer history search title, placeholder, hints, and no-match state
- `/config` title, filter placeholder, no-match state, filtered count, and footer hints
- help overlay title, filter placeholder, no-match state, section labels, and footer hints

Not yet translated in v0.7.6:

- model/system prompts and personalities
- provider or tool schemas
- full slash-command descriptions and every status/toast/error path
- README/docs content beyond this configuration note

## Translator Notes

Keep these technical terms stable unless a later glossary explicitly changes
them: `Plan`, `Agent`, `YOLO`, `/config`, `/mcp`, `@path`, `/attach`, `DeepSeek`,
`MCP`, `CLI`, `TUI`, and key chords such as `Enter`, `Esc`, `Tab`, `PgUp`, and
`PgDn`.

## QA Checklist

Before promoting a planned locale to shipped:

1. Add complete message coverage in `crates/tui/src/localization.rs`.
2. Add locale resolution tests and missing-key tests.
3. Add narrow-width render coverage for at least composer, help, and `/config`.
4. Verify CJK width, RTL punctuation, combining marks, and truncation.
5. Record native/fluent review status, or mark the locale as automated-QA-only.
