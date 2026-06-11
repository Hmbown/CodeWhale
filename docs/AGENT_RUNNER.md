# Agent Runner Protocol

How a headless agent (DeepSeek V4 on a DigitalOcean droplet, or any codewhale exec caller) picks up, implements, verifies, and delivers a milestone issue — fully autonomously.

## Prerequisites

- `gh` CLI authenticated with a fine-grained PAT scoped to `Hmbown/CodeWhale` (Contents RW, Issues RW, PRs RW, Metadata R)
- `codewhale` binary on `$PATH` (v0.8.57+)
- `DEEPSEEK_API_KEY` (or equivalent provider key) exported in the agent user's shell
- A `git worktree` per issue (never commit directly to `main`)

---

## The loop

### 1. Pick

```bash
gh issue list \
  --repo Hmbown/CodeWhale \
  --milestone v0.8.58 \
  --label agent-ready \
  --state open \
  --json number,title,url
```

Choose an issue.  Prefer `release-blocker` → `bug` → `enhancement` order.
Do not pick an issue already labeled `agent-in-progress`.

### 2. Claim

```bash
gh issue edit <N> --add-label agent-in-progress --remove-label agent-ready
```

This prevents other agents from picking the same issue.

### 3. Isolate

```bash
cd /opt/whalebro/codewhale
git fetch origin
git worktree add ../worktrees/issue-<N> -b agent/<N>-<slug> origin/main
cd ../worktrees/issue-<N>
```

Every issue gets its own branch and worktree.  The branch name convention is `agent/<issue-number>-<short-slug>`.

### 4. Execute

```bash
gh issue view <N> --json body -q .body | \
  codewhale exec --auto --output-format stream-json "$(cat)"
```

The agent reads the issue body and implements the fix.  Use a tmux session per issue so the run survives SSH disconnects:

```bash
tmux new-session -d -s "issue-<N>" \
  "gh issue view <N> --json body -q .body | \
   codewhale exec --auto --output-format stream-json \"\$(cat)\" 2>&1 | tee /tmp/issue-<N>.log"
```

For resuming an interrupted run (`--continue` picks up the most recent
session for this workspace; `--resume latest` only exists in the interactive
TUI):

```bash
codewhale exec --auto --output-format stream-json --continue "..."
```

### 5. Verify

Run the exact commands from the issue's **Verification** section.  If they pass, proceed.  If they fail, loop back to step 4 with the error output as context, or label `needs-human`.

### 6. Deliver

```bash
gh pr create \
  --repo Hmbown/CodeWhale \
  --base main \
  --title "<descriptive title>" \
  --body "Closes #<N>" \
  --label v0.8.58
```

All delivery is via PR — never push to `main` directly.  Human review is required before merge.

### 7. On blockage

```bash
gh issue edit <N> --add-label needs-human --remove-label agent-in-progress
gh issue comment <N> --body "Blocked: <reason>.  Human decision needed."
```

Common blockers: missing credentials, ambiguous scope, test environment unavailable, network outage.

---

## Label semantics

| Label | Meaning | Auto-applied? |
|---|---|---|
| `agent-ready` | Body has all six template sections; a remote agent may claim it | Yes (template) |
| `agent-in-progress` | Claimed by an agent run; do not double-pick | Manual (step 2) |
| `needs-human` | Agent blocked; requires human decision or credentials | Manual (step 7) |
| `autonomous-ready` | Legacy nightly-loop label; distinct from `agent-ready` | No |

The `autonomous-ready` label is for the legacy nightly loop (external automation).
New work uses `agent-ready`.

---

## Safety rules

1. **PR-only delivery.**  Never commit to `main`.  Every change is a branch + PR.
2. **No force-push.**  `git push --force` is forbidden.
3. **Secrets never in argv, history, or logs.**  API keys, PATs, and credentials live in `/etc/codewhale/*.env` and are sourced into the agent user's shell.  The runtime API listens on `127.0.0.1:7878` only.  Telegram bridge chats are allowlisted.
4. **Human reviews every PR.**  The droplet loop delivers PRs; a human on the laptop reviews and merges.
5. **One issue per worktree.**  No cross-contamination between concurrent agent runs.

---

## Issue body format

Every `agent-ready` issue must have these six sections (enforced by `.github/ISSUE_TEMPLATE/agent-task.yml`):

1. **Goal / Why** — what problem, why now
2. **Scope / Plan** — numbered steps with file paths
3. **Key files** — paths to read first
4. **Acceptance criteria** — behavior-level checkboxes
5. **Verification** — exact shell commands
6. **Out of scope** — explicit non-goals

The body must be self-sufficient: a fresh clone agent with no conversation context must be able to execute it.
