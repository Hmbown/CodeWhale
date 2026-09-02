import type { ChromeDict } from "../types";

/**
 * German chrome dictionary.
 *
 * Native Neufassung in der aktuellen englischen Richtung — „any model, on
 * your machine", nicht die eingestellte „local-first"-Positionierung.
 * Register wie der TUI-Pack (crates/tui/locales/de.json): knapp, nominal,
 * Anrede per Du in der Community-Tradition deutscher Entwicklerwerkzeuge.
 *
 * Terminologie folgt dem TUI-Pack: Modi und Permission-Posturen bleiben
 * literal (Plan / Work / Operate, Ask / Auto-Review / Full Access),
 * „Berechtigungen" ist permissions, „Laufzeitbeleg" ist receipt; `Runtime`,
 * `fleet` und `TUI` bleiben Produktnamen.
 *
 * Die sekundären Nav-Labels paaren das deutsche Primary mit einem kurzen
 * englischen Gegenstück — das Han-Paar ist das Editorial der englischen
 * Ausgabe.
 */
export const chrome: ChromeDict = {
  navDocs: "Dokumentation",
  navStart: "Erste Schritte",
  navInstall: "Installieren",
  navFaq: "FAQ",
  navCommunity: "Community",
  navContribute: "Mitwirken",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Zum Hauptinhalt springen",

  navPrimaryAria: "Hauptnavigation",
  navHomeAria: "Codewhale-Startseite",

  installCta: "Installieren →",

  authSignIn: "Anmelden",
  authRegister: "Registrieren",
  authGroupAria: "Konto",

  wordmarkSeal: "深",
  wordmarkTag: "jedes Modell, auf deiner Maschine",

  issueLabel: "Ausgabe vom {date}",
  dateLocale: "de-DE",

  starsAria: "GitHub-Sterne",
  githubFallback: "GitHub",

  tickerLiveLabel: "Echtzeit",
  tickerLiveTag: "LIVE",
  tickerMerged: "gemerged",
  tickerOpened: "geöffnet",
  tickerClosed: "geschlossen",
  tickerReleased: "veröffentlicht",
  tickerFirstContribution: "erster Beitrag",
  tickerBy: "von {handle}",
  tickerAria: "Letzte Aktivität im Repository",

  traceLabel: "Reasoning-Trace",
  traceTabsAria: "Sitzungsausschnitte",

  menuOpen: "Menü öffnen",
  menuClose: "Menü schließen",

  themeAuto: "auto",
  themeLight: "hell",
  themeDark: "dunkel",
  themeAria: "Dokumentations-Design: {mode} (Klick zum Wechseln)",
  themeTitle: "Dokumentations-Design · auto / hell / dunkel",

  footerTagline:
    "Codewhale taucht in die Tiefe, damit du es nicht musst — Docs, Quellcode und Community für die Open-Source-Runtime.",
  footerProduct: "Produkt",
  footerProject: "Projekt",
  footerDocs: "Dokumentation",
  footerGuide: "Erste Schritte",
  footerInstall: "Installation",
  footerModels: "Modelle",
  footerRuntime: "Runtime",
  footerFaq: "FAQ",
  footerIssues: "Issues",
  footerContribute: "Mitwirken",
  footerLicense: "MIT-Lizenz",
  footerPricing: "Preise",
  footerTerms: "Nutzungsbedingungen",
  footerPrivacy: "Datenschutz",
  footerChangelog: "Änderungsprotokoll",
  footerCanonicalSource: "Kanonische Quelle: ",
  footerReleases: " · Releases: ",
  footerReleasesLink: "GitHub-Releases",
  footerSecurity: "Sicherheit",

  switcherLabel: "Sprache",
  switcherSwitchTo: "Zu {label} wechseln",
  partialBadge: "(teilweise)",
};
