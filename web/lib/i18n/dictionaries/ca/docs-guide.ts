import type { DocsGuideDict } from "../types";

/**
 * Catalan dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Primers passos · Documentació de Codewhale",
  metaDescription:
    "El camí complet de la instal·lació a la teva fleet ideal: instal·lació, una primera sessió sense claus, connexió d’un proveïdor i configuració de la fleet.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Primers passos",
  overviewLead:
    "Quatre passos d’una ordre d’instal·lació a una fleet a punt per a la teva feina.",
  sessionTitle: "Mira una sessió real",
  sessionLead:
    "Aquí hi anirà l’enregistrament d’una sessió real. Encara no n’hi ha cap, per això no es mostra res.",
  nextTitle: "I ara què",
  sourceNote:
    "Documents font: docs/GUIDE.md, docs/KEYBINDINGS.md · El text dels passos viu a web/lib/content/getting-started.ts; actualitza docs-map.ts en fer canvis.",
};
