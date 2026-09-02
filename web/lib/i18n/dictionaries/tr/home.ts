import type { HomeDict } from "../types";

/**
 * Turkish home dictionary — «gazete-okyanus» açılış sayfası.
 *
 * Güncel İngilizce yönünde özgün yeniden yazım: modelini kendin getir,
 * her şey senin makinende olur. Ürün sözlüğü TUI paketiyle aynı kalır:
 * Plan / Work / Operate, Ask / Auto-Review / Full Access, Codewhale, TUI,
 * `codewhale exec`, Runtime API + MCP, fleet, Node 18+, Rust, MIT.
 *
 * Bölüm mühürleri (法, 行, …) İngilizce baskıyla paylaşılan gliflerdir —
 * işaretlerdir, düzyazı değil.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — derinlere dalar, sen dalmak zorunda kalmazsın.",
  metaDescription:
    "Codewhale derinlere dalar, sen dalmak zorunda kalmazsın — terminal için açık kaynak kodlama ajanı. Modelini kendin getir. Senin makineninde çalışır. Rust, MIT.",

  kicker: "Açık kaynak · Modelini kendin getir · Terminalinde çalışır",
  heroTitleA: "Codewhale derinlere dalar,",
  heroTitleB: "sen dalmak zorunda kalmazsın.",
  heroIntro:
    "{brand}, terminalin için açık kaynaklı bir kodlama ajanıdır. Ona bir model ve bir görev ver — kodunu okur, dosyaları düzenler, kendi denetimlerini çalıştırır ve iş bitince ya da sana ihtiyaç duyduğunda durur. İstediğin modeli getir ya da modelleri karıştır: her role ayrı bir model sabitle.",
  install: "Kur",
  docs: "Belgeler",
  copy: "Kopyala",
  copied: "Kopyalandı ✓",

  installEyebrow: "tek satır kurulum",
  installRequirement: "Node 18+ gerekir — Rust araç zinciri gerekmez",
  installOtherWays: "diğer yollar →",

  latestRelease: "En yeni sürüm {tag}",
  releaseUnavailable: "Sürüm durumu kullanılamıyor",
  currentSource: "Kaynak",
  sourceCandidate: "Yayımlanmadı",
  providerRoutes: "{count} sağlayıcı",
  publishedRelease: "yayımlandı",
  figcaptionSourceCandidate: "yayımlanmadı",

  shotSession: "Geçerli oturum",
  screenshotAlt:
    "Operate modu, balina, besteci ve alt bilgisi görünen geçerli Codewhale terminal oturumu",
  figcaption: "Geçerli Codewhale oturumu · Operate modu · Ask izin duruşu",

  proofHeading: "Bir su altı terminal kabuğu. İstediğin model. Senin makinen.",
  proofBody:
    "Zaten kullandığın modeli getir — barındırılan, ağ geçidi üzerinden ya da yerel. Plan / Work / Operate ve açık izin duruşları dalışı senin denetiminde tutar.",

  sealDecides: "法",
  decidesEyebrow: "Nasıl karar verdiğini gör",
  decidesHeading: "İzinde izleyebildiğin kurallar",
  decidesLede:
    "Gerçek oturum kesitleri — sıralı proje kuralları modelin muakemesinde görünür; yalnızca açılış sayfası iddiası değil.",

  sealWorkflow: "行",
  workflowHeading: "Görevden doğrulanmış değişikliğe.",
  workflow: [
    ["İncele", "Depoyu, talimatlarını ve görevi oku."],
    ["Eyle", "Dosyaları açık onay sınırları içinde düzenle."],
    ["Doğrula", "Denetimleri çalıştır, sonucu incele."],
    ["Raporla", "Öz ve kalıcı bir makbuz bırak."],
  ],
  receiptAria: "Örnek iş makbuzu",
  receiptInspect: "depo ve talimatlar",
  receiptAct: "seçili izin duruşuyla düzenleme",
  receiptReport: "denetimler geçti · makbuz kaydedildi",

  sealStart: "起",
  startHeading: "Codewhale’a yeni misin? Baştan sona dört adım.",
  startLede:
    "Kur → anahtarsız ilk oturum → bir sağlayıcı bağla → fleet’ini kur. Terimler sözlük sayfasında tanımlı.",
  startGuideLink: "Başlangıç kılavuzunu oku →",
  startVocabularyLink: "Ürün sözlüğünü gör →",

  sealBoundaries: "界",
  boundariesHeadingA: "Senin modelin.",
  boundariesHeadingB: "Senin sınırların.",
  boundariesBody:
    "Modeli, çalışma modunu ve izin duruşunu açıkça seç. Bilinmeyen maliyet bilinmiyor olarak kalır; önizleme yüzeyleri de böyle etiketlenir.",
  hostedGatewayLocal: "Barındırılan, ağ geçidi ve yerel modeller",
  planActOperateDesc: "Salt okunur planlamadan otonom işletmeye",
  askAutoReviewDesc: "İş için izin duruşunu seç",
  tuiExecWebDesc: "Etkileşimli ve arayüzsüz çalışma zamanı yüzeyleri",

  sealSurfaces: "面",
  surfacesHeading: "Çalışma zamanını işin olduğu yerde kullan.",
  surfaces: [
    ["TUI", "Terminalde etkileşimli iş"],
    ["codewhale exec", "Betikler ve CI"],
    ["Web istemcisi", "Yalnızca geri döngülü tarayıcı istemcisi"],
    ["Runtime API + MCP", "Yerel entegrasyonlar"],
    ["fleet", "Kalıcı çok ajanlı iş"],
  ],
  runtimeLink: "Çalışma zamanı yüzeylerini ve kararlılık notlarını gör →",

  installBandHeading: "Tek komutla başla.",
  binaries: "İkililer",
  chinaMirrors: "Çin yansıları",
  installGuideLink: "Kurulum kılavuzunu oku →",

  sealCommunity: "众",
  communityHeading: "Açıkça, halk önünde inşa edildi",
  communityBody:
    "MIT lisanslı; çalışma zamanları, sağlayıcılar, platformlar, belgelendirme ve testler katkısıyla şekillendi.",
  communityLinksAria: "Topluluk bağlantıları",
  contribute: "Katkıda bulun",
};
