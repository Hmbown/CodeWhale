import type { DocsEnterpriseDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/enterprise/page.tsx`.
 */
export const docsEnterprise: DocsEnterpriseDict = {
  metaTitle: "Enterprise review · Codewhale Docs",
  metaDescription:
    "Operator and security-review packet: local runtime, BYOK, first-party telemetry, crash dumps, sandbox, and air-gap controls.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Enterprise review",
  overviewLead:
    "Codewhale is an open-source coding agent that runs on the machine you give it. You bring the model. Approval policy, sandboxing, and telemetry are local controls. The managed account at app.codewhale.net is optional. This page describes behavior already in the runtime — it is not a compliance certificate.",
  facts: [
    [
      "Runs where you install it",
      "Hosted, gateway, and local models use the same local runtime, tools, and permission stack.",
    ],
    [
      "Credentials stay local unless you opt in",
      "Provider keys are configured with codewhale auth set. The optional account login is a separate browser device flow.",
    ],
    [
      "No third-party analytics SDK",
      "There is no PostHog, Sentry, or session-replay vendor in the runtime. Anonymous usage counting is first-party and disableable.",
    ],
    [
      "No invented certification",
      "This page does not claim SOC 2, ISO, SSO, or a hosted SLA. Review the source documents and the code.",
    ],
  ],
  telemetryTitle: "Telemetry and crash reporting",
  telemetryLead:
    "Enabled sessions send a closed, aggregate schema to telemetry.codewhale.net. Conversations, code, prompts, files, model ids, and credentials are never collected. Panic events carry only an allowlisted crates/… site — never the panic message. Crash dumps stay on disk at $CODEWHALE_HOME/crashes.",
  controlsTitle: "Kill switches",
  controls: [
    [
      "codewhale config set telemetry false",
      "Persistent opt-out. Stops collection and erases local telemetry state.",
    ],
    [
      "CODEWHALE_TELEMETRY=0",
      "Run-scoped kill switch. Collects nothing; does not erase disk state.",
    ],
    [
      "telemetry_endpoint = \"\"",
      "Stays enabled but contacts nobody. Batches append to a local dry-run file.",
    ],
    [
      "Repository config cannot aim it",
      "A checkout cannot turn telemetry on or point it at another host.",
    ],
  ],
  credentialsTitle: "Credentials and BYOK",
  credentialsLead:
    "Hosted providers use your keys. Local vLLM, SGLang, and Ollama usually need none. codewhale account login is optional device-flow sign-in; account keys list|set|remove manages that vault without printing secrets. Portable config export drops credentials instead of redacting them in place.",
  policyTitle: "Authorization and sandbox",
  policyLead:
    "Tool calls pass configuration, mode, hooks, typed permissions, auto-review, repository law, human approval, and then the OS sandbox — in that order. A later layer can still hold or block. macOS uses Seatbelt when the probe succeeds; Linux bubblewrap is opt-in; Windows currently reports no OS command sandbox. Project overlays may tighten policy, not loosen it.",
  airgapTitle: "Air-gapped and managed desktops",
  airgapLead:
    "Persist telemetry = false and [update] check_for_updates = false for air-gapped, corporate-proxy, or image-managed desktops. CODEWHALE_HOME isolates crash dumps, telemetry files, and the rest of product state onto a path you choose.",
  sessionsTitle: "Concurrent sessions",
  sessionsLead:
    "The runtime thread store is single-owner. A second Codewhale on the default store ($CODEWHALE_HOME/tasks/runtime) fails at startup. Until 0.9.12 scopes the store per session (#5630), set CODEWHALE_RUNTIME_DIR to a per-session path. Do not share one store across processes, and do not drop the owner lock — the store is not multi-writer safe. codewhale web --tailscale publishes the loopback UI onto your Tailscale tailnet without opening a LAN bind or Funnel.",
  claimsTitle: "What this page does not claim",
  claimsLead:
    "No SOC 2, ISO 27001, FedRAMP, or HIPAA certification. No enterprise SSO / SAML / SCIM surface in this open-source runtime. No requirement to use the managed app. Vulnerability reports go through SECURITY.md, not a public issue.",
  sourceNote: "Source document: docs/ENTERPRISE.md · Update docs-map.ts when changing.",
};
