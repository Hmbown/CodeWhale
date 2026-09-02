import type { DocsGuideDict } from "../types";

/**
 * German dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Erste Schritte · Codewhale-Dokumentation",
  metaDescription:
    "Der komplette Weg von der Installation bis zu deiner idealen fleet: Installation, eine erste schlüssellose Sitzung, Provider-Anbindung und fleet-Setup.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Erste Schritte",
  overviewLead:
    "Vier Schritte von einem Installationsbefehl bis zur einsatzbereiten fleet.",
  sessionTitle: "Eine echte Sitzung ansehen",
  sessionLead:
    "Hier erscheint die Aufnahme einer echten Sitzung. Es gibt noch keine Aufnahme, daher wird nichts angezeigt.",
  nextTitle: "Wie geht es weiter",
  sourceNote:
    "Quelldokumente: docs/GUIDE.md, docs/KEYBINDINGS.md · Der Schritttext lebt in web/lib/content/getting-started.ts; bei Änderungen docs-map.ts mitpflegen.",
};
