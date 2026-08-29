# Daytona cloud-agent dispatch

Local `cw` / Codewhale can offload a coding agent to Daytona the way Cursor
sends a cloud agent: the remote job raises a branch and opens a PR against an
explicit forge. Local stays responsive; spend and push never happen silently.

The sandboxes are **Codewhale-operated infrastructure**, not a user-facing
product or provider: nothing in the CLI, TUI, job cards, or PR bodies carries
a provider brand, and there is no provider signup or key setup a user needs
to perform. Access ships with Codewhale membership (`codewhale login`) and
fails closed without it.

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
`/dispatch confirm` is required. A proposal is written first; nothing creates
a sandbox or pushes a branch until that confirmation.

Cloud jobs are first-class on the existing jobs surface (`kind=cloud`):

```
/jobs list
/dispatch list
/dispatch show <id>
/dispatch cancel <id>
codewhale dispatch --list
```

`/jobs list` shows shell jobs and, when cloud jobs exist, appends the cloud
section; `codewhale dispatch --list` (and `/dispatch list`) shows the cloud
jobs alone. `cloud_*` ids route to cloud show/cancel from both surfaces.

## What a confirmed job actually does

The runner (`crates/tui/src/dispatch_runner.rs`) drives one lifecycle:

```
proposed → launching → running → openingpr → done
                          │            │
                          └── failed ───┘   (+ canceled from any active state)
```

1. **launching → running** — create the sandbox (labeled with the job id and
   forge) and wait until it accepts work. The job record keeps the sandbox
   id.
2. **running** — clone the target forge repository inside the sandbox and run
   **one** cloud agent turn through the same one-shot harness entry every
   local non-interactive caller uses (`codewhale exec --auto "<prompt>"`).
   There is no second engine: the sandbox runs the one `Engine::run_turn`
   path, remotely.
3. **openingpr** — collect the agent's work product (`format-patch` against
   the clone's default branch), apply it locally on a fresh shallow clone,
   and push the branch with a **plain** push (`--force` is never passed, so a
   moved branch fails closed instead of rewriting history).
4. **done** — open the PR on the target forge and record the URL:
   - `github` — the `gh` CLI (`gh pr create`), reusing the repo's existing
     gh seam and auth;
   - `gitee` — Gitee API v5 `POST /repos/{owner}/{repo}/pulls` with a token
     from the Codewhale service slot `gitee`;
   - `cnb` — CNB OpenAPI `POST /{repo}/-/pulls` with a token from the
     service slot `cnb`.
   The PR body is truthful: what the agent did, the receipts Codewhale has
   (job id, sandbox id, branch, base, head sha), and an explicit
   `No-Issue: cloud dispatch cloud_<id>` line.
5. **teardown** — the sandbox is deleted on completion, failure, *and*
   cancellation; the job note says whether teardown succeeded.

Every phase persists its transition, so `codewhale dispatch --show <id>` /
`/dispatch show <id>` stream real progress while the run is in flight.

### Where the run happens

- The sandbox launches from the Codewhale cloud-agent snapshot — the
  `codewhale` CLI is preinstalled in it (see
  [`cloud-agent-snapshot/`](./cloud-agent-snapshot/)) — with the
  dispatching account's machine token injected as `CODEWHALE_API_KEY`, so
  the in-sandbox `codewhale exec --auto` runs as the account and resolves
  the account's configured model. No provider API key widens into the
  sandbox.
- The CLI stays attached: after `--confirm` it prints the launching card and
  waits for the runner so a sandbox is never orphaned by an early exit
  (Ctrl-C exits the wait; the job record survives, and `--cancel` tears the
  sandbox down).
- The TUI detaches the runner so the session stays responsive; the job
  record is the source of truth and `/dispatch cancel` works at any time.

## Remotes

Forges are explicit: `github`, `cnb`, `gitee`.

CWC already treats a remote *named* `github` as authoritative GitHub and
`origin` as the CNB mirror when that URL is `cnb.cool`. Codewhale uses the
same rule:

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

## Access (fail-closed, membership-first)

Cloud agents ship with the Codewhale account. The gate is sign-in:
`codewhale login`. Until then dispatch proposes but refuses to confirm, and
`codewhale dispatch --status` says exactly that. **There are no provider
setup steps for users** — no provider signup, no dashboard, no user-held
provider key.

Internally (Codewhale operators only), the sandbox credential is discovered
from the service-side slot exactly as the first landing defined it
(`DAYTONA_API_KEY` / CWC alias / the `daytona` secret slot, plus the
`DAYTONA_API_URL` origin override). It is never printed, never logged, never
written into a job record, and is not a user surface: there is no
`auth set-slot` command for it and no locale string mentions it.

Forge credentials follow the same rule: GitHub auth comes from the existing
`gh` CLI login; Gitee and CNB tokens live in the Codewhale service slots
`gitee` and `cnb` and are read only at PR-open time.

The dispatching host also needs the account machine token
(`CODEWHALE_API_KEY`, a `cwc_key_…` key from Account → API keys): it is
injected into the sandbox so the in-sandbox `codewhale` runs as the
account. Without it confirm refuses before any spend — a sandbox whose
agent has no identity is money for nothing.

## Confirmation and fail-closed rules

- No `--confirm` / `/dispatch confirm`: write a `proposed` job, exit success,
  do not create a sandbox, do not push.
- Confirm without membership/credentials: write a `refused` job, exit
  failure, no sandbox.
- Confirm + credentials: the lifecycle above. Any phase that cannot honestly
  complete records a `failed` job with a sanitized, truthful note — a PR URL
  is never invented, and a missing forge token fails closed *after* the
  branch push with an explicit "no pull request was opened" message.
- `codewhale dispatch` may propose; it never confirms itself.

## Cancellation and cost transparency

- `--cancel <id>` / `/dispatch cancel <id>` / `/jobs cancel cloud_<id>` flip
  the record to `canceled` and tear a live sandbox down immediately; a runner
  in flight stops at its next checkpoint and tears down too.
- The status card and `--show` surface real receipts: sandbox id, PR URL when
  opened, head sha, and a runtime figure in whole minutes. Runtime is
  Codewhale's own bookkeeping (created → finished); it is not a provider
  bill, and the card says so.

## Network safety

Every credential-bearing outbound call (sandbox control plane, sandbox
toolbox, Gitee, CNB) goes through one origin guard: https only, no userinfo,
and no loopback / private / link-local / reserved / multicast / `.local` /
`.internal` targets. Explicit loopback origins are allowed only in debug
builds for local smoke testing. DNS-rebinding (a public name resolving to a
private address) is out of scope. Branch pushes are plain `git push` through
the machine's existing forge credentials; `--force` is never used.

## Live status vs recording tests

The full lifecycle (create → wait ready → clone → harness turn → collect →
push → PR → teardown), cancellation teardown, PR shapes, host validation,
and the no-force push rule are pinned by offline tests against recording
launchers and local git fixtures. The live network paths (Daytona sandbox
create/execute/delete against a real account, `gh pr create`, and the
Gitee/CNB REST calls) follow the providers' published OpenAPI shapes and
still need one real-sandbox smoke test per forge before the receipts they
produce can be called verified — the PR body and job notes never claim more
than the receipts shown.

## Leftover

- Live watch / log tail of a running sandbox.
- Auto-decide heuristics (Codewhale may propose; it must not confirm itself).
- Private-repo clones in the sandbox (needs a credential pass-through design
  that does not widen secrets into the agent process).
