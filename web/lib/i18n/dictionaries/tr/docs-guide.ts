import type { DocsGuideDict } from "../types";

/**
 * Turkish dictionary for the docs "Getting started" page. Latin script —
 * the reference body typography is kept.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Başlangıç · Codewhale Belgeleri",
  metaDescription:
    "Kurulumdan ideal fleet'ine kadar tam yol: kurulum, anahtarsız ilk oturum, sağlayıcı bağlantısı ve fleet kurulumu.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Başlangıç",
  overviewLead:
    "Tek bir kurulum komutundan işine hazır bir fleet'e dört adım.",
  sessionTitle: "Gerçek bir oturum izle",
  sessionLead:
    "Buraya gerçek bir oturumun kaydı gelecek. Henüz kayıt yok, bu yüzden hiçbir şey gösterilmiyor.",
  nextTitle: "Sıradaki adım",
  sourceNote:
    "Kaynak belgeler: docs/GUIDE.md, docs/KEYBINDINGS.md · Adım metinleri web/lib/content/getting-started.ts içinde; değişiklikte docs-map.ts'i güncelle.",
};
