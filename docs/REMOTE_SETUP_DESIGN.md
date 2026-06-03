# `codewhale remote-setup` — Design & Implementation Plan

Status: **design** (do not implement against the 0.8.48 release wrap; land on a
branch or after 0.8.48 ships). Author handoff doc, mirrors the style of
`REFACTOR_HANDOFF.md`.

## Goal

One command — `codewhale remote-setup` — that guides a user through standing up
a remote CodeWhale agent they can talk to from a phone chat app, across:

- **Cloud target:** Tencent Lighthouse **or** Azure (extensible to GCP/Hetzner/bare).
- **Chat bridge:** Feishu/Lark **or** Telegram (extensible to Slack/Discord).
- **Model provider:** any entry in the existing `PROVIDERS` registry
  (DeepSeek, OpenAI, NVIDIA NIM, Atlascloud, WanjieArk, OpenRouter, Novita,
  Fireworks, Moonshot, SGLang, vLLM, Ollama, Xiaomi).

Decisions locked with the user:
- **Form:** native Rust subcommand in-binary (touches `crates/cli` + `crates/tui`).
- **Scope:** generate the deploy bundle **and** optionally auto-provision via the
  cloud CLI (`az` / CNB), behind a confirmation gate.

## Prior art: Hermes Agent (reference only — do not copy)

`/Volumes/VIXinSSD/hermesagent` (Nous Research's Hermes Agent, Python) solves the
same problem and **validates this design**. Use it for ideas; keep CodeWhale's
style (Rust core, zero-dep Node bridges, plain-text replies).

- Its `gateway/platform_registry.py` is exactly the table-driven approach here: a
  `PlatformEntry { name, label, adapter_factory, check_fn, validate_config,
  required_env, install_hint, setup_fn, source }`. That maps 1:1 onto our
  `BridgeSpec`/`CloudTarget` rows, and its per-platform `setup_fn` + `required_env`
  are what our wizard reads to prompt. A single gateway process fans out to many
  platforms — the model we want.
- Its `gateway/pairing.py` mirrors our allowlist/first-pairing flow.

### Telegram hardening checklist (mined from `gateway/platforms/telegram.py`)

That adapter is battle-tested; its method names enumerate edge cases our MVP
bridge should handle. Status against `integrations/telegram-bridge`:

| Edge case | In Hermes | In our bridge |
|---|---|---|
| 409 polling conflict (two `getUpdates`) | `_looks_like_polling_conflict` | **done** — poll loop backs off 10s + warns |
| 429 `retry_after` | rate-limit handling | **done** — `telegramApi` honors `parameters.retry_after` |
| Forum "General topic = 1" send/typing asymmetry | `_message_thread_id_for_send` vs `_for_typing` | **done** — omit `message_thread_id` when id is 1 on send |
| "message to be replied not found" after restart | `_send_with_dm_topic_reply_anchor_retry` | **sidestepped** — we never set `reply_to_message_id` |
| Network/connect-timeout retry | `_looks_like_network_error` | partial — generic 3s backoff in poll loop |
| Text batching + progress-edit (edit one msg vs spam) | `test_telegram_text_batching` | **deferred** — we send a chunk every 15s |
| MarkdownV2 escaping + table rendering | `_escape_mdv2`, `_wrap_markdown_tables` | **deferred** — plain text (safe; tables look plain) |
| Webhook mode as an alternative to long polling | `_webhook_mode` | out of scope — long-poll only (no inbound ports) |

Deferred items are deliberate: progress-edit and MarkdownV2 add real UX polish
but also complexity and (for MDV2) a whole class of parser-escaping bugs. Revisit
after `remote-setup` lands.

## Design principle: table-driven, like `ProviderSpec`

The provider registry (`crates/config/src/lib.rs::PROVIDERS`) is the model to
copy: "adding a provider is one row." Apply the same to clouds and bridges so
the matrix grows by data, not by new control flow.

```
        CloudTarget  ×  BridgeSpec   +  ProviderSpec (existing registry)
        ───────────     ──────────      ────────────────────────────────
        lighthouse      feishu          deepseek / openai / nvidia-nim / …
        azure           telegram        (wizard reads PROVIDERS, prompts for
        (future…)       (future…)        that provider's env_keys[0])
```

Clean separation that the architecture already implies:
- **Provider = a `runtime.env` concern.** The runtime resolves the provider from
  `CODEWHALE_PROVIDER` and the provider's own key var. The bridge never needs to
  know which provider is behind the runtime — it only forwards `model` to
  `/v1/threads`. So "multi-provider" only touches `runtime.env` generation.
- **Cloud = where it runs + where secrets live.**
- **Bridge = pure transport** between a chat app and `127.0.0.1:7878`.

## Command surface

New variant in `crates/cli/src/lib.rs` `Commands`:

```rust
/// Provision and configure a remote CodeWhale agent (cloud + chat bridge).
RemoteSetup(RemoteSetupArgs),
```

`RemoteSetupArgs` (clap):

| Flag | Meaning |
|---|---|
| `--cloud <azure\|lighthouse>` | Skip the cloud prompt. |
| `--bridge <telegram\|feishu>` | Skip the bridge prompt. |
| `--provider <slug>` | Provider slug; validated against `PROVIDERS`. |
| `--out <dir>` | Bundle output dir (default `./codewhale-deploy/<cloud>-<bridge>`). |
| `--generate-only` | Emit the bundle, do not provision (default). |
| `--apply` | Run the cloud CLI to actually provision (the auto-provision path). |
| `--yes` | Skip the final confirmation gate (CI/non-interactive). |
| `--non-interactive` | Fail instead of prompting if any required value is missing. |

CLI delegates to the TUI binary exactly like `Serve`/`Setup` do
(`delegate_to_tui(&cli, &resolved_runtime, tui_args("remote-setup", args))`).
The implementation lives next to `run_setup` in `crates/tui/src/`.

## Code layout

New module `crates/tui/src/remote_setup/`:

```
remote_setup/
  mod.rs          # run_remote_setup(): wizard orchestration + dispatch
  registry.rs     # CloudTarget + BridgeSpec tables (the matrix)
  prompt.rs       # thin stdin prompt helpers (reuse existing patterns)
  bundle.rs       # render env files / systemd units / RUNBOOK.md to --out
  provision/
    mod.rs        # Provisioner trait + confirmation gate + dry-run printer
    azure.rs      # az preflight, RG, VM+cloud-init, Key Vault, NSG, start
    lighthouse.rs # cnb.yml + tag_deploy.yml generation, CNB guidance
  templates/      # runtime.env, <bridge>.env, *.service, cloud-init.yaml.tmpl
```

### Registry types

```rust
pub struct BridgeSpec {
    pub slug: &'static str,            // "telegram"
    pub display: &'static str,         // "Telegram"
    pub package_dir: &'static str,     // "integrations/telegram-bridge"
    pub service_unit: &'static str,    // "codewhale-telegram-bridge.service"
    pub env_template: &'static str,    // templates/telegram.env
    /// Bridge-specific secret env keys to prompt for (token, etc.).
    pub secret_keys: &'static [&'static str], // ["TELEGRAM_BOT_TOKEN"]
    /// One-liner shown before prompting (e.g. "Create a bot with @BotFather").
    pub setup_hint: &'static str,
}

pub struct CloudTarget {
    pub slug: &'static str,            // "azure"
    pub display: &'static str,         // "Azure VM"
    pub secret_store: SecretStore,     // KeyVault | EnvFile
    pub install: InstallMethod,        // Docker | NativeSystemd
    /// Builds the ordered list of provisioning steps as (description, command).
    /// Commands are returned as data so they can be dry-run printed, gated,
    /// and only then executed.
    pub plan: fn(&DeployInputs) -> Vec<ProvisionStep>,
}
```

A `ProvisionStep { description, program, args, secret_args }` is *data*, never a
shell string — so the confirmation gate can print every command, secrets are fed
via stdin/temp files (never argv/`history`), and `--apply` just executes the
already-printed plan.

## Wizard flow

1. **Cloud** — pick from `CLOUD_TARGETS` (or `--cloud`).
2. **Bridge** — pick from `BRIDGES` (or `--bridge`); print `setup_hint`.
3. **Provider** — list `PROVIDERS` (canonical names), pick (or `--provider`).
   Look up `spec.env_keys[0]` as the key var to prompt for.
4. **Secrets** — prompt for: provider API key, bridge token(s) from
   `secret_keys`, allowlist (chat ids). Generate a random `CODEWHALE_RUNTIME_TOKEN`.
5. **Mode** — generate-only vs `--apply`.
6. **Render bundle** to `--out` (always, even with `--apply`).
7. **Confirm + provision** (only if `--apply`): print the full ordered command
   plan, require `y` (unless `--yes`), then execute step by step with progress.
8. **Print RUNBOOK.md** path and the remaining manual steps.

## Generated bundle

Written to `./codewhale-deploy/<cloud>-<bridge>/`:

- `runtime.env` — **provider config lives here**:
  ```
  CODEWHALE_PROVIDER=openai
  OPENAI_API_KEY=…              # the provider's own key var, from registry
  CODEWHALE_MODEL=auto
  CODEWHALE_RUNTIME_TOKEN=<random>
  CODEWHALE_RUNTIME_PORT=7878
  CODEWHALE_RUNTIME_WORKERS=2
  RUST_LOG=info
  ```
- `<bridge>.env` — transport only: `CODEWHALE_RUNTIME_URL=http://127.0.0.1:7878`,
  matching `CODEWHALE_RUNTIME_TOKEN`, allowlist, `TELEGRAM_BOT_TOKEN` (or Feishu
  app id/secret), `CODEWHALE_WORKSPACE`, `CODEWHALE_MODEL`.
- `codewhale-runtime.service`, `codewhale-<bridge>.service`.
- Cloud artifact: `cloud-init.yaml` + `provision.sh` (Azure) or `cnb.yml` +
  `tag_deploy.yml` (Lighthouse).
- `RUNBOOK.md` — the exact remaining commands + first-pairing steps.

## Auto-provision

### Azure (`--apply --cloud azure`)
Preflight: `az account show` (fail with "run `az login`" if absent). Then the
`plan()` emits, in order:
1. `az group create` (region prompted; default `eastus`).
2. `az keyvault create` + `az keyvault secret set` for the provider key and the
   runtime token (secrets via stdin, not argv).
3. `az vm create` with `--custom-data cloud-init.yaml` and a **system-assigned
   managed identity**; cloud-init pulls `ghcr.io/hmbown/codewhale:latest`, reads
   the secrets from Key Vault via the identity, writes `/etc/codewhale/*.env`,
   installs both systemd units, `enable --now`.
4. NSG: SSH (22) only, scoped to the caller's IP; **7878 stays on `127.0.0.1`**.
5. Print the SSH tunnel command for `/status` from a laptop if desired.

### Lighthouse (`--apply --cloud lighthouse`)
Reuse the existing `deploy/tencent-lighthouse/cnb/*.example` pipeline: render
`cnb.yml` + `tag_deploy.yml` from inputs and walk the user through the CNB
trigger (CNB does the VM-side work). Systemd units mirror the existing
`codewhale-runtime.service`.

Safety (matches the harness rules for outward-facing actions):
- Every command printed before execution; `y` gate unless `--yes`.
- Secrets never in argv or shell history.
- `--generate-only` is the default; `--apply` is explicit.

## Namespace migration: `DEEPSEEK_*` → `CODEWHALE_*`

Follow the convention already in `crates/config/src/lib.rs`: **read
`CODEWHALE_X` first, fall back to `DEEPSEEK_X`.** Nothing breaks for existing
deployments.

Touch list:
1. **Bridges** (`integrations/feishu-bridge`, `integrations/telegram-bridge`):
   in `lib.mjs`/`index.mjs`, read `process.env.CODEWHALE_X ?? process.env.DEEPSEEK_X`
   for `RUNTIME_URL`, `RUNTIME_TOKEN`, `WORKSPACE`, `MODEL`, `MODE`, `ALLOW_SHELL`,
   `TRUST_MODE`, `AUTO_APPROVE`, `CHAT_ALLOWLIST`, `ALLOW_UNLISTED`, `TURN_TIMEOUT_MS`.
   Validators accept either; templates emit `CODEWHALE_*`.
2. **Deploy units** (`deploy/tencent-lighthouse/systemd/*`,
   `integrations/*/deploy/*`): `DEEPSEEK_RUNTIME_*` → `CODEWHALE_RUNTIME_*`,
   env file paths `/etc/deepseek/` → `/etc/codewhale/` (keep reading the old path
   if present).
3. **`.env.example` files + `config.example.toml`**: lead with `CODEWHALE_*`,
   document `DEEPSEEK_*` as legacy aliases.
4. **Drop DeepSeek-shaped defaults** in the bridge: no hardcoded
   `DEEPSEEK_MODEL=auto`; the provider lives in `runtime.env` via
   `CODEWHALE_PROVIDER` + the registry's key var.

Note: items 1–3 touch **tracked** files, so they are part of the same
"don't ship during 0.8.48" hold. The brand-new (untracked) Telegram bridge can
be converted to `CODEWHALE_*` first as the reference implementation.

## Tests

- `registry.rs`: every `CloudTarget`/`BridgeSpec` slug is unique; each bridge's
  `package_dir`/`service_unit`/`env_template` exists.
- `bundle.rs`: rendering a bundle for each cloud×bridge×provider triple produces
  files with `CODEWHALE_*` keys, a matching runtime/bridge token, and a non-empty
  RUNBOOK.
- `provision`: `plan()` returns the expected ordered steps; **commands are built
  but never executed** in tests (assert on program+args, secrets redacted).

## Extensibility check

- Add **GCP**: one `CloudTarget` row + a `provision/gcp.rs` + a cloud-init reuse.
- Add **Slack**: one `BridgeSpec` row + `integrations/slack-bridge` + template.
No changes to the wizard control flow — it iterates the registries.

## Suggested sequencing (given the 0.8.48 freeze)

1. **Now (safe, untracked):** convert the new Telegram bridge to `CODEWHALE_* ??
   DEEPSEEK_*`; finalize this design.
2. **Post-0.8.48, branch:** namespace migration on tracked bridges + deploy units.
3. **Then:** implement `remote-setup` (registry → bundle → Azure provisioner →
   Lighthouse provisioner), generate-only first, `--apply` second.
