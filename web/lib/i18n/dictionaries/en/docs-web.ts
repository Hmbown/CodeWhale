import type { DocsWebDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/web/page.tsx`.
 * Copy moved verbatim from the page's `isZh` ternaries — any wording change
 * belongs in its own commit, never mixed into a structural move.
 */
export const docsWeb: DocsWebDict = {
  metaTitle: "Browser Client · Codewhale Docs",
  metaDescription:
    "The loopback-only embedded browser client: one-time bootstrap, session cookie, and the local trust boundary.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Browser Client",
  overviewLead:
    "{webCommand} opens Codewhale's embedded browser client over the canonical Runtime API. It is a local surface: the server always binds to {loopbackHost}, cannot be rebound to a LAN address, and cannot run with Runtime authentication disabled. The default address is {defaultUrl}; on a port collision, pick another loopback port with {portExample}. Stop the process with Ctrl+C and the browser session ends with it.",
  overviewBody:
    "The current client provides a responsive thread and search rail, Runtime-owned session facts, transcript and tool receipts, and a composer. It can create, select, rename, and archive threads; start or steer turns; interrupt work; resolve approvals; and answer Runtime user-input requests. The browser is another view of the same local Runtime — it does not create a second cloud account, copy provider credentials into browser storage, or weaken the configured approval and sandbox policies.",
  authTitle: "Authentication boundary",
  authLead:
    "The browser-launch URL carries a random, short-lived, one-time bootstrap capability — never the Runtime bearer token. A loopback request exchanges it for an HttpOnly, SameSite=Strict, process-local session cookie and immediately invalidates the capability. Reused, expired, malformed, and non-loopback bootstrap attempts fail closed. The Runtime token is never placed in rendered HTML, browser storage, URL queries or fragments, or browser-launch arguments. Cookie-authenticated state-changing requests must also present the exact local web origin; cross-origin browser requests are rejected.",
  localTitle: "Local means local",
  localLead:
    "{webCommand} accepts {portFlag} and optional {tailscaleFlag}. There is no {hostFlag} and no insecure-auth option on this command. Do not treat it as a public website or expose its port through router forwarding, a public reverse proxy, or a generic tunnel. The separate {mobileCommand} and {httpFlag} modes carry different deployment and authentication contracts; read the Runtime API documentation before operating either one, especially before selecting a non-loopback bind.",
  tailscaleTitle: "Tailnet",
  tailscaleLead:
    "{tailscaleCommand} keeps the HTTP listener on loopback and publishes HTTPS:443 through Tailscale Serve. Peers on your tailnet open the MagicDNS bootstrap URL. This is Serve, not Funnel and not a public tunnel. Stopping the process turns off only that HTTPS:443 mapping.",
  troubleshootingTitle: "Troubleshooting",
  troubleshootingLead:
    "If port 7878 is occupied, pass an unused --port. If the browser cannot be opened, the command exits with an error rather than leaving a reusable bootstrap capability behind; check the OS default-browser setup and start again. If the page loads but a provider is unavailable, inspect codewhale doctor and /provider — the web command does not configure or move provider credentials. If a session expired, restart codewhale web to mint a new process-local session; reusing an old bootstrap URL is expected to fail.",
  sourceNote: "Source document: docs/WEB.md · Update docs-map.ts when changing.",
};
