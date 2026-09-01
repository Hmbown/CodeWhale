# Daytona cloud-agent dispatch

Local `cw` / Codewhale can offload a coding agent to Daytona the way Cursor
sends a cloud agent: the remote job raises a branch and is intended to open a
PR against an explicit forge. Local stays responsive; spend and push never
happen silently.

## One obvious offload

```sh
codewhale dispatch "open a PR that fixes the flake" --remote github
codewhale dispatch --confirm cloud_<id>
```

Same action in the TUI:

```
/dispatch open a PR that fixes the flake --remote github
/dispatch confirm cloud_<id>
```

`codewhale cloud-agent` and `/cloud-agent` are aliases. `--confirm` /
`/dispatch confirm` is required. A proposal is written first; nothing creates a
Daytona sandbox or pushes a branch until that confirmation.

Cloud jobs are first-class on the existing jobs surface (`kind=cloud`):

```
/jobs list
/dispatch list
/dispatch show <id>
/dispatch cancel <id>
codewhale dispatch --list
```

## Remotes

Forges are explicit: `github`, `cnb`, `gitee`.

CWC already treats a remote *named* `github` as authoritative GitHub and
`origin` as the CNB mirror when that URL is `cnb.cool`. Codewhale uses the same
rule:

| Remote name | URL host | Forge |
| --- | --- | --- |
| `github` | any | `github` |
| `cnb` | any | `cnb` |
| `gitee` | any | `gitee` |
| `origin` or other | `github.com` | `github` |
| `origin` or other | `cnb.cool` | `cnb` |
| `origin` or other | `gitee.com` | `gitee` |

If more than one forge is present, pass `--remote` / `--remote` on `/dispatch`.
Do not assume `origin` is GitHub.

## Enable Daytona (fail-closed)

This slice does **not** fake a successful remote PR. Without credentials,
dispatch refuses and records a `refused` cloud job.

1. Create an API key at [app.daytona.io/dashboard/keys](https://app.daytona.io/dashboard/keys).
2. Export it in the process environment (never commit it, never put it in
   `models.toml` or `config.toml`):

   ```sh
   export DAYTONA_API_KEY=...
   # optional
   export DAYTONA_API_URL=https://app.daytona.io/api
   ```

3. Or store the same key in the Codewhale secret slot `daytona` (OS keyring /
   `$CODEWHALE_HOME` secrets file). The CWC alias `CWC_DAYTONA_TOKEN` is also
   accepted so an already-configured control-plane shell works.

`daytona` CLI being installed is **not** a credential. `codewhale dispatch
--status` / bare `/dispatch` report CLI presence separately.

## Confirmation and fail-closed rules

- No `--confirm` / `/dispatch confirm`: write a `proposed` job, exit success,
  do not call Daytona, do not push.
- Confirm + no credentials: write a `refused` job, exit failure, no sandbox.
- Confirm + credentials: create a Daytona sandbox labeled with the job id and
  forge. This slice does **not** claim a GitHub/CNB/Gitee PR URL. A missing
  forge token fails closed the same way — it does not invent a PR.

## Leftover

- Live watch / log tail of a running sandbox.
- Cancel that tears down a paid Daytona sandbox.
- Auto-decide heuristics (Codewhale may propose; it must not confirm itself).
- The remote agent runner that actually raises the branch and opens the PR.
