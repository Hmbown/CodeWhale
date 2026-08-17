# DeepSeek Harness connected through Codewhale

`codewhale integrations dsh …` connects a user's **existing** official DeepSeek
Harness installation (`dsh`, npm `@deepseek-ai/dsh`) to their Codewhale setup.
DSH stays an integrated harness surface. Codewhale remains the owner of Fleet
configuration, provider/model selection, permissions, credentials, and
lifecycle authority; DSH is not a second Fleet scheduler and never an
authority bypass.

Verified against `dsh 0.1.0-rc.6` (the latest published release at the time
of writing). DSH is a developer preview that warns of compatibility-breaking
changes; a newer `dsh` is reported as `stale-version` (launchable, unverified),
an older one or one without `--patch` as `incompatible`.

## What is (and is not) connected

Codewhale uses only DSH's documented seams:

| Seam | How Codewhale uses it |
| --- | --- |
| `dsh --version` / `dsh --help` | read-only detection (never initializes a profile) |
| `$DSH_HOME` (or `~/.dsh`) | read-only inventory: profile names, `settings.yaml` top-level namespaces, whether `.credentials.yaml` exists and is `0600`. Values are never read. |
| `--patch <file>` overlay | Codewhale writes **one** overlay under its own home and passes it at launch |
| `DSH_PERMISSION_MODE` env | mirrors the Codewhale permission posture |
| `--profile web` / `--profile headless` | the two shipped DSH profiles; DSH initializes them itself on first launch (its own documented behavior) |

Codewhale writes **only** under `$CODEWHALE_HOME/integrations/dsh/` (plus,
with the opt-in plugin path below, whatever `dsh plugin` itself writes into
the dedicated `codewhale` DSH profile):

- `codewhale.patch.yml` — the overlay. Identity only: provider route, model,
  base URL, and (native DeepSeek route) `reasoningEffort`. For every
  non-native route it declares a `codewhale-<provider>` route on DSH's
  `llm-pi-ai` adapter, naming that route's own wire dialect under `api:`
  (`openai-completions`, `openai-responses`, or `anthropic-messages`) and
  `apiKeyEnv` naming the provider's canonical environment variable — the
  *name*, never the value. Keyless local routes (loopback Ollama / LM Studio
  / vLLM / SGLang) carry no credential reference.
- `receipt.json` — the current connection record plus an append-only history
  of `connect` / `update` / `disable` / `enable` / `remove` events with the
  overlay SHA-256, dsh version, `$DSH_HOME`, mapped identity, permission mode,
  and timestamps (see `docs/RECEIPTS.md`). Every event is also appended to
  `$CODEWHALE_HOME/audit.log`.
- `codewhale-dsh-skin.css` and `codewhale-dsh-skin-preview.html` — only with
  `--skin`; see below.
- `bundle/` — only after `install-bundle`; see below.

Codewhale **never**:

- copies, prints, or embeds API keys, OAuth documents, environment secrets,
  prompts, or filesystem contents (a `--api-key`/keyring credential Codewhale
  itself materialized into the process is stripped from the launched child;
  a key the user exported in their own shell is left alone);
- writes to `$DSH_HOME` (settings, credentials, profiles, sessions);
- edits installed `@deepseek-ai/dsh` package files;
- switches to a cloud model or broadens permissions silently. Codewhale
  `read-only` → DSH `read-only`; anything else → `workspace-write`;
  `danger-full-access` only with `--allow-full-access` **and** a Codewhale
  full-access posture (`sandbox_mode = "danger-full-access"` / yolo).

## States

| State | Meaning | Launch |
| --- | --- | --- |
| `not-installed` | `dsh` not on `PATH` | refused |
| `offline` | `dsh` exists but `--version` failed | refused |
| `incompatible` | older than 0.1.0-rc.6 or no `--patch` | refused |
| `detected` | usable dsh, no Codewhale overlay | refused (`connect` first) |
| `connected` | overlay matches the current Codewhale route | allowed |
| `stale-config` | route changed, overlay edited outside Codewhale, or missing | refused (`update`) |
| `stale-version` | connected, but dsh is newer than verified | allowed, unverified |
| `disabled` | overlay kept, launches refused | refused (`enable`) |

`status`, `plan`, `/setup tools` (Tools and MCP step) and `codewhale doctor`
are side-effect free.

## Commands

```bash
codewhale integrations dsh status [--json]
codewhale integrations dsh plan [--profile web|headless] [--allow-full-access] [--skin] [--json]
codewhale integrations dsh connect [--profile web|headless] [--allow-full-access] [--skin] [--yes]
codewhale integrations dsh update  [--profile …] [--allow-full-access] [--skin true|false] [--yes]
codewhale integrations dsh launch  [--profile web|headless] [--dry-run] [-- <dsh app args>]
codewhale integrations dsh disable
codewhale integrations dsh enable
codewhale integrations dsh remove [--yes]
codewhale integrations dsh install-bundle [--app web|headless] [--yes]
codewhale integrations dsh remove-bundle [--yes]
```

`connect`, `update`, and `remove` print the exact plan (files, identity,
permission mode, disclosures, and the overlay text) and require confirmation
(`--yes` when stdin is not a terminal). `launch` runs
`DSH_PERMISSION_MODE=<mode> dsh --profile <p> --patch <overlay> …` in the
Codewhale workspace with the user's own `$DSH_HOME`, so their credentials,
sessions, and profiles remain theirs.

### Disclosures the plan makes

- DSH layers the user's `settings.yaml` sections (`agent-default-model`,
  `llm-deepseek`, `llm-pi-ai`) over the overlay per field. If those sections
  exist, DSH's saved selection can shadow the pinned identity until it is
  cleared in DSH; `status`/`plan` list them.
- Reasoning tiers are mapped only for the native DeepSeek route
  (`off|high|max`); hand-declared routes send no effort parameter.
- Wire dialects are carried, never approximated: a Chat Completions route
  declares `api: openai-completions`, an OpenAI Responses route (e.g. the
  default `deepseek/deepseek-v4-flash`) declares `api: openai-responses`,
  and an Anthropic Messages route declares `api: anthropic-messages`. This
  follows the installed adapters' own declarations (verified against
  `@deepseek-ai/dsh@0.1.0-rc.6`): `@deepseek-ai/dsh-llm-deepseek` — the
  `deepseek-official` route — speaks chat completions only (its single wire
  call posts to `<baseURL>/chat/completions`, with no protocol switch),
  while `@deepseek-ai/dsh-llm-pi-ai`'s hand-declared route schema accepts
  exactly `openai-completions | openai-responses | anthropic-messages` for
  `api:`. So DeepSeek chat routes ride the native adapter (with reasoning
  tiers), and every other dialect — including DeepSeek's own
  Responses-dialect models — rides a hand-declared `codewhale-*` pi-ai
  route in its own dialect.
- What is refused: base URLs that embed credentials (userinfo or
  query/fragment material) are never copied into the overlay; `plan` fails
  naming the current `provider/model` and the reason, and `status` shows
  carry-ability for the current route before `plan` is ever run.

## The DSH plugin path (`install-bundle`)

`--patch` is Codewhale's default because it needs nothing but the launcher.
The **documented DSH plugin mechanism** is available as an explicit opt-in:

```bash
codewhale integrations dsh install-bundle [--app web|headless] [--yes]
codewhale integrations dsh remove-bundle [--yes]
```

`install-bundle` requires an existing connection and `pnpm` on `PATH` (dsh
shells out to it); without pnpm the status reads
`plugin path: not available: pnpm missing …` and the command refuses. It:

1. materializes an npm-shaped bundle package under
   `$CODEWHALE_HOME/integrations/dsh/bundle/` — `package.json`
   (`codewhale-dsh-bundle`, private, MIT, version
   `<codewhale version>+dsh.<patch sha12>`, `"dsh": {"bundle": {"patch":
   "./cordis.patch.yml"}}`), `cordis.patch.yml` (byte-identical to the
   overlay), `README.md`, `NOTICE.md` (DSH MIT notice retained);
2. runs the documented `dsh plugin --profile codewhale add <path>` twice: first
   for DSH's own shipped app bundle (`@deepseek-ai/dsh-web-app` or
   `dsh-headless`, linked from the installed launcher so the profile can boot;
   no network), then for the Codewhale bundle so its rows patch last. DSH
   creates the **dedicated** profile `$DSH_HOME/profiles/codewhale`
   (`package.json` with `link:` dependencies, `pnpm-lock.yaml`,
   `node_modules` links). The user's `web`/`headless` profiles are never
   touched;
3. records an `install_bundle` receipt (profile dir, bundle dir, package
   version, patch SHA-256, app bundle source, pnpm version, SHA-256 digest of
   the `dsh plugin` output — the output text itself is not stored).

Afterwards `dsh --profile codewhale` alone carries the identity (verified with
`dsh --profile codewhale --dump-config`), and `launch` prefers that profile
without `--patch`; `launch --profile web|headless` still uses the overlay.
Because the profile dependency is a `link:` to the Codewhale-owned directory,
`update` regenerates `cordis.patch.yml` in place — no pnpm run. Stale
detection covers the bundle: a modified or missing bundle patch, a bundle
that no longer matches the overlay, or a profile manifest that stopped
listing `codewhale-dsh-bundle` all report `stale-config`.

`remove-bundle` runs `dsh plugin --profile codewhale remove
codewhale-dsh-bundle` and deletes only the Codewhale-owned bundle files. The
profile directory itself (and the app bundle link dsh recorded there) is
DSH-owned and is left in place; the receipt says so. `remove` refuses while a
bundle is installed.

## Skin (unsupported overlay)

DSH 0.1.0-rc.6 has no supported custom-theme API — only the built-in
`ui-theme.preference` (`light|dark|system`). Its theme package documents
third-party themes as "an extension point, not a product": overriding
same-named `--dsw-alias-*` CSS variables, with no validation.

`--skin` therefore exports `codewhale-dsh-skin.css`, generated from the TUI's
real palette (`crates/tui/src/palette`, Blue Stage dark and light), including
the ombre water column, semantic role/permission/mode colors, focus,
selection, error, waiting, and working states, the crown-fluke mark as an
inline SVG, typography sizing, and `prefers-reduced-motion` fallbacks, and
maps a bounded set of `--dsw-alias-*` variables onto them. **It is never
injected**: applying it to a running DSH page (browser user stylesheet or a
future DSH plugin) is an unsupported overlay you enable yourself. The
attribution chip in the sheet reads "DeepSeek Harness connected through
Codewhale". `codewhale-dsh-skin-preview.html` renders the tokens for
inspection and is explicitly not the DSH UI.

## Removal

`remove` deletes only the overlay, skin, and preview under
`$CODEWHALE_HOME/integrations/dsh/`, appends a `remove` receipt, and never
touches `$DSH_HOME` or the installed package. DSH keeps working exactly as
before the connection.

## Attribution

DeepSeek Harness is © 2026 DeepSeek, MIT licensed; the integration invokes the
installed launcher and does not redistribute it. This is not native Codewhale
functionality: every surface labels it "DeepSeek Harness connected through
Codewhale".
