import type { HomeDict } from "../types";

/**
 * Indonesian home dictionary — native rewrite mirroring the current English
 * direction: "dives into the deep", bring-your-own-model, runs on your
 * machine. Every trace of the old positioning has been dropped.
 *
 * Product vocabulary stays fixed: modes Plan / Work / Operate, permission
 * postures Ask / Auto-Review / Full Access, and the product name Codewhale —
 * exactly as the TUI locale pack (`crates/tui/locales/id.json`) renders them.
 * Commands, package names, and surface names (`codewhale exec`, fleet,
 * Runtime API + MCP) stay literal; only the prose around them is translated.
 *
 * Section seals (法 行 起 界 面 众) are the paper's marks, shared across
 * locales.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — menyelam ke laut dalam, jadi Anda tidak perlu.",
  metaDescription:
    "Codewhale menyelam ke laut dalam, jadi Anda tidak perlu — agen coding terminal sumber terbuka. Bawa model sendiri. Berjalan di mesin Anda. Rust, MIT.",

  kicker: "Sumber terbuka · Bawa model sendiri · Berjalan di terminal Anda",
  heroTitleA: "Codewhale menyelam ke laut dalam",
  heroTitleB: "jadi Anda tidak perlu.",
  heroIntro:
    "{brand} adalah agen coding sumber terbuka untuk terminal Anda. Beri ia model dan tugas — ia membaca kode Anda, mengedit berkas, menjalankan pemeriksaannya sendiri, dan berhenti saat pekerjaan selesai atau saat butuh Anda. Bawa model apa pun, atau campurkan: pasang model berbeda untuk tiap peran.",
  install: "Instal",
  docs: "Dokumentasi",
  copy: "Salin",
  copied: "Tersalin ✓",

  installEyebrow: "instalasi satu baris",
  installRequirement: "perlu Node 18+ — tanpa toolchain Rust",
  installOtherWays: "cara lain →",

  latestRelease: "Rilis terbaru {tag}",
  releaseUnavailable: "Status rilis tidak tersedia",
  currentSource: "Sumber",
  sourceCandidate: "Belum dirilis",
  providerRoutes: "{count} penyedia",
  publishedRelease: "dirilis",
  figcaptionSourceCandidate: "belum dirilis",

  shotSession: "Sesi saat ini",
  screenshotAlt:
    "Sesi terminal Codewhale saat ini yang menampilkan mode Operate, sang paus, komposer, dan bilah bawah",
  figcaption: "Sesi Codewhale saat ini · mode Operate · postur izin Ask",

  proofHeading: "Shell terminal bawah laut. Model apa pun. Di mesin Anda.",
  proofBody:
    "Bawa model yang sudah Anda pakai — di-host, gateway, atau lokal. Plan / Work / Operate dan postur izin eksplisit menjaga penyelaman tetap dalam kendali Anda.",

  sealDecides: "法",
  decidesEyebrow: "Lihat bagaimana ia memutuskan",
  decidesHeading: "Aturan yang bisa Anda saksikan di jejak",
  decidesLede:
    "Cuplikan sesi nyata — aturan proyek yang berjenjang terlihat di penalaran model, bukan sekadar klaim di halaman depan.",

  sealWorkflow: "行",
  workflowHeading: "Dari tugas hingga perubahan terverifikasi.",
  workflow: [
    ["Memeriksa", "Membaca repositori, instruksinya, dan tugasnya."],
    ["Bertindak", "Mengedit berkas dalam batas persetujuan yang eksplisit."],
    ["Memverifikasi", "Menjalankan pemeriksaan dan menelaah hasilnya."],
    ["Melaporkan", "Meninggalkan tanda terima yang ringkas dan tahan lama."],
  ],
  receiptAria: "Contoh tanda terima kerja",
  receiptInspect: "repositori dan instruksi",
  receiptAct: "mengedit melalui postur izin yang dipilih",
  receiptReport: "pemeriksaan lulus · tanda terima tersimpan",

  sealStart: "起",
  startHeading: "Baru mengenal Codewhale? Empat langkah dari awal sampai akhir.",
  startLede:
    "Instal → sesi pertama tanpa kunci → hubungkan penyedia → workflow fleet pertama. Istilah didefinisikan di halaman kosakata.",
  startGuideLink: "Baca panduan memulai →",
  startVocabularyLink: "Lihat kosakata produk →",

  sealBoundaries: "界",
  boundariesHeadingA: "Model Anda.",
  boundariesHeadingB: "Batas Anda.",
  boundariesBody:
    "Pilih model, mode kerja, dan postur izin secara eksplisit. Biaya yang tidak diketahui tetap tidak diketahui, dan antarmuka pratinjau tetap ditandai sebagai pratinjau.",
  hostedGatewayLocal: "Model di-host, gateway, dan lokal",
  planActOperateDesc: "Perencanaan baca-saja hingga pengoperasian otonom",
  askAutoReviewDesc: "Pilih postur izin untuk pekerjaan yang dijalankan",
  tuiExecWebDesc: "Antarmuka runtime interaktif dan headless",

  sealSurfaces: "面",
  surfacesHeading: "Gunakan runtime di tempat pekerjaan berlangsung.",
  surfaces: [
    ["TUI", "Kerja terminal interaktif"],
    ["codewhale exec", "Skrip dan CI"],
    ["Klien Web", "Klien peramban khusus loopback"],
    ["Runtime API + MCP", "Integrasi lokal"],
    ["fleet", "Kerja multi-agen yang tahan lama"],
  ],
  runtimeLink: "Lihat antarmuka runtime dan catatan stabilitas →",

  installBandHeading: "Mulai dengan satu perintah.",
  binaries: "Biner",
  chinaMirrors: "Mirror Tiongkok",
  installGuideLink: "Baca panduan instalasi →",

  sealCommunity: "众",
  communityHeading: "Dibangun secara terbuka",
  communityBody:
    "Berlisensi MIT dan dibentuk oleh para kontributor di berbagai runtime, penyedia, platform, dokumentasi, dan pengujian.",
  communityLinksAria: "Tautan komunitas",
  contribute: "Kontribusi",
};
