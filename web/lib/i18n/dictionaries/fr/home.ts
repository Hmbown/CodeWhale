import type { HomeDict } from "../types";

/**
 * French home dictionary — la page d’accueil « journal-océan ».
 *
 * Réécriture native dans la direction actuelle de l’anglais : apportez
 * votre modèle, tout se passe sur votre machine. Le vocabulaire produit
 * reste littéral et aligné sur le pack TUI : Plan / Work / Operate,
 * Ask / Auto-Review / Full Access, Codewhale, TUI, `codewhale exec`,
 * Runtime API + MCP, fleet, Node 18+, Rust, MIT.
 *
 * Les sceaux de section (法, 行, …) sont des glyphes partagés avec
 * l’édition anglaise — des marques, pas de la prose.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — plonge dans les profondeurs à votre place.",
  metaDescription:
    "Codewhale plonge dans les profondeurs à votre place — un agent de codage open source pour le terminal. Apportez votre modèle. Sur votre machine. En Rust, sous licence MIT.",

  kicker: "Open source · Apportez votre modèle · Dans votre terminal",
  heroTitleA: "Codewhale plonge dans les profondeurs",
  heroTitleB: "pour que vous n’ayez pas à le faire.",
  heroIntro:
    "{brand} est un agent de codage open source pour votre terminal. Donnez-lui un modèle et une tâche : il lit votre code, modifie les fichiers, lance ses propres vérifications et s’arrête quand le travail est fini ou quand il a besoin de vous. Apportez n’importe quel modèle, ou mélangez-les : épinglez un modèle différent pour chaque rôle.",
  install: "Installer",
  docs: "Documentation",
  copy: "Copier",
  copied: "Copié ✓",

  installEyebrow: "installation en une ligne",
  installRequirement: "nécessite Node 18+ — aucune chaîne d’outils Rust",
  installOtherWays: "autres méthodes →",

  latestRelease: "Dernière version {tag}",
  releaseUnavailable: "État des versions indisponible",
  currentSource: "Source",
  sourceCandidate: "Non publiée",
  providerRoutes: "{count} fournisseurs",
  publishedRelease: "publiée",
  figcaptionSourceCandidate: "non publiée",

  shotSession: "Session en cours",
  screenshotAlt:
    "Session Codewhale actuelle dans le terminal : mode Operate, la baleine, le composeur et le pied de page",
  figcaption: "Session Codewhale actuelle · mode Operate · posture de permissions Ask",

  proofHeading: "Un shell sous-marin. N’importe quel modèle. Sur votre machine.",
  proofBody:
    "Apportez le modèle que vous utilisez déjà — hébergé, passerelle ou local. Plan / Work / Operate et des postures de permissions explicites gardent la plongée sous votre contrôle.",

  sealDecides: "法",
  decidesEyebrow: "Voir comment il décide",
  decidesHeading: "Des règles visibles dans la trace",
  decidesLede:
    "Des extraits de sessions réelles : la hiérarchie des règles du projet apparaît dans le raisonnement du modèle, ce n’est pas une simple promesse de page d’accueil.",

  sealWorkflow: "行",
  workflowHeading: "De la tâche au changement vérifié.",
  workflow: [
    ["Inspecter", "Lire le dépôt, ses instructions et la tâche."],
    ["Agir", "Modifier les fichiers dans des limites d’approbation explicites."],
    ["Vérifier", "Lancer les vérifications et examiner le résultat."],
    ["Rendre compte", "Laisser un reçu concis et durable."],
  ],
  receiptAria: "Exemple de reçu de travail",
  receiptInspect: "dépôt et instructions",
  receiptAct: "modification selon la posture de permissions choisie",
  receiptReport: "vérifications passées · reçu enregistré",

  sealStart: "起",
  startHeading: "Nouveau sur Codewhale ? Quatre étapes de bout en bout.",
  startLede:
    "Installer → première session sans clé → connecter un fournisseur → configurer votre fleet. Les termes sont définis sur la page de vocabulaire.",
  startGuideLink: "Lire le guide de démarrage →",
  startVocabularyLink: "Voir le vocabulaire du produit →",

  sealBoundaries: "界",
  boundariesHeadingA: "Votre modèle.",
  boundariesHeadingB: "Vos limites.",
  boundariesBody:
    "Choisissez explicitement le modèle, le mode de travail et la posture de permissions. Un coût inconnu reste déclaré inconnu, et les surfaces en préversion restent étiquetées comme telles.",
  hostedGatewayLocal: "Modèles hébergés, passerelle et locaux",
  planActOperateDesc: "De la planification en lecture seule à l’opération autonome",
  askAutoReviewDesc: "Choisir la posture de permissions pour le travail",
  tuiExecWebDesc: "Surfaces de runtime interactives et sans interface",

  sealSurfaces: "面",
  surfacesHeading: "Utilisez le runtime là où se fait le travail.",
  surfaces: [
    ["TUI", "Travail interactif dans le terminal"],
    ["codewhale exec", "Scripts et CI"],
    ["Client web", "Client navigateur en boucle locale uniquement"],
    ["Runtime API + MCP", "Intégrations locales"],
    ["fleet", "Travail multi-agents durable"],
  ],
  runtimeLink: "Voir les surfaces du runtime et les notes de stabilité →",

  installBandHeading: "Commencez avec une seule commande.",
  binaries: "Binaires",
  chinaMirrors: "Miroirs en Chine",
  installGuideLink: "Lire le guide d’installation →",

  sealCommunity: "众",
  communityHeading: "Construit en public",
  communityBody:
    "Sous licence MIT et façonné par des contributeurs sur les runtimes, les fournisseurs, les plateformes, la documentation et les tests.",
  communityLinksAria: "Liens de la communauté",
  contribute: "Contribuer",
};
