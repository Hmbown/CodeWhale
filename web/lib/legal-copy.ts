/** Binding legal text for Shannon Labs / Codewhale. Same body as app.codewhale.net/legal. */

export const LEGAL_UPDATED = "July 23, 2026";

export const TERMS_SECTIONS = [
  {
    title: "Your account",
    body: "You are responsible for your account, connected providers, repositories, runners, and credentials. Keep access methods secure and provide accurate information. Do not share authority you are not permitted to grant.",
  },
  {
    title: "Acceptable use",
    body: "Do not use Codewhale to break the law, harm people or systems, evade access controls, distribute malware, interfere with the service, or process data you lack authority to use. Automated actions remain subject to the permissions, reviews, and stop conditions shown in the product.",
  },
  {
    title: "Your content and connected services",
    body: "You retain ownership of your content. You give Shannon Labs the limited permission needed to process it to provide Codewhale. Model, repository, compute, and other third-party providers have their own terms. You are responsible for charges you authorize with those providers.",
  },
  {
    title: "Plans and charges",
    body: "Features described as free do not create a Codewhale payment obligation. If paid Codewhale features become active, the product will show the price and obtain authorization before charging. Provider-billed model usage remains outside Codewhale charges.",
  },
  {
    title: "Service changes and termination",
    body: "We may change, suspend, or discontinue features and may restrict accounts that violate these terms or threaten the service. You may stop using Codewhale and use the available account deletion process. Required security, audit, deletion, and financial receipts may survive account deletion as described in the privacy policy.",
  },
  {
    title: "Disclaimers and liability",
    body: "Codewhale is provided on an “as is” and “as available” basis to the extent permitted by law. Software agents can make mistakes; review important changes and keep independent backups. Shannon Labs does not promise uninterrupted or error-free operation and is not responsible for third-party services outside its control.",
  },
  {
    title: "Changes",
    body: "We may update these terms. The effective date above identifies the current version. Continued use after an update means you accept the revised terms.",
  },
] as const;

export const PRIVACY_SECTIONS = [
  {
    title: "Information we collect",
    body: "We collect the identity information needed to create and protect your account, including your GitHub identity, display name, and a primary verified email when GitHub makes one available. We also store product information you create, such as preferences, projects, conversations, Work runs, approvals, and security or operational receipts.",
  },
  {
    title: "How we use it",
    body: "We use this information to authenticate you, operate and secure Codewhale, preserve your requested product state, provide support, and—only when you have enabled the relevant communication—send product or waitlist updates. We do not sell personal information.",
  },
  {
    title: "Storage and processing",
    body: "Codewhale is one global product. There is no residency selector, no region-specific account, and no promise that your account data stays in a particular jurisdiction. Account authentication and session state is stored in Cloudflare Durable Objects, today configured in Cloudflare’s US jurisdiction; that placement is an operational choice we may change, and we will update this policy when we do. Durable product state is owned by Codewhale’s private product runtime. Cloudflare may perform TLS, request routing, and cryptographic processing on its global edge, so we do not describe edge processing as US-only. Hosted compute is a separate system from account storage and, when enabled, identifies its placement before launch.",
  },
  {
    title: "Model providers and repositories",
    body: "Codewhale sends content to a model provider only when you choose or connect that provider. Bring-your-own provider credentials remain separate from Codewhale billing. Repository access is limited to the grants you approve with the source provider.",
  },
  {
    title: "Retention and deletion",
    body: "You can review export and deletion controls after signing in under Account, Data & privacy. Content is deleted according to the displayed retention and deletion process. Privacy-minimal security, audit, deletion, and financial receipts may be retained when necessary to prove an action, prevent abuse, or meet legal obligations. Provider-backed resources are not treated as deleted until the provider confirms deletion.",
  },
  {
    title: "Questions and requests",
    body: "Use Account, Data & privacy after signing in to request an export or deletion. If you cannot sign in, use the public support channel shown in the product once it is verified. Codewhale does not present an unverified mailbox as monitored.",
  },
  {
    title: "Changes",
    body: "We may update this policy as the product and its processors change. The effective date above identifies the version that applies.",
  },
] as const;
