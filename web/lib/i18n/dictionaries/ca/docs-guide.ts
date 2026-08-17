import type { DocsGuideDict } from "../types";

/**
 * Catalan dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Primers passos · Documentació de Codewhale",
  metaDescription:
    "El camí complet de la instal·lació a la teva Fleet ideal: instal·lació, una primera sessió sense claus, connexió d’un proveïdor i configuració de la Fleet.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Primers passos",
  overviewLead:
    "Quatre passos d’una ordre d’instal·lació a una Fleet a punt per a la teva feina. Cada pas només afirma el que el candidat actual fa realment; allò no publicat o no enregistrat es marca com a tal.",
  sessionTitle: "Mira una sessió real",
  sessionLead:
    "A sota hi ha l’espai reservat al material de sessió real. És deliberadament en estat pendent: fins que existeixi l’enregistrament dogfood del candidat v0.9.2, aquest lloc no mostra cap peça de substitució ni cap escena muntada.",
  nextTitle: "I ara què",
  sourceNote:
    "Documents font: docs/GUIDE.md, docs/KEYBINDINGS.md · El text dels passos viu a web/lib/content/getting-started.ts; actualitza docs-map.ts en fer canvis.",
};
