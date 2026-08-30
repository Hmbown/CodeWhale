import type { ChromeDict } from "../types";

/**
 * Ukrainian chrome pack — native rewrite mirroring the current English copy;
 * the old "local-first" tag is gone ("any model, on your machine" instead).
 * Terminology follows the TUI locale pack (`crates/tui/locales/uk.json`):
 * режим дозволів for the permission posture, провайдер, репозиторій,
 * композер, міркування. Plan / Work / Operate and Ask / Auto-Review /
 * Full Access stay literal there and stay literal here.
 *
 * Secondary nav labels pair the Ukrainian primary with a short English
 * companion — the Han pair is the English edition's own device.
 */
export const chrome: ChromeDict = {
  navDocs: "Документація",
  navStart: "Початок",
  navInstall: "Встановлення",
  navFaq: "Питання",
  navCommunity: "Спільнота",
  navContribute: "Участь",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Перейти до основного вмісту",


  navPrimaryAria: "Основна навігація",
  navHomeAria: "Головна сторінка Codewhale",

  installCta: "Встановити →",

  authSignIn: "Увійти",
  authRegister: "Реєстрація",
  authGroupAria: "Обліковий запис",

  wordmarkSeal: "深",
  wordmarkTag: "будь-яка модель, на вашій машині",

  issueLabel: "Випуск {date}",
  dateLocale: "uk-UA",

  starsAria: "Зірки на GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "Наживо",
  tickerLiveTag: "LIVE",
  tickerMerged: "влито",
  tickerOpened: "відкрито",
  tickerClosed: "закрито",
  tickerReleased: "реліз",
  tickerFirstContribution: "перший внесок",
  tickerBy: "автор {handle}",
  tickerAria: "Нещодавня активність репозиторію",

  traceLabel: "хід міркувань",
  traceTabsAria: "Фрагменти сеансу",

  menuOpen: "Відкрити меню",
  menuClose: "Закрити меню",

  themeAuto: "авто",
  themeLight: "світла",
  themeDark: "темна",
  themeAria: "Тема документації: {mode} (натисніть, щоб перемкнути)",
  themeTitle: "Тема документації · авто / світла / темна",

  footerTagline:
    "Codewhale занурюється в глибину, щоб не довелося вам — документація, код і спільнота рантайму з відкритим кодом.",
  footerProduct: "Продукт",
  footerProject: "Проєкт",
  footerDocs: "Документація",
  footerGuide: "З чого почати",
  footerInstall: "Встановлення",
  footerModels: "Моделі",
  footerRuntime: "Рантайм",
  footerFaq: "Питання та відповіді",
  footerIssues: "Проблеми",
  footerContribute: "Участь",
  footerLicense: "Ліцензія MIT",
  footerPricing: "Ціни",
  footerTerms: "Умови використання",
  footerPrivacy: "Приватність",
  footerChangelog: "Журнал змін",
  footerCanonicalSource: "Канонічне джерело: ",
  footerReleases: " · Релізи: ",
  footerReleasesLink: "Релізи на GitHub",
  footerSecurity: "Безпека",

  switcherLabel: "Мова",
  switcherSwitchTo: "Перемкнути на {label}",
  partialBadge: "(частково)",
};
