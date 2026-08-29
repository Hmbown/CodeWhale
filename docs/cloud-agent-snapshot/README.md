# Codewhale cloud-agent snapshot

The sandbox a `/dispatch` run launches from is a Daytona **snapshot**
built from this directory's `Dockerfile`, with the `codewhale` CLI
preinstalled. Daytona's create-sandbox API launches from snapshots — a
raw image is only ever a snapshot-build input — so "snapshot" is the
correct and only restorable artifact class here.

Why this exists (founder decision, 2026-08-29): the created sandbox must
run `codewhale exec --auto` itself, authenticated as the dispatching
account, resolving the account's configured model — dispatch → sandbox
(own codewhale) → account identity → forge PR. The account identity is
injected at create time as the `CODEWHALE_API_KEY` env var (a
`cwc_key_…` machine token); no provider API key ever widens into the
sandbox (BYOK stays local).

## Build the snapshot (operator, once per codewhale rev)

```sh
daytona snapshot create codewhale-cloud-agent \
  --dockerfile ./Dockerfile \
  --cpu 2 --memory 4
```

- The default name `codewhale-cloud-agent` matches
  `DEFAULT_CLOUD_AGENT_SNAPSHOT` in `crates/tui/src/cloud_dispatch.rs`;
  a different name is set per host with `CODEWHALE_DISPATCH_SNAPSHOT`
  (slug charset, ≤64 chars — an invalid override falls back to the
  default).
- Pin a rev with `--build-arg CODEWHALE_GIT_REV=<sha>` when the default
  moves past the rev you verified.
- Daytona snapshots deactivate after two weeks of disuse; reactivate
  before reuse or create fails.
- Resource sizing: the harness declares a one-hour turn budget; 2 vCPU /
  4 GB is the floor the offline tests assume.

## What the create call sends

`LiveDaytonaLauncher::create_sandbox` POSTs to the provider's sandbox
endpoint with:

```json
{
  "name": "cw-<job-id>",
  "snapshot": "codewhale-cloud-agent",
  "env": { "CODEWHALE_API_KEY": "<account machine token>" },
  "labels": { "codewhale.job": "<id>", "codewhale.forge": "<forge>", "codewhale.product": "dispatch" }
}
```

Labels are applied via the provider's dedicated labels endpoint right
after create (Daytona does not apply create-body labels); a labels
failure tears the fresh sandbox down and fails the create truthfully —
an unlabeled sandbox is spend the orphan reconciler can never find.

## Machine tokens

Minted from the web app (Account → API keys) or the CLI's cloud
api-key surface; `cwc_key_`-prefixed, read-mostly scopes, revocable
server-side. A confirmed dispatch without one refuses before any spend:
the sandbox's codewhale would have no identity to run as.
