import type { ChromeDict } from "../types";

/**
 * Ukrainian chrome pack. Terminology follows the TUI locale pack
 * (`crates/tui/locales/uk.json`): режим дозволів for the permission posture,
 * провайдер, репозиторій, композер, міркування. Plan / Act / Operate and
 * Ask / Auto-Review / Full Access stay literal there and stay literal here.
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

  wordmarkSeal: "深",
  wordmarkTag: "будь-яка модель, спершу локально",

  issueLabel: "Випуск {date}",
  dateLocale: "uk-UA",

  starsAria: "Зірки на GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "Наживо",
  tickerLiveTag: "LIVE",

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
    "Ми занурюємось у глибину, щоб не довелося вам — документація, код і спільнота рантайму з відкритим кодом.",
  footerProduct: "Продукт",
  footerProject: "Проєкт",
  footerDocs: "Документація",
  footerGuide: "З чого почати",
  footerInstall: "Встановлення",
  footerModels: "Моделі",
  footerRuntime: "Рантайм",
  footerFaq: "Питання та відповіді",
  footerIssues: "Issues",
  footerContribute: "Участь",
  footerLicense: "Ліцензія MIT",
  footerCanonicalSource: "Канонічне джерело: ",
  footerReleases: " · Релізи: ",
  footerReleasesLink: "Релізи на GitHub",
  footerSecurity: "Безпека",

  switcherLabel: "Мова",
  switcherSwitchTo: "Перемкнути на {label}",
  partialBadge: "(частково)",
};
