# Enterprise review

> 阅读简体中文版：[zh_hans/ENTERPRISE.md](zh_hans/ENTERPRISE.md)

This is the operator and security-review packet for running Codewhale in a
company. It describes behavior that is already in the runtime. It is not a
compliance certificate, a managed-service SLA, or a claim that a hosted
control plane is required.

Codewhale is an open-source coding agent that runs on the machine you give it.
You bring the model. Approval policy, sandboxing, and telemetry are local
controls. The managed account at `app.codewhale.net` is optional.

## What a reviewer should take as given

- **The binary runs where you install it.** Hosted, gateway, and local models
  all go through the same local runtime, tools, and permission stack.
- **Credentials stay on the machine unless you opt into an account.** Provider
  keys are configured with `codewhale auth set --provider <provider>`. The
  optional Codewhale account (`codewhale account login`) is a separate browser
  device flow and can hold a redacted BYOK vault.
- **Nothing in this document invents a certification.** There is no SOC 2,
  ISO, or SSO claim here. Review the linked source documents and the code.

## Data that never leaves the machine

The product telemetry schema is closed and tested against this repository.
Codewhale does not collect conversations, code, prompts, files, file or repo
or branch names, model content, model ids, or credentials. It sends no
per-turn or per-tool timeline.

See [TELEMETRY.md](TELEMETRY.md) for the field-by-field contract. The public
red-line list there is the authority; this page does not weaken it.

## Telemetry and crash reporting

Codewhale does **not** embed PostHog, Sentry, or any other third-party
analytics or crash SDK. Anonymous usage counting and panic *sites* go to the
first-party ingest at `https://telemetry.codewhale.net/v1/telemetry` when
telemetry is enabled. The ingest source is in this repository at
[`telemetry-ingest/`](../telemetry-ingest/).

| Control | Effect |
|---|---|
| `codewhale config set telemetry false` | Persistent opt-out. Stops collection and erases local telemetry state. |
| `CODEWHALE_TELEMETRY=0` | Run-scoped kill switch. Collects nothing; does not erase disk state. |
| `codewhale --telemetry false` | The same kill switch for one command. |
| `telemetry_endpoint = ""` | Stays enabled but contacts nobody. Batches append to `$CODEWHALE_HOME/telemetry/dryrun.jsonl`. |

A repository `.codewhale/config.toml` cannot set `telemetry` or
`telemetry_endpoint`. A workspace `.env` cannot either. Someone else's
checkout cannot turn telemetry on or aim it at another host.

**IT / MDM floor.** Persist `telemetry = false` in the user config, or set
`CODEWHALE_TELEMETRY=0` in the managed environment. The file setting is a
floor: `--telemetry true` and `CODEWHALE_TELEMETRY=1` both lose to it.

**Crashes stay local.** Panic dumps and fatal-signal markers are written to
`$CODEWHALE_HOME/crashes` (otherwise `~/.codewhale/crashes`). If telemetry is
armed, the process may also send a `panic` event that carries only an
allowlisted `crates/…` source site — never the panic message. Fleet workers
never emit telemetry.

The first interactive launch shows one localized, non-blocking notice after
the terminal is ready. Telemetry stays unarmed until that notice has been
drawn. `/settings` remains the ordinary toggle.

## Credentials and account login

- **Provider BYOK.** `codewhale auth set --provider <provider>` writes the
  key to the user config. Hosted providers use your credentials; local
  vLLM, SGLang, and Ollama usually need none.
- **Optional account.** `codewhale account login` starts the Codewhale
  browser device flow against `app.codewhale.net`. `codewhale account keys
  list|set|remove` manages that account's BYOK vault without printing secret
  values.
- **Secret storage.** Account sessions prefer the OS credential manager and
  fall back to the private `0600` Codewhale secrets file on headless hosts,
  SSH, and containers.
- **Portable config.** `codewhale config export --portable` writes a
  secret-free bundle. Credential and machine-specific keys are dropped, not
  redacted in place.

## Authorization, modes, and sandbox

Tool calls are not a single allow/deny bit. The interactive engine evaluates
configuration, mode, hooks, typed `permissions.toml` rules, auto-review,
repository law, human approval, and then the execution sandbox — in that
order. A later layer can still hold or block a call. See
[AUTHORIZATION_ORDER.md](AUTHORIZATION_ORDER.md).

Operators set how much the agent may do without asking:

- **Modes.** Plan is read-only. Work and Operate change how the agent
  proceeds; they do not replace the permission stack.
- **Permission posture.** Ask, Auto-Review, and Full Access. Full Access is
  not a sandbox grant.
- **OS command sandbox.** macOS uses Seatbelt when the probe succeeds. Linux
  bubblewrap is opt-in (`prefer_bwrap = true`). Windows currently reports no
  OS command sandbox. Approval policy and workspace-aware file tools still
  apply. See [SANDBOX.md](SANDBOX.md).
- **Project overlays** may tighten `approval_policy`, `sandbox_mode`, or
  shell availability. They may not loosen them.

## Audit surfaces

- `~/.codewhale/audit.log` records resolved **key names**, never values.
- `/config audit` shows which documented keys the TUI can change live.
- Tool-audit events (`tool.repo_law_decision`, `tool.auto_review`) carry
  labels, not prompt text or secret values.
- Fleet ledger records store audit labels such as `slack` or `webhook`, not
  message bodies.

These are local logs. They are not a hosted SIEM integration.

## Air-gapped and managed desktops

```toml
telemetry = false

[update]
check_for_updates = false
```

```sh
CODEWHALE_TELEMETRY=0 codewhale
```

The startup update check never blocks a turn and fails silently when
offline. Disabling it is the right setting for air-gapped, corporate-proxy,
or image-managed desktops. Install from a verified channel in
[INSTALL.md](INSTALL.md) — npm, Cargo, Homebrew, Docker, or the checksummed
`install.sh` binaries.

`CODEWHALE_HOME` isolates all product state, including crash dumps and
telemetry files, onto a path you choose.

## Fleet and remote control

- Fleet workers do not emit product telemetry.
- Fleet `security_policy` and worker `trust_level` are the authority
  envelope for dispatched runs. See [FLEET.md](FLEET.md).
- `/rc` can lease the live session to `app.codewhale.net` after a one-time
  browser approval. The terminal remains a readable safety surface;
  interrupt still works. Disconnect keeps local input locked until the
  lease expires so two controllers cannot compete.

## What this packet does not claim

- No SOC 2, ISO 27001, FedRAMP, or HIPAA certification.
- No enterprise SSO / SAML / SCIM surface in this open-source runtime.
- No promise that the managed app is required, or that it replaces local
  policy.
- No third-party analytics, session replay, or crash-reporting vendor.

Vulnerability reports go through [SECURITY.md](../SECURITY.md), not a public
issue.

## Source documents

| Topic | Document |
|---|---|
| Telemetry schema and kill switches | [TELEMETRY.md](TELEMETRY.md) |
| Authorization order | [AUTHORIZATION_ORDER.md](AUTHORIZATION_ORDER.md) |
| Sandbox threat model | [SANDBOX.md](SANDBOX.md) |
| Configuration, update checks, account login | [CONFIGURATION.md](CONFIGURATION.md) |
| Install channels | [INSTALL.md](INSTALL.md) |
| Fleet policy | [FLEET.md](FLEET.md) |
| Security disclosure | [SECURITY.md](../SECURITY.md) |
