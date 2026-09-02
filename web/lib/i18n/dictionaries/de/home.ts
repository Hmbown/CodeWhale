import type { HomeDict } from "../types";

/**
 * German home dictionary — die „Zeitung-Ozean"-Landingpage.
 *
 * Native Neufassung in der aktuellen englischen Richtung: eigenes Modell
 * mitbringen, alles läuft auf der eigenen Maschine. Produktvokabular bleibt
 * literal wie im TUI-Pack: Plan / Work / Operate, Ask / Auto-Review /
 * Full Access, Codewhale, TUI, `codewhale exec`, Runtime API + MCP, fleet,
 * Node 18+, Rust, MIT.
 *
 * Die Sektions-Siegel (法, 行, …) sind Glyphen, die mit der englischen
 * Ausgabe geteilt werden — Marken, kein Fließtext.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — taucht in die Tiefe, damit du es nicht musst.",
  metaDescription:
    "Codewhale taucht in die Tiefe, damit du es nicht musst — ein Open-Source-Coding-Agent für das Terminal. Eigenes Modell mitbringen. Läuft auf deiner Maschine. Rust, MIT.",

  kicker: "Open Source · Eigenes Modell mitbringen · Läuft im Terminal",
  heroTitleA: "Codewhale taucht in die Tiefe,",
  heroTitleB: "damit du es nicht musst.",
  heroIntro:
    "{brand} ist ein Open-Source-Coding-Agent für dein Terminal. Gib ihm ein Modell und eine Aufgabe — er liest deinen Code, bearbeitet Dateien, prüft selbst und stoppt, wenn die Arbeit erledigt ist oder er dich braucht. Bring jedes Modell mit, oder mische sie: pinne pro Rolle ein eigenes Modell fest.",
  install: "Installieren",
  docs: "Dokumentation",
  copy: "Kopieren",
  copied: "Kopiert ✓",

  installEyebrow: "Installation mit einer Zeile",
  installRequirement: "braucht Node 18+ — keine Rust-Toolchain",
  installOtherWays: "weitere Wege →",

  latestRelease: "Aktuellstes Release {tag}",
  releaseUnavailable: "Release-Status nicht verfügbar",
  currentSource: "Quelle",
  sourceCandidate: "Unveröffentlicht",
  providerRoutes: "{count} Provider",
  publishedRelease: "veröffentlicht",
  figcaptionSourceCandidate: "unveröffentlicht",

  shotSession: "Aktuelle Sitzung",
  screenshotAlt:
    "Aktuelle Codewhale-Terminalsitzung mit Operate-Modus, dem Wal, dem Composer und der Fußzeile",
  figcaption: "Aktuelle Codewhale-Sitzung · Operate-Modus · Ask-Permission-Posture",

  proofHeading: "Eine Unterwasser-Terminal-Shell. Jedes Modell. Auf deiner Maschine.",
  proofBody:
    "Bring das Modell mit, das du schon nutzt — gehostet, über Gateway oder lokal. Plan / Work / Operate und explizite Permission-Posturen halten den Tauchgang unter deiner Kontrolle.",

  sealDecides: "法",
  decidesEyebrow: "Sieh, wie es entscheidet",
  decidesHeading: "Regeln, die du in der Trace sehen kannst",
  decidesLede:
    "Echte Sitzungsausschnitte — die gerankte Projekt-Law erscheint im Reasoning des Modells, nicht nur als Behauptung auf einer Landingpage.",

  sealWorkflow: "行",
  workflowHeading: "Von der Aufgabe zur verifizierten Änderung.",
  workflow: [
    ["Inspizieren", "Repository, Anweisungen und Aufgabe lesen."],
    ["Handeln", "Dateien innerhalb expliziter Freigabegrenzen bearbeiten."],
    ["Verifizieren", "Checks ausführen und das Ergebnis prüfen."],
    ["Berichten", "Einen knappen, dauerhaften Laufzeitbeleg hinterlassen."],
  ],
  receiptAria: "Beispiel-Laufzeitbeleg",
  receiptInspect: "Repository und Anweisungen",
  receiptAct: "Bearbeiten über die gewählte Permission-Posture",
  receiptReport: "Checks bestanden · Beleg gespeichert",

  sealStart: "起",
  startHeading: "Neu bei Codewhale? Vier Schritte von Anfang bis Ende.",
  startLede:
    "Installieren → erste schlüssellose Sitzung → Provider verbinden → fleet einrichten. Begriffe werden auf der Vokabularseite definiert.",
  startGuideLink: "Leitfaden für die ersten Schritte lesen →",
  startVocabularyLink: "Produktvokabular ansehen →",

  sealBoundaries: "界",
  boundariesHeadingA: "Dein Modell.",
  boundariesHeadingB: "Deine Grenzen.",
  boundariesBody:
    "Wähle Modell, Arbeitsmodus und Permission-Posture explizit. Unbekannte Kosten bleiben als unbekannt deklariert, und Preview-Oberflächen bleiben als solche gekennzeichnet.",
  hostedGatewayLocal: "Gehostete, Gateway- und lokale Modelle",
  planActOperateDesc: "Von der Nur-Lese-Planung bis zum autonomen Operieren",
  askAutoReviewDesc: "Permission-Posture für die Arbeit wählen",
  tuiExecWebDesc: "Interaktive und Headless-Runtime-Oberflächen",

  sealSurfaces: "面",
  surfacesHeading: "Nutze die Runtime dort, wo die Arbeit passiert.",
  surfaces: [
    ["TUI", "Interaktive Arbeit im Terminal"],
    ["codewhale exec", "Skripte und CI"],
    ["Web-Client", "Browser-Client, nur Loopback"],
    ["Runtime API + MCP", "Lokale Integrationen"],
    ["fleet", "Dauerhafte Multi-Agenten-Arbeit"],
  ],
  runtimeLink: "Runtime-Oberflächen und Stabilitätshinweise ansehen →",

  installBandHeading: "Starte mit einem einzigen Befehl.",
  binaries: "Binärdateien",
  chinaMirrors: "China-Mirrors",
  installGuideLink: "Installationsleitfaden lesen →",

  sealCommunity: "众",
  communityHeading: "Öffentlich gebaut",
  communityBody:
    "MIT-lizenziert und geprägt von Beitragenden aus Runtimes, Providern, Plattformen, Dokumentation und Tests.",
  communityLinksAria: "Community-Links",
  contribute: "Mitwirken",
};
