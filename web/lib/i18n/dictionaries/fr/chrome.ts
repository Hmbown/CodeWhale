import type { ChromeDict } from "../types";

/**
 * French chrome dictionary.
 *
 * Native rewrite mirroring the current English direction — « any model, on
 * your machine », pas l'ancien positionnement « local-first ». Registre en
 * vouvoiement, aligné sur le pack TUI (crates/tui/locales/fr.json) :
 * « Exécutez `{command}` pour confirmer ».
 *
 * Terminologie alignée sur le pack TUI : les modes et les postures de
 * permissions restent littéraux (Plan / Work / Operate, Ask / Auto-Review /
 * Full Access), « receipt » est « reçu », « runtime », `fleet` et `TUI`
 * restent des noms produits. L'apostrophe est typographique (’).
 *
 * Les libellés secondaires de navigation associent le libellé français à un
 * court équivalent anglais — le couple han (文档 / 指引 / …) est le procédé
 * éditorial propre à l'édition anglaise.
 */
export const chrome: ChromeDict = {
  navDocs: "Documentation",
  navStart: "Premiers pas",
  navInstall: "Installer",
  navFaq: "FAQ",
  navCommunity: "Communauté",
  navContribute: "Contribuer",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Aller au contenu principal",

  navPrimaryAria: "Navigation principale",
  navHomeAria: "Accueil de Codewhale",

  installCta: "Installer →",

  authSignIn: "Se connecter",
  authRegister: "Créer un compte",
  authGroupAria: "Compte",

  wordmarkSeal: "深",
  wordmarkTag: "n’importe quel modèle, sur votre machine",

  issueLabel: "Édition du {date}",
  dateLocale: "fr-FR",

  starsAria: "Étoiles GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "En direct",
  tickerLiveTag: "LIVE",
  tickerMerged: "fusionné",
  tickerOpened: "ouvert",
  tickerClosed: "fermé",
  tickerReleased: "publié",
  tickerFirstContribution: "première contribution",
  tickerBy: "par {handle}",
  tickerAria: "Activité récente du dépôt",

  traceLabel: "trace de raisonnement",
  traceTabsAria: "Extraits de session",

  menuOpen: "Ouvrir le menu",
  menuClose: "Fermer le menu",

  themeAuto: "auto",
  themeLight: "clair",
  themeDark: "sombre",
  themeAria: "Thème de la documentation : {mode} (cliquer pour changer)",
  themeTitle: "Thème de la documentation · auto / clair / sombre",

  footerTagline:
    "Codewhale plonge dans les profondeurs à votre place — documentation, code source et communauté du runtime open source.",
  footerProduct: "Produit",
  footerProject: "Projet",
  footerDocs: "Documentation",
  footerGuide: "Premiers pas",
  footerInstall: "Installation",
  footerModels: "Modèles",
  footerRuntime: "Runtime",
  footerFaq: "FAQ",
  footerIssues: "Issues",
  footerContribute: "Contribuer",
  footerLicense: "Licence MIT",
  footerPricing: "Tarifs",
  footerTerms: "Conditions d’utilisation",
  footerPrivacy: "Confidentialité",
  footerChangelog: "Journal des modifications",
  footerCanonicalSource: "Source canonique : ",
  footerReleases: " · Versions : ",
  footerReleasesLink: "Versions GitHub",
  footerSecurity: "Sécurité",

  switcherLabel: "Langue",
  switcherSwitchTo: "Passer en {label}",
  partialBadge: "(partiel)",
};
