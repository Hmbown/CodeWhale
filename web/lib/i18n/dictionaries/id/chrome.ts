import type { ChromeDict } from "../types";

/**
 * Indonesian chrome dictionary — native rewrite mirroring the current English
 * direction. The wordmark tag renders "any model, on your machine" natively;
 * the old positioning is gone.
 *
 * Terminology follows the TUI locale pack (`crates/tui/locales/id.json`):
 * "penyedia" (provider), "izin" / "postur izin" (permission posture),
 * "penalaran" (reasoning), "repositori", "tanda terima" (receipt). The mode
 * names (Plan / Work / Operate) and permission postures (Ask / Auto-Review /
 * Full Access) stay literal there and stay literal here.
 *
 * Secondary nav labels pair the Indonesian primary with a short English
 * companion — the masthead's bilingual device, minus the Han seals, which
 * belong to the English edition.
 */
export const chrome: ChromeDict = {
  navDocs: "Dokumentasi",
  navStart: "Mulai",
  navInstall: "Instal",
  navFaq: "Tanya jawab",
  navCommunity: "Komunitas",
  navContribute: "Kontribusi",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Lewati ke konten utama",


  navPrimaryAria: "Navigasi utama",
  navHomeAria: "Beranda Codewhale",

  installCta: "Instal →",

  authSignIn: "Masuk",
  authRegister: "Daftar",
  authGroupAria: "Akun",

  wordmarkSeal: "深",
  wordmarkTag: "model apa pun, di mesin Anda",

  issueLabel: "Edisi {date}",
  dateLocale: "id-ID",

  starsAria: "Bintang GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "Langsung",
  tickerLiveTag: "LIVE",
  tickerMerged: "digabungkan",
  tickerOpened: "dibuka",
  tickerClosed: "ditutup",
  tickerReleased: "dirilis",
  tickerFirstContribution: "kontribusi pertama",
  tickerBy: "oleh {handle}",
  tickerAria: "Aktivitas terbaru repositori",

  traceLabel: "jejak penalaran",
  traceTabsAria: "Cuplikan sesi",

  menuOpen: "Buka menu",
  menuClose: "Tutup menu",

  themeAuto: "otomatis",
  themeLight: "terang",
  themeDark: "gelap",
  themeAria: "Tema dokumentasi: {mode} (klik untuk mengganti)",
  themeTitle: "Tema dokumentasi · otomatis / terang / gelap",

  footerTagline:
    "Codewhale menyelam ke laut dalam, jadi Anda tidak perlu — dokumentasi, kode sumber, dan komunitas untuk runtime sumber terbuka.",
  footerProduct: "Produk",
  footerProject: "Proyek",
  footerDocs: "Dokumentasi",
  footerGuide: "Panduan memulai",
  footerInstall: "Instalasi",
  footerModels: "Model",
  footerRuntime: "Antarmuka runtime",
  footerFaq: "Tanya jawab",
  footerIssues: "Masalah",
  footerContribute: "Kontribusi",
  footerLicense: "Lisensi MIT",
  footerPricing: "Harga",
  footerTerms: "Ketentuan layanan",
  footerPrivacy: "Privasi",
  footerChangelog: "Catatan perubahan",
  footerCanonicalSource: "Sumber kanonis: ",
  footerReleases: " · Rilis: ",
  footerReleasesLink: "Rilis GitHub",
  footerSecurity: "Keamanan",

  switcherLabel: "Bahasa",
  switcherSwitchTo: "Beralih ke {label}",
  partialBadge: "(sebagian)",
};
