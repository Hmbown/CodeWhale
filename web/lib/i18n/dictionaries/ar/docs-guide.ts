import type { DocsGuideDict } from "../types";

/**
 * Arabic dictionary for the docs "Getting started" page. Arabic script
 * needs roomier leading than the Latin reference — loose, short of the
 * CJK treatment.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "البداية · توثيق Codewhale",
  metaDescription:
    "المسار الكامل من التثبيت إلى أسطولك المثالي: التثبيت، وأول جلسة بلا مفاتيح، وربط مزوّد، وإعداد الأسطول.",
  bodyClassName: "text-ink-soft leading-loose",
  overviewTitle: "البداية",
  overviewLead:
    "أربع خطوات من أمر تثبيت واحد إلى أسطول جاهز لعملك. كل خطوة تذكر فقط ما يفعله المرشّح الحالي فعلًا؛ وكل ما هو غير منشور أو غير مسجّل موسوم بذلك.",
  sessionTitle: "شاهد جلسة حقيقية",
  sessionLead:
    "أدناه موضع وسائط الجلسة الحقيقية. هو عمدًا في حالة الانتظار: إلى أن توجد تسجيلة dogfood لمرشّح v0.9.2، لا يعرض هذا الموقع أي بديل أو مشاهد مفبركة.",
  nextTitle: "إلى أين بعد ذلك",
  sourceNote:
    "المستندات المصدر: docs/GUIDE.md، docs/KEYBINDINGS.md · نصوص الخطوات في web/lib/content/getting-started.ts؛ حدّث docs-map.ts عند أي تغيير.",
};
