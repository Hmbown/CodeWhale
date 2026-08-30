import type { DocsAuthDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/docs/auth/page.tsx`.
 * Statements trace to docs/CONFIGURATION.md ("codewhale login"), docs/
 * CODEWHALE_AGENT.md, and docs/PROVIDERS.md. Commands are code-owned
 * literals typeset by the page.
 */
export const docsAuth: DocsAuthDict = {
  metaTitle: "Account & keys · Codewhale Docs",
  metaDescription:
    "Provider keys versus the optional Codewhale account: how each is set, where each is stored, and what needs no account at all.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Account & keys",
  overviewLead:
    "Codewhale holds two unrelated kinds of credential, and it helps to keep them apart. A provider key lets a model answer. A Codewhale account is optional and gates the managed surfaces — cloud agents and chat channels. Installing and running the local runtime needs neither an account nor a key.",
  credentials: [
    ["Provider key (BYOK)", "Your own key for DeepSeek, OpenAI, Anthropic, OpenRouter, a local runtime, or any other route. Set with {authSet}. Required for model replies."],
    ["Codewhale account", "A browser device-flow session started by {login}. Optional. Gates cloud agents and the Codewhale Agent surfaces; never required locally."],
  ],
  providerTitle: "Provider keys",
  providerLead:
    "{authSet} saves the key for one provider. A process-level {apiKeyFlag} still wins for a single run. {login} is not a provider-key command: provider credentials are configured exclusively through {authSet}.",
  accountTitle: "The account session",
  accountLead:
    "{login} is the same browser device flow as {accountLogin}. The session is scoped to the selected {profile}, and the older {cloud} spelling remains an alias.",
  accountCommands: [
    ["codewhale login", "Sign in through the browser device flow."],
    ["codewhale account status", "Show the session for the selected profile."],
    ["codewhale account logout", "Remove that session."],
  ],
  storageTitle: "Where sessions live",
  storageLead:
    "Account sessions prefer the operating system's credential manager and fall back automatically to the private, 0600 Codewhale secrets file when no credential manager is available — headless hosts, SSH, containers. The former {fileStoreEnv} opt-in is deprecated and ignored.",
  vaultTitle: "The account's own key vault",
  vaultLead:
    "{keys} manages the signed-in account's bring-your-own-key vault without ever displaying a secret value.",
  portableTitle: "Moving to another machine",
  portableLead:
    "{portable} writes a secret-free bundle: credential and machine-specific keys are dropped, never replaced with a redacted placeholder, so the file is safe to carry and nothing in it can sign in as you.",
  appTitle: "Sign in on the web",
  appLead:
    "The hosted Codewhale app carries the same account. Sign in or register there; the local runtime keeps working without it.",
  appSignIn: "Sign in →",
  appRegister: "Register →",
  sourceNote:
    "Source documents: docs/CONFIGURATION.md, docs/CODEWHALE_AGENT.md, docs/PROVIDERS.md · Update docs-map.ts when changing.",
};
