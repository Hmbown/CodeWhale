import type { ChromeDict } from "../types";

/**
 * Polish chrome dictionary.
 *
 * Natywny przekład w aktualnym kierunku angielskiego — „any model, on your
 * machine", bez wycofanego pozycjonowania „local-first". Rejestr bezpośredni
 * (ty), standard polskich narzędzi deweloperskich.
 *
 * Tryby i postawy uprawnień zostają dosłowne (Plan / Work / Operate, Ask /
 * Auto-Review / Full Access); `Runtime`, `fleet` i `TUI` to nazwy produktu;
 * „receipt" to potwierdzenie.
 *
 * Drugie etykiety nawigacji parują polską etykietę z krótkim angielskim
 * odpowiednikiem — para han to zabieg redakcyjny wydania angielskiego.
 */
export const chrome: ChromeDict = {
  navDocs: "Dokumentacja",
  navStart: "Pierwsze kroki",
  navInstall: "Instalacja",
  navFaq: "FAQ",
  navCommunity: "Społeczność",
  navContribute: "Współtwórz",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Przejdź do treści głównej",

  navPrimaryAria: "Nawigacja główna",
  navHomeAria: "Strona główna Codewhale",

  installCta: "Instaluj →",

  authSignIn: "Zaloguj się",
  authRegister: "Zarejestruj się",
  authGroupAria: "Konto",

  wordmarkSeal: "深",
  wordmarkTag: "dowolny model, na twojej maszynie",

  issueLabel: "Wydanie z {date}",
  dateLocale: "pl-PL",

  starsAria: "Gwiazdki na GitHubie",
  githubFallback: "GitHub",

  tickerLiveLabel: "Na żywo",
  tickerLiveTag: "LIVE",
  tickerMerged: "scalono",
  tickerOpened: "otwarto",
  tickerClosed: "zamknięto",
  tickerReleased: "wydano",
  tickerFirstContribution: "pierwszy wkład",
  tickerBy: "autor: {handle}",
  tickerAria: "Ostatnia aktywność w repozytorium",

  traceLabel: "ślad rozumowania",
  traceTabsAria: "Fragmenty sesji",

  menuOpen: "Otwórz menu",
  menuClose: "Zamknij menu",

  themeAuto: "auto",
  themeLight: "jasny",
  themeDark: "ciemny",
  themeAria: "Motyw dokumentacji: {mode} (kliknij, aby przełączyć)",
  themeTitle: "Motyw dokumentacji · auto / jasny / ciemny",

  footerTagline:
    "Codewhale zanurza się w głębinach, żebyś ty nie musiał — dokumentacja, kod źródłowy i społeczność otwartoźródłowego runtime'u.",
  footerProduct: "Produkt",
  footerProject: "Projekt",
  footerDocs: "Dokumentacja",
  footerGuide: "Pierwsze kroki",
  footerInstall: "Instalacja",
  footerModels: "Modele",
  footerRuntime: "Runtime",
  footerFaq: "FAQ",
  footerIssues: "Issues",
  footerContribute: "Współtwórz",
  footerLicense: "Licencja MIT",
  footerPricing: "Cennik",
  footerTerms: "Warunki usługi",
  footerPrivacy: "Prywatność",
  footerChangelog: "Dziennik zmian",
  footerCanonicalSource: "Repozytorium źródłowe: ",
  footerReleases: " · Wydania: ",
  footerReleasesLink: "Wydania na GitHubie",
  footerSecurity: "Bezpieczeństwo",

  switcherLabel: "Język",
  switcherSwitchTo: "Przełącz na: {label}",
  partialBadge: "(częściowo)",
};
