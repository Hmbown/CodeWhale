import type { DocsGuideDict } from "../types";

/**
 * French dictionary for the docs "Getting started" page. Nominal French
 * typography for a Latin-script locale — the reference body class is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Premiers pas · Documentation Codewhale",
  metaDescription:
    "Le parcours complet de l’installation à votre fleet idéale : installation, première session sans clé, connexion d’un fournisseur et configuration de la fleet.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Premiers pas",
  overviewLead:
    "Quatre étapes d’une commande d’installation à une fleet prête pour votre travail.",
  sessionTitle: "Regarder une vraie session",
  sessionLead:
    "L’enregistrement d’une vraie session viendra ici. Il n’existe pas encore, donc rien n’est affiché.",
  nextTitle: "Et ensuite",
  sourceNote:
    "Documents sources : docs/GUIDE.md, docs/KEYBINDINGS.md · Le texte des étapes vit dans web/lib/content/getting-started.ts ; mettez à jour docs-map.ts en cas de modification.",
};
