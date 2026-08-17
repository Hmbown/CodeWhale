import type { DocsGuideDict } from "../types";

/**
 * German dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Erste Schritte · Codewhale-Dokumentation",
  metaDescription:
    "Der komplette Weg von der Installation bis zu deiner idealen Fleet: Installation, eine erste schlüssellose Sitzung, Provider-Anbindung und Fleet-Setup.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Erste Schritte",
  overviewLead:
    "Vier Schritte von einem Installationsbefehl bis zur einsatzbereiten Fleet. Jeder Schritt beschreibt nur, was der aktuelle Kandidat wirklich tut; alles Unveröffentlichte oder nicht Aufgezeichnete ist als solches gekennzeichnet.",
  sessionTitle: "Eine echte Sitzung ansehen",
  sessionLead:
    "Unten ist der Slot für echtes Sitzungsmaterial. Er ist bewusst im Status „ausstehend“: Solange die Dogfood-Aufnahme des v0.9.2-Kandidaten nicht existiert, zeigt diese Seite kein Platzhalter- oder gestellt Material.",
  nextTitle: "Wie geht es weiter",
  sourceNote:
    "Quelldokumente: docs/GUIDE.md, docs/KEYBINDINGS.md · Der Schritttext lebt in web/lib/content/getting-started.ts; bei Änderungen docs-map.ts mitpflegen.",
};
