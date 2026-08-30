import type { DocsTrustDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/trust/page.tsx`.
 * Statements trace to docs/SANDBOX.md, docs/AUTHORIZATION_ORDER.md,
 * docs/TELEMETRY.md, and the `trust` block of docs/public-surface-facts.json.
 */
export const docsTrust: DocsTrustDict = {
  metaTitle: "Security & trust · Codewhale Docs",
  metaDescription:
    "What stays on your machine, what a hosted provider receives, how approvals and the OS sandbox differ, what telemetry sends and how to turn it off, and where to report a vulnerability.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Security & trust",
  overviewLead:
    "This page says what Codewhale does with your code, your commands, and your data — as implemented, not as intended. Each claim is drawn from a repository document that the facts gate checks on every build.",
  boundaryTitle: "Where your data goes",
  boundaries: [
    ["Local runtime", "The runtime, workspace state, and audit log stay on your machine."],
    ["Hosted provider", "The hosted provider you select receives the turn context required for inference. There is no mandatory Codewhale relay in between."],
    ["Local inference", "A loopback local-model route (vLLM, Ollama, SGLang) can keep inference on your machine entirely."],
    ["Account", "No account is required for the local runtime."],
    ["Plan mode", "Plan is read-only."],
  ],
  approvalTitle: "Approvals are not a sandbox",
  approvalLead:
    "Approval posture — Ask, Auto-Review, Full Access — decides whether a proposed command is shown to you before it runs. An approval from one layer is never a universal bypass: a later layer can still require review or block the call, and an approval is not an operating-system sandbox grant. The full model tool-call pipeline is nine ordered layers, from effective configuration through hooks, typed permission rules, repository law, and human approval to the execution sandbox.",
  sandboxTitle: "The OS sandbox, per platform",
  sandboxLead:
    "Only behaviour wired into the command execution path is described here, and Codewhale reports the mechanism it actually selected.",
  sandboxes: [
    ["macOS — Seatbelt", "Automatic when the runtime probe of sandbox-exec succeeds. Reported as {seatbelt}."],
    ["Linux — bubblewrap", "Opt-in: {preferBwrap} and an executable /usr/bin/bwrap. Reported as {bwrap}."],
    ["Linux without bwrap", "No OS wrapper by default. Reported as {none}."],
    ["Windows", "No OS wrapper in the current implementation. Reported as {none}."],
    ["External service", "{opensandbox} routes execution to an OpenSandbox-compatible service."],
  ],
  sandboxNote:
    "The repository also contains a seccomp module and a future Windows helper contract. Neither is wired into child-command launch, so Codewhale does not advertise them: source-only sandbox code is not evidence that a command was restricted.",
  telemetryTitle: "Telemetry, exactly",
  telemetryLead:
    "Anonymous usage counting is on by default, announced once by a non-blocking first-run notice, and can be turned off immediately and durably.",
  telemetry: [
    ["Never collected", "Conversations, code, prompts, files, file/repo/branch names, model content, credentials, or any per-turn or per-tool timeline."],
    ["Sent", "Version and platform classes, session duration and outcome, feature and error counters, closed enums, and a random install id that rotates every 90 days."],
    ["Endpoint", "{endpoint} — a first-party Cloudflare Worker whose source is in the repository under telemetry-ingest/."],
    ["Storage", "No IP, country, or geo column — structurally, not as a setting. Nothing is logged. Retention is a fixed three months."],
    ["Audit it yourself", "Set {dryRun}: batches are appended to {dryRunFile} on your machine, byte for byte what the server would have received, and no HTTP client is constructed."],
    ["Turn it off", "{configOff} or {envOff}."],
  ],
  auditTitle: "Local audit log",
  auditLead:
    "Sensitive events — credential, approval, and elevation events — append best-effort to {auditLog}. Write failures are logged rather than hidden. Provider token and cache usage is shown locally when available.",
  reportTitle: "Report a vulnerability",
  reportLead:
    "Send security reports by email to the maintainer rather than filing a public issue. Include the version from the masthead and a reproduction if you have one.",
  reportCta: "Email the maintainer",
  sourceNote:
    "Source documents: docs/SANDBOX.md, docs/AUTHORIZATION_ORDER.md, docs/TELEMETRY.md, docs/public-surface-facts.json · Update docs-map.ts when changing.",
};
