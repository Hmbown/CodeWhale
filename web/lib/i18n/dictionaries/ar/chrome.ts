import type { ChromeDict } from "../types";

/**
 * Arabic chrome dictionary — فصحى معاصرة، بترجمة صيغة RTL.
 *
 * إعادة صياغة أصلية باتجاه الإنجليزية الحالي — «أي نموذج، على جهازك»،
 * لا تموضع «local-first» المتقاعد.
 *
 * أسماء الأوامر والمنتجات تبقى كما هي: Codewhale و GitHub و Issues
 * و `Runtime` و `fleet` و TUI. الأسهم تشير إلى الأمام في سياق RTL (←).
 *
 * التسميات الثانوية للتنقل تقارن التسمية العربية بنظيرة إنجليزية
 * قصيرة — ثنائية الهان هي أداة تحريرية خاصة بالنسخة الإنجليزية.
 */
export const chrome: ChromeDict = {
  navDocs: "التوثيق",
  navStart: "البداية",
  navInstall: "التثبيت",
  navFaq: "الأسئلة الشائعة",
  navCommunity: "المجتمع",
  navContribute: "المساهمة",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "الانتقال إلى المحتوى الرئيسي",

  navPrimaryAria: "التنقل الرئيسي",
  navHomeAria: "الصفحة الرئيسية لـ Codewhale",

  installCta: "ثبّت ←",

  authSignIn: "تسجيل الدخول",
  authRegister: "إنشاء حساب",
  authGroupAria: "الحساب",

  wordmarkSeal: "深",
  wordmarkTag: "أي نموذج، على جهازك",

  issueLabel: "عدد {date}",
  dateLocale: "ar",

  starsAria: "نجوم GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "مباشر",
  tickerLiveTag: "LIVE",
  tickerMerged: "دُمج",
  tickerOpened: "فُتح",
  tickerClosed: "أُغلق",
  tickerReleased: "أُصدر",
  tickerFirstContribution: "أول مساهمة",
  tickerBy: "بواسطة {handle}",
  tickerAria: "آخر نشاط في المستودع",

  traceLabel: "أثر الاستدلال",
  traceTabsAria: "مقتطفات الجلسات",

  menuOpen: "افتح القائمة",
  menuClose: "أغلق القائمة",

  themeAuto: "تلقائي",
  themeLight: "فاتح",
  themeDark: "داكن",
  themeAria: "سمة التوثيق: {mode} (انقر للتبديل)",
  themeTitle: "سمة التوثيق · تلقائي / فاتح / داكن",

  footerTagline:
    "Codewhale يغوص في الأعماق بدلًا منك — توثيق ومصدر ومجتمع لِ Runtime مفتوح المصدر.",
  footerProduct: "المنتج",
  footerProject: "المشروع",
  footerDocs: "التوثيق",
  footerGuide: "البداية",
  footerInstall: "التثبيت",
  footerModels: "النماذج",
  footerRuntime: "Runtime",
  footerFaq: "الأسئلة الشائعة",
  footerIssues: "Issues",
  footerContribute: "المساهمة",
  footerLicense: "رخصة MIT",
  footerPricing: "الأسعار",
  footerTerms: "شروط الخدمة",
  footerPrivacy: "الخصوصية",
  footerChangelog: "سجل التغييرات",
  footerCanonicalSource: "المصدر القانوني: ",
  footerReleases: " · الإصدارات: ",
  footerReleasesLink: "إصدارات GitHub",
  footerSecurity: "الأمن",

  switcherLabel: "اللغة",
  switcherSwitchTo: "التبديل إلى {label}",
  partialBadge: "(جزئي)",
};
