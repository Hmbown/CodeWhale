import type { ChromeDict } from "../types";

/**
 * Russian chrome dictionary — native rewrite mirroring the current English
 * direction. Key parity with `en/chrome.ts` is enforced by
 * `npm run check:locales` and `dictionaries.test.ts`.
 *
 * Terminology follows the TUI ru locale pack (`crates/tui/locales/ru.json`):
 * the modes Plan / Work / Operate and the permission postures
 * Ask / Auto-Review / Full Access stay Latin, wrapped in Russian prose
 * ("режим Operate", "режим разрешений"). The 深 seal is the masthead's mark,
 * not prose, and is shared across locales. Native nav labels pair with short
 * English secondaries, per the masthead convention.
 */
export const chrome: ChromeDict = {
  navDocs: "Документация",
  navStart: "Начало",
  navInstall: "Установка",
  navFaq: "Вопросы",
  navCommunity: "Сообщество",
  navContribute: "Участие",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Перейти к основному содержимому",


  navPrimaryAria: "Основная навигация",
  navHomeAria: "Главная Codewhale",

  installCta: "Установить →",

  authSignIn: "Войти",
  authRegister: "Регистрация",
  authGroupAria: "Аккаунт",

  wordmarkSeal: "深",
  wordmarkTag: "любая модель, на вашей машине",

  issueLabel: "Выпуск {date}",
  dateLocale: "ru-RU",

  starsAria: "Звёзды на GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "Эфир",
  tickerLiveTag: "LIVE",
  tickerMerged: "влит",
  tickerOpened: "открыт",
  tickerClosed: "закрыт",
  tickerReleased: "выпуск",
  tickerFirstContribution: "первый вклад",
  tickerBy: "автор {handle}",
  tickerAria: "Недавняя активность репозитория",

  traceLabel: "ход рассуждений",
  traceTabsAria: "Фрагменты сеанса",

  menuOpen: "Открыть меню",
  menuClose: "Закрыть меню",

  themeAuto: "авто",
  themeLight: "светлая",
  themeDark: "тёмная",
  themeAria: "Тема документации: {mode} (нажмите, чтобы переключить)",
  themeTitle: "Тема документации · авто / светлая / тёмная",

  footerTagline:
    "Codewhale ныряет в глубину, чтобы вам не пришлось — документация, исходники и сообщество рантайма с открытым кодом.",
  footerProduct: "Продукт",
  footerProject: "Проект",
  footerDocs: "Документация",
  footerGuide: "С чего начать",
  footerInstall: "Установка",
  footerModels: "Модели",
  footerRuntime: "Рантайм",
  footerFaq: "Вопросы и ответы",
  footerIssues: "Задачи",
  footerContribute: "Участие",
  footerLicense: "Лицензия MIT",
  footerPricing: "Цены",
  footerTerms: "Условия использования",
  footerPrivacy: "Конфиденциальность",
  footerChangelog: "Журнал изменений",
  footerCanonicalSource: "Канонический источник: ",
  footerReleases: " · Релизы: ",
  footerReleasesLink: "Релизы на GitHub",
  footerSecurity: "Безопасность",

  switcherLabel: "Язык",
  switcherSwitchTo: "Переключиться на {label}",
  partialBadge: "(частично)",
};
