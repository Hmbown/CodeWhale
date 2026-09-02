import type { ChromeDict } from "../types";

/**
 * Catalan chrome dictionary.
 *
 * Reescriptura nativa en la direcció anglesa actual — «any model, on your
 * machine», no el posicionament «local-first» retirat. Registre amb tu,
 * com el pack TUI (crates/tui/locales/ca.json): «Executa `{command}` per
 * confirmar».
 *
 * Terminologia alineada amb el pack TUI: els modes i les postures de
 * permisos es mantenen literals (Plan / Work / Operate, Ask / Auto-Review /
 * Full Access), «permisos» és permissions, «resguard» és receipt; `Runtime`,
 * `fleet` i `TUI` resten noms de producte.
 *
 * Les etiquetes secundàries de navegació aparien l’etiqueta catalana amb
 * un equivalent anglès curt — la parella han és el recurs editorial propi
 * de l’edició anglesa.
 */
export const chrome: ChromeDict = {
  navDocs: "Documentació",
  navStart: "Començar",
  navInstall: "Instal·lar",
  navFaq: "PMF",
  navCommunity: "Comunitat",
  navContribute: "Col·laborar",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Salta al contingut principal",

  navPrimaryAria: "Navegació principal",
  navHomeAria: "Inici de Codewhale",

  installCta: "Instal·la →",

  authSignIn: "Inicia la sessió",
  authRegister: "Registra't",
  authGroupAria: "Compte",

  wordmarkSeal: "深",
  wordmarkTag: "qualsevol model, a la teva màquina",

  issueLabel: "Edició del {date}",
  dateLocale: "ca-ES",

  starsAria: "Estrelles a GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "En directe",
  tickerLiveTag: "LIVE",
  tickerMerged: "fusionat",
  tickerOpened: "obert",
  tickerClosed: "tancat",
  tickerReleased: "publicat",
  tickerFirstContribution: "primera contribució",
  tickerBy: "per {handle}",
  tickerAria: "Activitat recent del repositori",

  traceLabel: "traça de raonament",
  traceTabsAria: "Extractes de sessió",

  menuOpen: "Obre el menú",
  menuClose: "Tanca el menú",

  themeAuto: "auto",
  themeLight: "clar",
  themeDark: "fosc",
  themeAria: "Tema de la documentació: {mode} (fes clic per canviar)",
  themeTitle: "Tema de la documentació · auto / clar / fosc",

  footerTagline:
    "Codewhale s’immergeix a les profunditats per tu — documentació, codi font i comunitat del runtime de codi obert.",
  footerProduct: "Producte",
  footerProject: "Projecte",
  footerDocs: "Documentació",
  footerGuide: "Primers passos",
  footerInstall: "Instal·lació",
  footerModels: "Models",
  footerRuntime: "Runtime",
  footerFaq: "PMF",
  footerIssues: "Issues",
  footerContribute: "Col·laborar",
  footerLicense: "Llicència MIT",
  footerPricing: "Preus",
  footerTerms: "Termes del servei",
  footerPrivacy: "Privadesa",
  footerChangelog: "Registre de canvis",
  footerCanonicalSource: "Font canònica: ",
  footerReleases: " · Versions: ",
  footerReleasesLink: "Versions de GitHub",
  footerSecurity: "Seguretat",

  switcherLabel: "Llengua",
  switcherSwitchTo: "Canvia a {label}",
  partialBadge: "(parcial)",
};
