import type { DocsRuntimeApiDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/runtime-api/page.tsx`.
 * Copy moved verbatim from the page's `isZh` ternaries — any wording change
 * belongs in its own commit, never mixed into a structural move.
 */
export const docsRuntimeApi: DocsRuntimeApiDict = {
  metaTitle: "Runtime API · Codewhale Docs",
  metaDescription:
    "Local HTTP/SSE, JSON-RPC stdio, and ACP entrypoints for integrations, bridges, and automation.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Runtime API",
  overviewLead:
    "codewhale app-server is the canonical local runtime API and control plane. Local SDKs, mobile/remote-control clients, and editor integrations talk to it instead of screen-scraping terminal output. The engine runs as a local-only process: every API binds to localhost by default — no hosted relay, no provider-token custody, no secret leakage. codewhale serve --http / --mobile remain compatibility aliases for app-server --http / --mobile and launch the identical server; new integrations should target app-server.",
  entries: [
    ["http", "The full /v1/* HTTP/SSE runtime API (canonical entry), default 127.0.0.1:7878."],
    ["mobile", "The runtime API plus the /mobile phone control page."],
    [
      "stdio",
      "Newline-delimited JSON-RPC 2.0 control transport with no listener, for local SDKs and probes.",
    ],
    [
      "web",
      "The loopback-only browser client, embedded in the binary and opened in the default browser.",
    ],
    ["doctor", "Machine-readable health and capability report."],
    ["acp", "ACP (Agent Client Protocol) stdio adapter for editors such as Zed."],
    [
      "exec",
      "The one-shot headless worker (stream-json, fleet subprocess, CI primitive) — not part of this API, but it shares the same runtime and event vocabulary.",
    ],
  ],
  stdioTitle: "Probe without model tokens",
  stdioLead:
    "The stdio control transport can be probed without spending model tokens. capabilities returns the advertised method families (thread/*, app/*, prompt/*) and the full method list; the method set is pinned by a drift test in crates/app-server/src/lib.rs, so SDK and local integration clients can rely on it not changing silently.",
  interruptNote:
    "A live turn can be asked to stop with thread/interrupt (or POST /v1/threads/{id}/turns/{turn_id}/interrupt over HTTP); when no turn is streaming the reply carries interrupted: false — not an error, just nothing to stop.",
  securityTitle: "Security boundary",
  securityLead:
    "The runtime API token is read from {authToken}, then {runtimeTokenEnv}, then {legacyTokenEnv}; {insecureFlag} is only accepted with a loopback bind. Cross-origin browser requests are rejected by the CORS allow-list. Before selecting a non-loopback bind — especially {mobileFlag} — read the full deployment and authentication contract in docs/RUNTIME_API.md.",
  sourceNote: "Source document: docs/RUNTIME_API.md · Update docs-map.ts when changing.",
};
