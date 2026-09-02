import type { ChromeDict } from "../types";

/**
 * Turkish chrome dictionary.
 *
 * Güncel İngilizce yönü yansıtan özgün yeniden yazım — «istediğin model,
 * senin makinen»; emekli «local-first» konumlandırması yok. Türkçe
 * geliştirici topluluğunun alışıldığı samimi «sen» dili.
 *
 * Modlar ve izin duruşları (posture) literal kalır (Plan / Work / Operate,
 * Ask / Auto-Review / Full Access); `Runtime`, `fleet` ve `TUI` ürün adı
 * olarak kalır, «makbuz» receipt karşılığıdır.
 *
 * İkincil gezinme etiketleri Türkçe birinciyi kısa bir İngilizce eşle
 * eşler — Han çifti İngilizce baskının kendi editoryal aracıdır.
 */
export const chrome: ChromeDict = {
  navDocs: "Belgeler",
  navStart: "Başlangıç",
  navInstall: "Kurulum",
  navFaq: "SSS",
  navCommunity: "Topluluk",
  navContribute: "Katkı",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Ana içeriğe geç",

  navPrimaryAria: "Ana gezinme",
  navHomeAria: "Codewhale ana sayfası",

  installCta: "Kur →",

  authSignIn: "Giriş yap",
  authRegister: "Kayıt ol",
  authGroupAria: "Hesap",

  wordmarkSeal: "深",
  wordmarkTag: "istediğin model, senin makinen",

  issueLabel: "{date} sayısı",
  dateLocale: "tr-TR",

  starsAria: "GitHub yıldızları",
  githubFallback: "GitHub",

  tickerLiveLabel: "Canlı",
  tickerLiveTag: "LIVE",
  tickerMerged: "birleştirildi",
  tickerOpened: "açıldı",
  tickerClosed: "kapatıldı",
  tickerReleased: "yayımlandı",
  tickerFirstContribution: "ilk katkı",
  tickerBy: "{handle} tarafından",
  tickerAria: "Depodaki son etkinlik",

  traceLabel: "muhakeme izi",
  traceTabsAria: "Oturum kesitleri",

  menuOpen: "Menüyü aç",
  menuClose: "Menüyü kapat",

  themeAuto: "otomatik",
  themeLight: "açık",
  themeDark: "koyu",
  themeAria: "Belge teması: {mode} (geçiş için tıkla)",
  themeTitle: "Belge teması · otomatik / açık / koyu",

  footerTagline:
    "Derinlere Codewhale dalar — senin dalmana gerek yok: açık kaynak çalışma zamanı için belgeler, kaynak ve topluluk.",
  footerProduct: "Ürün",
  footerProject: "Proje",
  footerDocs: "Belgeler",
  footerGuide: "Başlangıç",
  footerInstall: "Kurulum",
  footerModels: "Modeller",
  footerRuntime: "Runtime",
  footerFaq: "SSS",
  footerIssues: "Issues",
  footerContribute: "Katkı",
  footerLicense: "MIT lisansı",
  footerPricing: "Fiyatlandırma",
  footerTerms: "Hizmet şartları",
  footerPrivacy: "Gizlilik",
  footerChangelog: "Değişiklik günlüğü",
  footerCanonicalSource: "Yetkili kaynak: ",
  footerReleases: " · Sürümler: ",
  footerReleasesLink: "GitHub sürümleri",
  footerSecurity: "Güvenlik",

  switcherLabel: "Dil",
  switcherSwitchTo: "{label} diline geç",
  partialBadge: "(kısmi)",
};
