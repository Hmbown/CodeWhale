import type { HomeDict } from "../types";

/**
 * Italian home dictionary — la landing «giornale-oceano».
 *
 * Riscrittura nativa nella direzione inglese attuale: porta il tuo modello,
 * tutto avviene sulla tua macchina. Il vocabolario di prodotto resta
 * letterale come nel pack TUI: Plan / Work / Operate, Ask / Auto-Review /
 * Full Access, Codewhale, TUI, `codewhale exec`, Runtime API + MCP, fleet,
 * Node 18+, Rust, MIT.
 *
 * I sigilli di sezione (法, 行, …) sono glifi condivisi con l'edizione
 * inglese — marchi, non prosa.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — si tuffa negli abissi al posto tuo.",
  metaDescription:
    "Codewhale si tuffa negli abissi al posto tuo — un agente di coding open source per il terminale. Porta il tuo modello. Gira sulla tua macchina. Rust, MIT.",

  kicker: "Open source · Porta il tuo modello · Nel tuo terminale",
  heroTitleA: "Codewhale si tuffa negli abissi",
  heroTitleB: "così non devi farlo tu.",
  heroIntro:
    "{brand} è un agente di coding open source per il tuo terminale. Dagli un modello e un compito: legge il tuo codice, modifica i file, esegue i suoi controlli e si ferma quando il lavoro è finito o quando serve te. Porta qualsiasi modello, o mescolali: fissa un modello diverso per ciascun ruolo.",
  install: "Installa",
  docs: "Documentazione",
  copy: "Copia",
  copied: "Copiato ✓",

  installEyebrow: "installazione in una riga",
  installRequirement: "serve Node 18+ — nessuna toolchain Rust",
  installOtherWays: "altri modi →",

  latestRelease: "Ultima release {tag}",
  releaseUnavailable: "Stato delle release non disponibile",
  currentSource: "Sorgente",
  sourceCandidate: "Non rilasciata",
  providerRoutes: "{count} provider",
  publishedRelease: "rilasciata",
  figcaptionSourceCandidate: "non rilasciata",

  shotSession: "Sessione corrente",
  screenshotAlt:
    "Sessione corrente di Codewhale nel terminale: modalità Operate, la balena, il compositore e il piè di pagina",
  figcaption: "Sessione corrente di Codewhale · modalità Operate · postura di permessi Ask",

  proofHeading: "Una shell sottomarina. Qualsiasi modello. Sulla tua macchina.",
  proofBody:
    "Porta il modello che già usi — hosted, via gateway o locale. Plan / Work / Operate e le posture di permessi esplicite tengono l'immersione sotto il tuo controllo.",

  sealDecides: "法",
  decidesEyebrow: "Guarda come decide",
  decidesHeading: "Regole che vedi nella traccia",
  decidesLede:
    "Stralci di sessioni reali: la gerarchia di regole del progetto è visibile nel ragionamento del modello, non è solo la promessa di una landing.",

  sealWorkflow: "行",
  workflowHeading: "Dal compito alla modifica verificata.",
  workflow: [
    ["Ispezionare", "Leggere il repository, le sue istruzioni e il compito."],
    ["Agire", "Modificare i file entro confini di approvazione espliciti."],
    ["Verificare", "Eseguire i controlli ed esaminare il risultato."],
    ["Riferire", "Lasciare una ricevuta concisa e durevole."],
  ],
  receiptAria: "Esempio di ricevuta di lavoro",
  receiptInspect: "repository e istruzioni",
  receiptAct: "modifiche con la postura di permessi scelta",
  receiptReport: "controlli superati · ricevuta salvata",

  sealStart: "起",
  startHeading: "Nuovo su Codewhale? Quattro passi dall'inizio alla fine.",
  startLede:
    "Installare → prima sessione senza chiavi → collegare un provider → configurare la tua fleet. I termini sono definiti nella pagina del vocabolario.",
  startGuideLink: "Leggi la guida introduttiva →",
  startVocabularyLink: "Vedi il vocabolario del prodotto →",

  sealBoundaries: "界",
  boundariesHeadingA: "Il tuo modello.",
  boundariesHeadingB: "I tuoi confini.",
  boundariesBody:
    "Scegli esplicitamente modello, modalità di lavoro e postura di permessi. Un costo sconosciuto resta dichiarato sconosciuto, e le superfici in anteprima restano etichettate come tali.",
  hostedGatewayLocal: "Modelli hosted, via gateway e locali",
  planActOperateDesc: "Dalla pianificazione in sola lettura all'operazione autonoma",
  askAutoReviewDesc: "Scegli la postura di permessi per il lavoro",
  tuiExecWebDesc: "Superfici di runtime interattive e headless",

  sealSurfaces: "面",
  surfacesHeading: "Usa il runtime dove avviene il lavoro.",
  surfaces: [
    ["TUI", "Lavoro interattivo nel terminale"],
    ["codewhale exec", "Script e CI"],
    ["Client web", "Client browser, solo in loopback"],
    ["Runtime API + MCP", "Integrazioni locali"],
    ["fleet", "Lavoro multi-agente durevole"],
  ],
  runtimeLink: "Vedi le superfici del runtime e le note di stabilità →",

  installBandHeading: "Inizia con un solo comando.",
  binaries: "Binari",
  chinaMirrors: "Mirror in Cina",
  installGuideLink: "Leggi la guida d'installazione →",

  sealCommunity: "众",
  communityHeading: "Costruito in pubblico",
  communityBody:
    "Con licenza MIT e plasmato da contributor su runtime, provider, piattaforme, documentazione e test.",
  communityLinksAria: "Link della community",
  contribute: "Contribuisci",
};
