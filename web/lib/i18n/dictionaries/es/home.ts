import type { HomeDict } from "../types";

/**
 * Spanish home dictionary — native rewrite in neutral (pan-Hispanic)
 * Spanish, informal `tú`, mirroring the current English direction:
 * bring-your-own-model, runs on your machine, no trace of the old
 * positioning.
 *
 * The hero keeps the English slogan's structure: the whale is the subject
 * that dives ("Codewhale se sumerge …"), so the second line ("para que tú
 * no tengas que hacerlo") reads as the contrast it is instead of telling
 * the reader to dive and then not to.
 *
 * Fixed vocabulary, matching crates/tui/locales/es-419.json: Plan / Act /
 * Operate, Ask / Auto-Review / Full Access, "postura de permisos",
 * "recibo", `Runtime`, `fleet`, `Workflow`, "compositor", "pie de página",
 * "alojado" for hosted. Commands, package names, and surface names stay
 * literal.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — se sumerge en las profundidades para que tú no tengas que hacerlo.",
  metaDescription:
    "Codewhale se sumerge en las profundidades para que tú no tengas que hacerlo: un agente de programación de código abierto para la terminal. Trae tu propio modelo. Se ejecuta en tu máquina. Rust, MIT.",

  kicker: "Código abierto · Trae tu propio modelo · Se ejecuta en tu terminal",
  heroTitleA: "Codewhale se sumerge en las profundidades",
  heroTitleB: "para que tú no tengas que hacerlo.",
  heroIntro:
    "{brand} es un agente de programación de código abierto para tu terminal. Dale un modelo y una tarea: lee tu código, edita archivos, ejecuta sus propias comprobaciones y se detiene cuando el trabajo está hecho o te necesita. Trae cualquier modelo, o combínalos: asigna un modelo distinto a cada rol.",
  install: "Instalar",
  docs: "Documentación",
  copy: "Copiar",
  copied: "Copiado ✓",

  installEyebrow: "instalación en una línea",
  installRequirement: "requiere Node 18+ — no hace falta Rust",
  installOtherWays: "otras formas →",

  latestRelease: "Último lanzamiento {tag}",
  releaseUnavailable: "Estado del lanzamiento no disponible",
  currentSource: "Fuente",
  sourceCandidate: "Sin publicar",
  providerRoutes: "{count} proveedores",
  publishedRelease: "publicado",
  figcaptionSourceCandidate: "sin publicar",

  shotSession: "Sesión actual",
  screenshotAlt:
    "Sesión de terminal actual de Codewhale con el modo Operate, la ballena, el compositor y el pie de página",
  figcaption: "Sesión actual de Codewhale · modo Operate · postura de permisos Ask",

  proofHeading: "Un shell de terminal submarino. Cualquier modelo. En tu máquina.",
  proofBody:
    "Trae el modelo que ya usas: alojado, de gateway o local. Plan / Work / Operate y las posturas de permisos explícitas mantienen la inmersión bajo tu control.",

  sealDecides: "法",
  decidesEyebrow: "Mira cómo decide",
  decidesHeading: "Reglas que puedes ver en la traza",
  decidesLede:
    "Extractos de sesiones reales: la jerarquía de reglas del proyecto se observa en el razonamiento del modelo, no es solo una afirmación de esta página.",

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
    "Instalar → primera sesión sin claves → conectar un proveedor → primer Workflow de fleet. Los términos se definen en la página de vocabulario.",
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
    ["fleet", "Trabajo multiagente duradero"],
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
