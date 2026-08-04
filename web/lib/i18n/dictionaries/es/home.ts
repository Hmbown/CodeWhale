import type { HomeDict } from "../types";

/**
 * Spanish home dictionary — neutral (pan-Hispanic) Spanish, informal `tú`.
 *
 * The hero keeps the English slogan's structure: the whale is the subject
 * that dives ("Se sumerge …"), so the second line ("para que tú no tengas
 * que hacerlo") reads as the contrast it is instead of telling the reader
 * to dive and then not to.
 *
 * Fixed vocabulary, matching crates/tui/locales/es-419.json: Plan / Act /
 * Operate, Ask / Auto-Review / Full Access, "postura de permisos",
 * "recibo", `Runtime`, `Fleet`, `Workflow`, "compositor", "pie de página",
 * "alojado" for hosted. Commands, package names, and surface names stay
 * literal.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — Se sumerge en las profundidades para que tú no tengas que hacerlo.",
  metaDescription:
    "Codewhale se sumerge en las profundidades para que tú no tengas que hacerlo: un agente de terminal que pone la fuerza de los LLM al alcance de cualquier persona para construir cosas. Se ejecuta en tu máquina. Rust, licencia MIT.",

  kicker: "Código abierto · Cualquier modelo · Se ejecuta en tu terminal",
  heroTitleA: "Se sumerge en las profundidades",
  heroTitleB: "para que tú no tengas que hacerlo.",
  heroIntro:
    "{brand} pone la fuerza de los LLM al alcance de cualquier persona para construir cosas. En tu terminal lee el repositorio, edita archivos, ejecuta comprobaciones y deja un recibo — sin dar por sentado que ya sabes programar. Se ejecuta en tu máquina; el modelo es un componente que eliges tú, no el producto.",
  install: "Instalar",
  docs: "Documentación",
  copy: "Copiar",
  copied: "Copiado ✓",

  installEyebrow: "instalación en una línea",
  installRequirement: "requiere Node 18+ — no hace falta Rust",
  installOtherWays: "otras formas →",

  latestRelease: "Último lanzamiento {tag}",
  releaseUnavailable: "Estado del lanzamiento no disponible",
  currentSource: "Fuente actual",
  sourceCandidate: "Fuente candidata",
  providerRoutes: "{count} rutas de proveedor",
  publishedRelease: "lanzamiento publicado",
  figcaptionSourceCandidate: "fuente candidata",

  shotSession: "Sesión actual",
  screenshotAlt:
    "Sesión de terminal actual de Codewhale con el modo Operate, la ballena, el compositor y el pie de página",
  figcaption: "Sesión actual de Codewhale · modo Operate · postura de permisos Ask",

  proofHeading: "Un shell de terminal submarino. Neutral ante el modelo. Local primero.",
  proofBody:
    "Trae el modelo alojado, de gateway o local que ya usas. Codewhale se ejecuta en tu máquina y trata el modelo como un componente que eliges, no como el producto. Plan / Act / Operate y las posturas de permisos explícitas mantienen la inmersión bajo tu control.",

  sealDecides: "法",
  decidesEyebrow: "Mira cómo decide",
  decidesHeading: "Reglas que puedes ver en la traza",
  decidesLede:
    "Extractos fieles de una sesión real: la jerarquía de reglas del proyecto se observa en el razonamiento del modelo, no es una afirmación en una página de inicio.",

  sealWorkflow: "行",
  workflowHeading: "De la tarea al cambio verificado.",
  workflow: [
    ["Inspeccionar", "Lee el repositorio, sus instrucciones y la tarea."],
    ["Actuar", "Edita archivos dentro de límites de aprobación explícitos."],
    ["Verificar", "Ejecuta las comprobaciones e inspecciona el resultado."],
    ["Reportar", "Deja un recibo conciso y duradero."],
  ],
  receiptAria: "Ejemplo de recibo de trabajo",
  receiptInspect: "repositorio e instrucciones",
  receiptAct: "editar según la postura de permisos elegida",
  receiptReport: "comprobaciones superadas · recibo guardado",

  sealStart: "起",
  startHeading: "¿Nuevo en Codewhale? Cuatro pasos de principio a fin.",
  startLede:
    "Instalar → una primera sesión sin claves → conexión con el proveedor → un primer Workflow de Fleet. Los términos se definen en la página de vocabulario.",
  startGuideLink: "Leer la guía de primeros pasos →",
  startVocabularyLink: "Ver el vocabulario del producto →",

  sealBoundaries: "界",
  boundariesHeadingA: "Tu modelo.",
  boundariesHeadingB: "Tus límites.",
  boundariesBody:
    "Elige explícitamente el modelo, el modo de trabajo y la postura de permisos. El costo desconocido se declara desconocido, y las interfaces en vista previa se marcan como tales.",
  hostedGatewayLocal: "Modelos alojados, de gateway y locales",
  planActOperateDesc: "De la planificación de solo lectura a la operación autónoma",
  askAutoReviewDesc: "Elige la postura de permisos para el trabajo",
  tuiExecWebDesc: "Interfaces de runtime interactivas y headless",

  sealSurfaces: "面",
  surfacesHeading: "Usa el runtime donde ocurre el trabajo.",
  surfaces: [
    ["TUI", "Trabajo interactivo en la terminal"],
    ["codewhale exec", "Scripts y CI"],
    ["Cliente web", "Cliente de navegador, solo loopback"],
    ["Runtime API + MCP", "Integraciones locales"],
    ["Fleet", "Trabajo multiagente duradero"],
  ],
  runtimeLink: "Ver las interfaces de runtime y las notas de estabilidad →",

  installBandHeading: "Empieza con un solo comando.",
  binaries: "Binarios",
  chinaMirrors: "Espejos en China",
  installGuideLink: "Leer la guía de instalación →",

  sealCommunity: "众",
  communityHeading: "Construido en público",
  communityBody:
    "Con licencia MIT y moldeado por colaboradores en runtimes, proveedores, plataformas, documentación y pruebas.",
  communityLinksAria: "Enlaces de la comunidad",
  contribute: "Contribuir",
};
