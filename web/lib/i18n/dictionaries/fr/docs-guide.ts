import type { DocsGuideDict } from "../types";

/**
 * French dictionary for the docs "Getting started" page. Nominal French
 * typography for a Latin-script locale — the reference body class is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Premiers pas · Documentation Codewhale",
  metaDescription:
    "Le parcours complet de l’installation à votre Fleet idéale : installation, première session sans clé, connexion d’un fournisseur et configuration de la Fleet.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Premiers pas",
  overviewLead:
    "Quatre étapes d’une commande d’installation à une Fleet prête pour votre travail. Chaque étape ne décrit que ce que le candidat actuel fait réellement ; tout ce qui n’est pas publié ou enregistré est étiqueté comme tel.",
  sessionTitle: "Regarder une vraie session",
  sessionLead:
    "Ci-dessous, l’emplacement réservé au média de session réelle. Il est volontairement en attente : tant que l’enregistrement dogfood du candidat v0.9.2 n’existe pas, ce site n’affiche ni séquence fictive ni image de substitution.",
  nextTitle: "Et ensuite",
  sourceNote:
    "Documents sources : docs/GUIDE.md, docs/KEYBINDINGS.md · Le texte des étapes vit dans web/lib/content/getting-started.ts ; mettez à jour docs-map.ts en cas de modification.",
};
