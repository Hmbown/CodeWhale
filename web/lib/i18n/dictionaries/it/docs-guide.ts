import type { DocsGuideDict } from "../types";

/**
 * Italian dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Primi passi · Documentazione di Codewhale",
  metaDescription:
    "Il percorso completo dall'installazione alla fleet ideale: installazione, una prima sessione senza chiavi, collegamento di un provider e configurazione della fleet.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Primi passi",
  overviewLead:
    "Quattro passi da un comando d'installazione a una fleet pronta per il tuo lavoro.",
  sessionTitle: "Guarda una sessione reale",
  sessionLead:
    "Qui andrà la registrazione di una sessione reale. Non esiste ancora, quindi non viene mostrato nulla.",
  nextTitle: "E adesso",
  sourceNote:
    "Documenti sorgente: docs/GUIDE.md, docs/KEYBINDINGS.md · Il testo dei passi vive in web/lib/content/getting-started.ts; aggiorna docs-map.ts quando lo cambi.",
};
