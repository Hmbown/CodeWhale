import type { DocsComputersDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/computers/page.tsx`.
 * Every statement is taken from docs/DAYTONA_CLOUD_DISPATCH.md and
 * docs/CODEWHALE_AGENT.md; the commands, env names, and job states are
 * code-owned literals typeset by the page.
 */
export const docsComputers: DocsComputersDict = {
  metaTitle: "Cloud computers · Codewhale Docs",
  metaDescription:
    "How a local Codewhale session proposes, confirms, and tracks a cloud agent on a Daytona computer — explicit forges, fail-closed credentials, and what is not built yet.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Cloud computers",
  overviewLead:
    "A local session can offload a coding task to a cloud computer the way an editor sends a cloud agent: the remote job raises a branch and is meant to open a pull request against an explicit forge. Local stays responsive. Spend and push never happen silently — a proposal is written first, and nothing creates a sandbox or pushes a branch until you confirm it.",
  proposeTitle: "Propose, then confirm",
  proposeLead:
    "The first command writes a proposal and exits. The second confirms it by id. The TUI has the same two steps as slash commands, and the {cloudAgent} spelling is an alias of {dispatch}.",
  jobsLead:
    "Cloud jobs are first-class on the existing jobs surface as {kind}. List, show, and cancel them from the TUI or the CLI:",
  remotesTitle: "Explicit forges",
  remotesLead:
    "A remote is never assumed to be GitHub. A remote named after a forge is that forge; any other remote is classified by its URL host. If more than one forge is present, pass {remoteFlag}.",
  remotes: [
    ["github · cnb · gitee (by remote name)", "that forge, whatever the URL"],
    ["origin or other → github.com", "github"],
    ["origin or other → cnb.cool", "cnb"],
    ["origin or other → gitee.com", "gitee"],
  ],
  enableTitle: "Enable a Daytona computer",
  enableLead:
    "Credentials live in the process environment or in the Codewhale secret store — never in config.toml or models.toml, and never committed.",
  enableSteps: [
    ["Create an API key", "In the Daytona dashboard under API keys."],
    [
      "Export it for the session",
      "{apiKey}, optionally {apiUrl} for a non-default endpoint.",
    ],
    [
      "Or store it once",
      "In the Codewhale secret slot {slot} (OS keyring or the $CODEWHALE_HOME secrets file). The {alias} alias is also accepted.",
    ],
  ],
  cliNote:
    "An installed daytona CLI is not a credential. {status} and a bare {bare} report CLI presence separately from credential presence.",
  rulesTitle: "Fail-closed rules",
  rules: [
    ["No confirm", "A {proposed} job is written; the command exits success; Daytona is not called; nothing is pushed."],
    ["Confirm, no credentials", "A {refused} job is written; the command exits failure; no sandbox exists."],
    [
      "Confirm with credentials",
      "A Daytona sandbox is created, labelled with the job id and forge. This slice does not claim a GitHub, CNB, or Gitee PR URL, and a missing forge token fails closed the same way.",
    ],
  ],
  membershipTitle: "Who can dispatch",
  membershipLead:
    "Managed Agent surfaces authenticate to the same Codewhale membership — the {login} account session. Membership gates cloud agents, not local dispatch: `codewhale dispatch` with Daytona and forge credentials needs no account. Provider brands stay internal, and installing or running the local runtime needs no account at all.",
  leftoverTitle: "Not built yet",
  leftover: [
    ["Live watch", "A log tail of a running sandbox."],
    ["Cancel that tears down", "Cancelling a paid Daytona sandbox from the job surface."],
    ["Auto-decide", "Codewhale may propose a dispatch; it must not confirm its own proposal."],
    ["The remote runner", "The agent that actually raises the branch and opens the pull request."],
  ],
  sourceNote:
    "Source documents: docs/DAYTONA_CLOUD_DISPATCH.md, docs/CODEWHALE_AGENT.md · Update docs-map.ts when changing.",
};
