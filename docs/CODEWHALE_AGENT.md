# Codewhale Agent — one identity, many surfaces

"Codewhale Agent" is one product identity with many transport surfaces.
This page is the identity map: what each surface is, where each lives, and
the rules that keep them one product instead of several.

## The one identity

| Surface | Identity object | Where it lives | State |
|---|---|---|---|
| GitHub PR review | GitHub App bot `codewhale-agent[bot]` | GitHub org settings; repo variable `CODEWHALE_APP_ID` + secret `CODEWHALE_APP_PRIVATE_KEY` | `codewhale review --pr N --post` ships with the `codewhale-review.yml` workflow; it no-ops green until the App is configured (founder-gated), then posts as the App instead of the workflow token. Posting is always opt-in via `--post`. |
| Commit co-author | `.github/AUTHOR_MAP` | repo | existing; harvested co-author credit lands in the contributor graph |
| Chat channels (Telegram today; Slack/Feishu/Lark next; Discord/WeCom to come) | per-user bot registration bound to a Codewhale membership | CWC control-plane `services/control-plane/src` BotGateway + `integrations/chat/`; contracts in `packages/contracts` | Telegram pairing end-to-end (token → vault `credentialRef` → one-time code → `/start` binds chat↔membership). Default read-only command allowlist; opt-in write verbs; approve/deny keyboards route as permission decisions. |

## Rules that make it one identity

- Every surface is I/O around the single `Engine::run_turn`
  (`crates/tui/src/core/engine/turn_loop.rs`). No surface gets its own
  engine or turn loop.
- Every surface authenticates to the same Codewhale membership
  (`codewhale login` account session). Membership gates cloud agents;
  provider brands stay internal and invisible.
- Approvals from any surface route back as permission decisions into the
  same engine. A bare message never executes a gated action.
- Every command and approval is audited, on every channel.
- All surfaces are FREE. Rate limits are anti-abuse only. There is no
  per-channel fee, message metering, team gating, or trial timer — and a
  billing hook on a channel feature is forbidden by the money model.

## Naming

- Product: Codewhale. Agent surface: "Codewhale Agent".
- Bot handle wherever it can be chosen: `codewhale-agent`.
- The agent speaks as "Codewhale cloud agent" — never as a provider brand.

## Channel contract

Per-channel command allowlists (default read-only: `status`, `jobs`,
`receipts`, `help`; opt-in: `new task`, `approve`, `deny`), inline
approve/deny keyboards routed as permission decisions, `assistant_changes`
reply mode (message only when something changed), per-channel message
format adapters (Telegram markdown today; Slack blocks, Feishu/Lark cards,
WeCom markdown planned), quiet hours + digest, automation output routable
to any paired channel, one pairing code bound inside the authenticated
app, unbind revokes instantly.

WeChat note: personal-WeChat automation is **not supported** —
reverse-engineered protocols get users banned. Official 公众号/服务号 APIs
are the only acceptable path and are still under assessment.

## Pointer

Channel contracts live in `packages/contracts` (CWC repo); the gateway and
adapters live in `services/control-plane/src` (CWC repo). The GitHub
surface lives in `.github/workflows/codewhale-review.yml` here.
