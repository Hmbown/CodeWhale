/**
 * Public membership copy is intentionally separate from the billing catalog.
 * The public site may describe what is usable now, but it must not promote a
 * price or allowance until that commercial term has its own approval.
 */

import type { LocalizedText } from "./vocabulary";

export interface PublicMembershipOption {
  id: "account" | "local" | "paid";
  title: LocalizedText;
  body: LocalizedText;
}

/** Machine-checkable launch state; copy tests assert these truths, not wording. */
export const PUBLIC_MEMBERSHIP_STATUS = {
  checkout: "dormant",
  paymentFromPage: false,
  localUseRequiresPaidMembership: false,
  commercialTerms: "not-published",
} as const;

export const PUBLIC_MEMBERSHIP_COPY = {
  metadata: {
    title: { en: "Membership · Codewhale", zh: "会员 · Codewhale" },
    description: {
      en: "Create a Codewhale account or keep using the local runtime with your own model provider. Paid membership is not on sale on this deployment.",
      zh: "创建 Codewhale 账户，或继续在本地运行时中使用你自己的模型提供商。此部署尚未销售付费会员。",
    },
  },
  kicker: { en: "Membership", zh: "会员" },
  title: {
    en: "Start with an account. Keep local use in your hands.",
    zh: "从账户开始。本地使用始终由你掌控。",
  },
  lead: {
    en: "Create an account for the Codewhale app, or run the open-source Runtime on your own machine with a model provider you choose. Paid checkout is dormant here, so this page cannot charge you.",
    zh: "创建账户以使用 Codewhale 应用，或在自己的机器上运行开源 Runtime，并选择自己的模型提供商。此处的付费结账处于休眠状态，因此本页无法向你收费。",
  },
  options: [
    {
      id: "account",
      title: { en: "Codewhale account", zh: "Codewhale 账户" },
      body: {
        en: "Keep your conversations and work together in the app. Creating an account does not start a paid plan.",
        zh: "在应用中统一保留对话和工作。创建账户不会启动付费方案。",
      },
    },
    {
      id: "local",
      title: { en: "Local Runtime / BYOK", zh: "本地 Runtime / 自带密钥" },
      body: {
        en: "Install the open-source Runtime, work on your own machine, and use your own provider credentials. A paid membership is not required for local use.",
        zh: "安装开源 Runtime，在自己的机器上工作，并使用自己的提供商凭据。本地使用不需要付费会员。",
      },
    },
    {
      id: "paid",
      title: { en: "Paid membership", zh: "付费会员" },
      body: {
        en: "Public prices, included capacity, overage, concurrency, and storage terms are not published until they are separately approved. When sales open, Billing must show the current terms before payment.",
        zh: "公开价格、包含容量、超额用量、并发和存储条款将在分别获批后发布。销售开放时，账单页面必须在付款前显示当时有效的条款。",
      },
    },
  ] satisfies PublicMembershipOption[],
  note: {
    en: "No paid usage starts from this page. Any future checkout must disclose the exact plan, renewal terms, and what happens when included capacity is exhausted before you pay.",
    zh: "本页不会启动任何付费用量。未来的任何结账都必须在付款前说明确切方案、续订条款，以及包含容量用尽后的处理方式。",
  },
  actions: {
    createAccount: { en: "Create account", zh: "创建账户" },
    continueLocally: { en: "Continue locally", zh: "继续在本地使用" },
    signIn: { en: "Already have an account? Sign in", zh: "已有账户？登录" },
  },
} as const;
