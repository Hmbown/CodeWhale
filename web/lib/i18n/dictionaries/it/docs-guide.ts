import type { DocsGuideDict } from "../types";

/**
 * Italian dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Primi passi · Documentazione di Codewhale",
  metaDescription:
    "Il percorso completo dall'installazione alla Fleet ideale: installazione, una prima sessione senza chiavi, collegamento di un provider e configurazione della Fleet.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Primi passi",
  overviewLead:
    "Quattro passi da un comando d'installazione a una Fleet pronta per il tuo lavoro. Ogni passo afferma solo ciò che il candidato attuale fa davvero; tutto ciò che non è rilasciato o registrato è etichettato come tale.",
  sessionTitle: "Guarda una sessione reale",
  sessionLead:
    "Qui sotto c'è lo slot multimediale della sessione reale. È volutamente in stato pending: finché non esiste la registrazione dogfood del candidato v0.9.2, questo sito non mostra segnaposto o riprese preparate.",
  nextTitle: "E adesso",
  sourceNote:
    "Documenti sorgente: docs/GUIDE.md, docs/KEYBINDINGS.md · Il testo dei passi vive in web/lib/content/getting-started.ts; aggiorna docs-map.ts quando lo cambi.",
};
