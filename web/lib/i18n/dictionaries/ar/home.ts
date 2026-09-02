import type { HomeDict } from "../types";

/**
 * Arabic home dictionary — صفحة الهبوط «جريدة-محيط».
 *
 * إعادة صياغة أصلية باتجاه الإنجليزية الحالي: أحضر نموذجك، وكل شيء
 * يجري على جهاك. مفردات المنتج تبقى حرفية كما في حزمة TUI:
 * Plan / Work / Operate، Ask / Auto-Review / Full Access، Codewhale،
 * TUI، `codewhale exec`، Runtime API + MCP، fleet، Node 18+، Rust، MIT.
 *
 * أختام الأقسام (法، 行، …) محارف مشتركة مع النسخة الإنجليزية —
 * علامات لا نثرًا. الأسهم تشير إلى الأمام في سياق RTL (←).
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — يغوص في الأعماق بدلًا منك.",
  metaDescription:
    "Codewhale يغوص في الأعماق بدلًا منك — وكيل برمجة مفتوح المصدر للطرفية. أحضر نموذجك. يعمل على جهازك. بـ Rust، ورخصة MIT.",

  kicker: "مفتوح المصدر · أحضر نموذجك · يعمل في طرفيتك",
  heroTitleA: "Codewhale يغوص في الأعماق،",
  heroTitleB: "حتى لا تضطر أنت إلى ذلك.",
  heroIntro:
    "{brand} وكيل برمجة مفتوح المصدر يعمل في طرفيتك. أعطه نموذجًا ومهمة — يقرأ شيفرتك، ويحرّر الملفات، ويشغّل فحوصه بنفسه، ويتوقف عندما تنتهي المهمة أو عندما يحتاج إليك. أحضر أي نموذج، أو امزجها: ثبّت نموذجًا مختلفًا لكل دور.",
  install: "ثبّت",
  docs: "التوثيق",
  copy: "انسخ",
  copied: "نُسخ ✓",

  installEyebrow: "تثبيت بسطر واحد",
  installRequirement: "يتطلب Node 18+ — لا حاجة إلى سلسلة أدوات Rust",
  installOtherWays: "طرق أخرى ←",

  latestRelease: "أحدث إصدار {tag}",
  releaseUnavailable: "حالة الإصدار غير متاحة",
  currentSource: "المصدر",
  sourceCandidate: "غير منشور",
  providerRoutes: "{count} مزوّد",
  publishedRelease: "منشور",
  figcaptionSourceCandidate: "غير منشور",

  shotSession: "الجلسة الحالية",
  screenshotAlt:
    "جلسة Codewhale الطرفية الحالية: وضع Operate، والحوت، والمحرّر، والتذييل",
  figcaption: "جلسة Codewhale الحالية · وضع Operate · وضعية أذونات Ask",

  proofHeading: "صدفة طرفية تحت الماء. أي نموذج. على جهازك.",
  proofBody:
    "أحضر النموذج الذي تستخدمه أصلًا — مستضاف، أو عبر بوابة، أو محلي. Plan / Work / Operate ووضعيات الأذونات الصريحة تُبقي الغوصة تحت سيطرتك.",

  sealDecides: "法",
  decidesEyebrow: "شاهد كيف يقرر",
  decidesHeading: "قواعد تراها في الأثر",
  decidesLede:
    "مقتطفات من جلسات حقيقية — تسلسل قواعد المشروع يظهر في استدلال النموذج، لا مجرد وعد على صفحة هبوط.",

  sealWorkflow: "行",
  workflowHeading: "من المهمة إلى تغيير متحقَّق منه.",
  workflow: [
    ["الفحص", "قراءة المستودع وتعليماته والمهمة."],
    ["التنفيذ", "تحرير الملفات ضمن حدود موافقة صريحة."],
    ["التحقق", "تشغيل الفحوص ومعاينة النتيجة."],
    ["التقرير", "ترك إيصال موجز وباقٍ."],
  ],
  receiptAria: "مثال إيصال عمل",
  receiptInspect: "المستودع والتعليمات",
  receiptAct: "التحرير عبر وضعية الأذونات المختارة",
  receiptReport: "الفحوص ناجحة · الإيصال محفوظ",

  sealStart: "起",
  startHeading: "جديد على Codewhale؟ أربع خطوات من البداية إلى النهاية.",
  startLede:
    "ثبّت ← أول جلسة بلا مفاتيح ← اربط مزوّدًا ← جهّز أسطولك. المصطلحات معرّفة في صفحة المفردات.",
  startGuideLink: "اقرأ دليل البداية ←",
  startVocabularyLink: "اطّلع على مفردات المنتج ←",

  sealBoundaries: "界",
  boundariesHeadingA: "نموذجك.",
  boundariesHeadingB: "حدودك.",
  boundariesBody:
    "اختر النموذج ووضع العمل ووضعية الأذونات صراحةً. التكلفة المجهولة تبقى مصرّحًا بها كمجهولة، والواجهات التجريبية تبقى موسومة بذلك.",
  hostedGatewayLocal: "نماذج مستضافة وعبر بوابات ومحلية",
  planActOperateDesc: "من التخطيط للقراءة فقط إلى التشغيل الذاتي",
  askAutoReviewDesc: "اختر وضعية الأذونات للعمل",
  tuiExecWebDesc: "واجهات Runtime تفاعلية وبلا واجهة",

  sealSurfaces: "面",
  surfacesHeading: "استخدم الـ Runtime حيث يجري العمل.",
  surfaces: [
    ["TUI", "عمل تفاعلي في الطرفية"],
    ["codewhale exec", "سكربتات وCI"],
    ["عميل الويب", "عميل متصفح محصور في loopback"],
    ["Runtime API + MCP", "تكاملات محلية"],
    ["fleet", "عمل متعدد الوكلاء دائم"],
  ],
  runtimeLink: "اطّلع على واجهات الـ Runtime وملاحظات الاستقرار ←",

  installBandHeading: "ابدأ بأمر واحد.",
  binaries: "الملفات الثنائية",
  chinaMirrors: "مرايا في الصين",
  installGuideLink: "اقرأ دليل التثبيت ←",

  sealCommunity: "众",
  communityHeading: "يُبنى علنًا",
  communityBody:
    "برخصة MIT، وبتشكيل من المساهمين عبر الـ Runtimeات والمزودين والمنصات والتوثيق والاختبارات.",
  communityLinksAria: "روابط المجتمع",
  contribute: "ساهم",
};
