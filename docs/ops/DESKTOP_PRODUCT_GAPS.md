# Desktop product gaps

Last reconciled: 2026-07-31 against Codewhale `main` at `3c7a614d7`.

This is the Codewhale runtime-side operations ledger for desktop and mobile
product convergence. It records only gaps that belong in the open-source
runtime or one of its public client contracts. A feature that already exists in
the runtime but is absent from the managed desktop belongs in the desktop
product backlog, not here.

> **Cross-repo scope note for auditors.** Desktop, hosted-web/PWA, mobile,
> and control-plane capabilities live in the private managed-product tree
> (`cwc`), not this repository. Before declaring any such capability absent
> from Codewhale, check both layers, with `cwc/docs/CURRENT.md` as ground
> truth (its `AGENTS.md` precedence rule: CURRENT wins; `cwc/docs/ARCHITECTURE.md`
> is M0-era and stale). Also re-check this ledger's "Last reconciled" commit
> against current `main` before citing it: rows here can lag the runtime by
> hours — e.g., Fleet HTTP creation/replay below was closed by `f7d95ea66`
> the day after the 2026-07-31 reconcile.

## Status vocabulary

- **BASE GAP** — Codewhale does not expose the capability a native client needs.
- **DESKTOP PROJECTION** — Codewhale has the capability; the desktop has not
  made it available or easy to use.
- **FIXTURE ONLY** — a contract or client helper exists without a live runtime
  acceptance receipt.
- **EXTERNAL GATE** — code cannot prove the required account, provider, signing,
  packaging, or device state.
- **PASS** — directly exercised on the named surface and recorded with evidence.

## Reconciled capability map

| Area | Base Codewhale truth | Classification | Desktop acceptance test |
| --- | --- | --- | --- |
| Single-agent repository Work | The Runtime HTTP API exposes authenticated threads, turns, event replay, interrupt, approval decisions, files, Git state, diffs, and terminal operations in `crates/tui/src/runtime_api.rs`. | DESKTOP PROJECTION until packaged dogfood passes | Open this repository in the packaged desktop, run one real provider turn that changes exactly one file, inspect its tool activity and diff, approve only the requested write, run a focused test, and leave the change uncommitted. Restart the app and prove thread/run recovery without losing the diff. |
| Fleet definitions and durable execution | Fleet protocol, ledger, profiles, roles, manager, worker runtime, CLI, and TUI are implemented in `crates/protocol/src/fleet.rs`, `crates/tui/src/fleet/`, and `crates/tui/src/main.rs`. Runtime HTTP supports list/get/interrupt/restart/stop in `crates/tui/src/runtime_api.rs`. | BASE GAP | Add an authenticated, approval-aware `POST /v1/fleet/runs` that accepts the canonical typed Fleet task specification, plus a bounded replayable event stream. A native client must create a planner + implementer + reviewer/verifier Fleet, observe exact provider/model/reasoning/permission receipts, interrupt one worker, resume it, and finish with a durable run receipt. |
| Fleet SDK | `npm/runtime-sdk` has typed Fleet inspection/control helpers. `createFleetRun()` and `fleetEvents()` are deliberately typed ahead of the Rust routes and currently raise a stable capability error. | FIXTURE ONLY / BASE GAP | Run the SDK against a real local Codewhale process, not a fetch fixture. Creation and ordered event replay must pass; reconnect with `since_seq` must neither drop nor duplicate events. |
| App-server control contract | The stdio app-server exposes thread, goal, config, model, prompt, interrupt, and user-input methods in `crates/app-server/src/lib.rs`; it exposes no Fleet methods. The desktop currently launches the fuller Runtime HTTP API through `codewhale app-server --http`, so native Fleet should extend the canonical Runtime HTTP surface first. | BASE GAP only if a stdio/native client needs parity after the HTTP work | Add Fleet methods to stdio only with a capability drift test and `docs/RUNTIME_API.md` update. Do not create a second Fleet implementation in app-server. |
| GitHub issue and pull-request context | The `github` tool in `crates/tui/src/tools/github.rs` reads issue/PR context and gates comments and closes with evidence and approval. `codewhale pr <N>` exists. `crates/protocol/src/workroom.rs` models GitHub issue and pull-request thread references. | DESKTOP PROJECTION | From a packaged desktop opened on a GitHub repository, choose an issue or PR without pasting its URL, load bounded untrusted context, ask Codewhale to analyze it, show checks/diff/review state, and draft a comment. Posting or closing must show the exact destination and require approval. |
| GitHub-native workflow | Base GitHub mutations intentionally exclude push and merge. There is no typed native-client API for issue/PR search, selection, draft-PR creation, check monitoring, review submission, or branch publication. | BASE GAP | Expose capability-scoped typed operations with idempotency keys and audit receipts. Read operations may run without approval; comments, issue/PR close, branch push, review submission, draft-PR creation, ready-for-review, and merge each retain their own policy decision. Force-push is not included. |
| GitHub safety | Issue bodies, PR descriptions, comments, and repository files are untrusted model input. The current GitHub tool keeps mutations evidence-backed and approval-gated and never pushes or merges. | PASS in TUI/CLI; DESKTOP PROJECTION elsewhere | Preserve the same boundary in every native API. A prompt-injection fixture inside an issue body must not expand scopes, auto-approve a mutation, reveal credentials, or change the destination repository. |
| Local/cloud Fleet placement | Fleet host and trust types exist, including local and remote host specifications. Managed Daytona/Sail scheduling, subscription entitlements, and provider metering belong to the managed Codewhale product rather than this repository. | DESKTOP PROJECTION / EXTERNAL GATE | The desktop chooses Local computer or Codewhale cloud explicitly. The resulting Fleet receipt records runtime placement per worker. No hosted allocation occurs from Chat or from merely opening Fleet setup. |
| Mobile control | The Runtime API can bind beyond loopback with an explicit token, but it is a local convenience boundary without TLS or user isolation. Secure account-paired remote control belongs to the managed product/native host. | EXTERNAL GATE | On physical iOS and Android devices, pair through an account-scoped relay, view a running desktop/cloud session, answer an approval with replay protection, revoke the device, and prove the old device can no longer poll or approve. |

## Runtime work queue

### CW-DESKTOP-001 — create and stream a Fleet through Runtime HTTP

Owner: Codewhale runtime.

Required behavior:

1. Add `POST /v1/fleet/runs` using the canonical `FleetRun`/task/worker types.
2. Add a bounded replayable `GET /v1/fleet/runs/{run_id}/events` stream with a
   monotonic cursor and the existing runtime authentication guard.
3. Keep the Fleet ledger authoritative; the HTTP layer must call the same
   manager/executor paths as `codewhale fleet`, not fork the scheduler.
4. Carry exact provider, model, reasoning, permission, trust, host, budget, and
   expected-artifact data into the durable receipt.
5. Make retries idempotent and make cancel/interrupt/restart outcomes explicit.
6. Update the Runtime SDK, `docs/RUNTIME_API.md`, and hermetic route tests in the
   same change.

Release evidence: UNRUN.

### CW-DESKTOP-002 — typed GitHub work API for native clients

Owner: Codewhale runtime.

Required behavior:

1. Reuse the existing `github` tool's repository resolution, redaction,
   evidence, and approval policy.
2. Add bounded typed reads for issue/PR search, context, checks, reviews, and
   changed files.
3. Add separately gated mutations for draft comment, post comment, close,
   branch push, draft PR, ready-for-review, review submission, and merge.
4. Require destination repository + issue/PR + branch identity in the approval
   receipt. Every mutating request accepts an idempotency key.
5. Never let issue/PR content alter scopes or approval policy. Never expose GitHub
   credentials or raw command construction to a native client.

Release evidence: UNRUN.

### CW-DESKTOP-003 — packaged self-edit contract

Owner: Codewhale runtime + managed desktop.

The runtime side is complete only when the released Codewhale pair can be
discovered, started under an opaque local-folder grant, driven through the
authenticated Runtime HTTP API, and stopped without orphaning a process. The
managed desktop must separately prove the user experience.

Release evidence: UNRUN against the Codewhale repository with a real model.

## Managed-product follow-ups (not base runtime defects)

- Add a calm Fleet composer and live worker/reviewer/verifier view to the shared
  web/desktop UI once CW-DESKTOP-001 exists.
- Add repository-native issue/PR pickers and approval sheets once
  CW-DESKTOP-002 exists. The current prompt shortcut for “create issue” is not
  a GitHub integration.
- Preserve local folder Work without requiring Daytona, Sail, Vercel, or any
  hosted product attachment.
- Treat desktop signing/notarization, Windows 11 packaging, physical mobile
  devices, production account model credentials, and hosted sandbox billing as
  separate evidence gates.

## Current blockers, stated narrowly

- Packaged Codewhale-on-Codewhale self-edit is unrun.
- Fleet creation and Fleet event streaming are missing from Runtime HTTP.
- First-class GitHub issue/PR navigation and guarded workflow mutations are not
  exposed as a native-client contract.
- A production hosted model failure is not presently evidence of a base
  Codewhale defect: the observed Kimi account connection requires replacement,
  while the real DeepSeek round trip has only been proven locally.
