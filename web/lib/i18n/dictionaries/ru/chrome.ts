import type { ChromeDict } from "../types";

/**
 * Russian chrome dictionary. Key parity with `en/chrome.ts` is enforced by
 * `npm run check:locales` and `dictionaries.test.ts`.
 *
 * Terminology follows the TUI ru locale pack (`crates/tui/locales/ru.json`):
 * the modes Plan / Act / Operate and the permission postures
 * Ask / Auto-Review / Full Access stay Latin, wrapped in Russian prose
 * ("режим Operate", "режим разрешений"). The 深 seal is the masthead's mark,
 * not prose, and is shared across locales.
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

  installCta: "Установка →",

  wordmarkSeal: "深",
  wordmarkTag: "любая модель · работает локально",

  issueLabel: "Выпуск {date}",
  dateLocale: "ru-RU",

  starsAria: "Звёзды на GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "Эфир",
  tickerLiveTag: "LIVE",

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
    "Мы ныряем в глубину, чтобы не пришлось вам — документация, исходный код и сообщество рантайма с открытым кодом.",
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
  footerCanonicalSource: "Канонический источник: ",
  footerReleases: " · Релизы: ",
  footerReleasesLink: "Релизы на GitHub",
  footerSecurity: "Безопасность",

  switcherLabel: "Язык",
  switcherSwitchTo: "Переключиться на {label}",
  partialBadge: "(частично)",
};
