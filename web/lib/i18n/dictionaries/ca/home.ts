import type { HomeDict } from "../types";

/**
 * Catalan home dictionary — la pàgina d’inici «diari-oceà».
 *
 * Reescriptura nativa en la direcció anglesa actual: porta el teu model,
 * tot passa a la teva màquina. El vocabulari de producte es manté literal
 * i coincideix amb el pack TUI: Plan / Work / Operate, Ask / Auto-Review /
 * Full Access, Codewhale, TUI, `codewhale exec`, Runtime API + MCP, fleet,
 * Node 18+, Rust, MIT.
 *
 * Els segells de secció (法, 行, …) són glifs compartits amb l’edició
 * anglesa — marques, no prosa.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — s’immergeix a les profunditats per tu.",
  metaDescription:
    "Codewhale s’immergeix a les profunditats per tu — un agent de programació de codi obert per al terminal. Porta el teu model. A la teva màquina. En Rust, amb llicència MIT.",

  kicker: "Codi obert · Porta el teu model · Al teu terminal",
  heroTitleA: "Codewhale s’immergeix a les profunditats",
  heroTitleB: "perquè tu no ho hagis de fer.",
  heroIntro:
    "{brand} és un agent de programació de codi obert per al teu terminal. Dóna-li un model i una tasca: llegeix el teu codi, edita fitxers, executa les seves pròpies comprovacions i s’atura quan la feina és feta o quan et necessita. Porta el model que vulguis, o barreja’ls: fixa un model diferent per a cada rol.",
  install: "Instal·la",
  docs: "Documentació",
  copy: "Copia",
  copied: "Copiat ✓",

  installEyebrow: "instal·lació en una línia",
  installRequirement: "cal Node 18+ — sense cadena d’eines Rust",
  installOtherWays: "altres mètodes →",

  latestRelease: "Última versió {tag}",
  releaseUnavailable: "Estat de la versió no disponible",
  currentSource: "Font",
  sourceCandidate: "Sense publicar",
  providerRoutes: "{count} proveïdors",
  publishedRelease: "publicada",
  figcaptionSourceCandidate: "sense publicar",

  shotSession: "Sessió actual",
  screenshotAlt:
    "Sessió de terminal actual de Codewhale amb el mode Operate, la balena, el compositor i el peu de pàgina",
  figcaption: "Sessió actual de Codewhale · mode Operate · postura de permisos Ask",

  proofHeading: "Un intèrpret d’ordres submarí. Qualsevol model. A la teva màquina.",
  proofBody:
    "Porta el model que ja utilitzes — allotjat, via gateway o local. Plan / Work / Operate i les postures de permisos explícites mantenen la immersió sota el teu control.",

  sealDecides: "法",
  decidesEyebrow: "Mira com decideix",
  decidesHeading: "Regles que pots veure a la traça",
  decidesLede:
    "Extractes de sessions reals: la jerarquia de regles del projecte es veu al raonament del model, no és només una afirmació de la portada.",

  sealWorkflow: "行",
  workflowHeading: "De la tasca al canvi verificat.",
  workflow: [
    ["Inspeccionar", "Llegir el repositori, les seves instruccions i la tasca."],
    ["Actuar", "Editar fitxers dins de límits d’aprovació explícits."],
    ["Verificar", "Executar les comprovacions i inspeccionar el resultat."],
    ["Reportar", "Deixar un resguard concís i durable."],
  ],
  receiptAria: "Exemple de resguard de feina",
  receiptInspect: "repositori i instruccions",
  receiptAct: "edició segons la postura de permisos triada",
  receiptReport: "comprovacions superades · resguard desat",

  sealStart: "起",
  startHeading: "Nou a Codewhale? Quatre passos de principi a fi.",
  startLede:
    "Instal·lar → primera sessió sense claus → connectar un proveïdor → configurar la teva fleet. Els termes es defineixen a la pàgina de vocabulari.",
  startGuideLink: "Llegeix la guia d’inici →",
  startVocabularyLink: "Consulta el vocabulari del producte →",

  sealBoundaries: "界",
  boundariesHeadingA: "El teu model.",
  boundariesHeadingB: "Els teus límits.",
  boundariesBody:
    "Tria explícitament el model, el mode de treball i la postura de permisos. El cost desconegut es declara desconegut, i les superfícies en previsualització es marquen com a tals.",
  hostedGatewayLocal: "Models allotjats, de gateway i locals",
  planActOperateDesc: "De la planificació de només lectura a l’operació autònoma",
  askAutoReviewDesc: "Tria la postura de permisos per a la feina",
  tuiExecWebDesc: "Superfícies de runtime interactives i sense interfície",

  sealSurfaces: "面",
  surfacesHeading: "Fes servir el runtime on passa la feina.",
  surfaces: [
    ["TUI", "Treball interactiu al terminal"],
    ["codewhale exec", "Scripts i CI"],
    ["Client web", "Client de navegador, només loopback"],
    ["Runtime API + MCP", "Integracions locals"],
    ["fleet", "Feina multiagent durable"],
  ],
  runtimeLink: "Veure les superfícies del runtime i les notes d’estabilitat →",

  installBandHeading: "Comença amb una sola ordre.",
  binaries: "Binaris",
  chinaMirrors: "Mirrors a la Xina",
  installGuideLink: "Llegeix la guia d’instal·lació →",

  sealCommunity: "众",
  communityHeading: "Construït en públic",
  communityBody:
    "Amb llicència MIT i format per col·laboradors de runtimes, proveïdors, plataformes, documentació i tests.",
  communityLinksAria: "Enllaços de la comunitat",
  contribute: "Col·labora",
};
