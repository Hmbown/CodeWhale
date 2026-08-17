import type { DocsGuideDict } from "../types";

/**
 * Turkish dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Başlangıç · Codewhale Belgeleri",
  metaDescription:
    "Kurulumdan ideal Fleet'ine kadar tam yol: kurulum, anahtarsız ilk oturum, sağlayıcı bağlantısı ve Fleet kurulumu.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Başlangıç",
  overviewLead:
    "Tek bir kurulum komutundan işine hazır bir Fleet'e dört adım. Her adım yalnızca geçerli adayın gerçekten yaptığını söyler; yayımlanmamış ya da kaydedilmemiş her şey böyle etiketlenir.",
  sessionTitle: "Gerçek bir oturum izle",
  sessionLead:
    "Aşağıda gerçek oturum medyası için ayrılmış yer var. Bilerek bekleme durumunda: v0.9.2 adayının dogfood kaydı var olana dek bu site yer tutucu ya da sahne görüntüsü göstermez.",
  nextTitle: "Sıradaki adım",
  sourceNote:
    "Kaynak belgeler: docs/GUIDE.md, docs/KEYBINDINGS.md · Adım metinleri web/lib/content/getting-started.ts içinde; değişiklikte docs-map.ts'i güncelle.",
};
