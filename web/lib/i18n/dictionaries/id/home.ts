import type { HomeDict } from "../types";

/**
 * Indonesian home dictionary.
 *
 * Product vocabulary stays fixed: modes Plan / Act / Operate, permission
 * postures Ask / Auto-Review / Full Access, and the product name Codewhale —
 * exactly as the TUI locale pack (`crates/tui/locales/id.json`) renders them.
 * Commands, package names, and surface names (`codewhale exec`, Fleet,
 * Runtime API + MCP) stay literal; only the prose around them is translated.
 *
 * Section seals (法 行 起 界 面 众) are the paper's marks, shared across
 * locales.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — Menyelam ke laut dalam agar Anda tidak perlu melakukannya.",
  metaDescription:
    "Codewhale menyelam ke laut dalam agar Anda tidak perlu melakukannya — agen terminal yang memberi orang biasa daya ungkit LLM untuk membangun sesuatu. Berjalan di mesin Anda. Ditulis dengan Rust, lisensi MIT.",

  kicker: "Sumber terbuka · Model apa pun · Berjalan di terminal Anda",
  heroTitleA: "Menyelam ke laut dalam",
  heroTitleB: "agar Anda tidak perlu melakukannya.",
  heroIntro:
    "{brand} memberi orang biasa daya ungkit LLM untuk membangun sesuatu. Di terminal Anda, ia membaca repositori, mengedit berkas, menjalankan pemeriksaan, dan meninggalkan tanda terima — tanpa mengandaikan Anda sudah fasih berkode. Semuanya berjalan di mesin Anda; model adalah komponen yang dapat dipilih, bukan produknya.",
  install: "Instal",
  docs: "Dokumentasi",
  copy: "Salin",
  copied: "Tersalin ✓",

  installEyebrow: "instalasi satu baris",
  installRequirement: "perlu Node 18+ — tanpa toolchain Rust",
  installOtherWays: "cara lain →",

  latestRelease: "Rilis terbaru {tag}",
  releaseUnavailable: "Status rilis tidak tersedia",
  currentSource: "Sumber saat ini",
  sourceCandidate: "Kandidat sumber",
  providerRoutes: "{count} rute penyedia",
  publishedRelease: "rilis yang diterbitkan",
  figcaptionSourceCandidate: "kandidat sumber",

  shotSession: "Sesi saat ini",
  screenshotAlt:
    "Sesi terminal Codewhale saat ini yang menampilkan mode Operate, sang paus, komposer, dan bilah bawah",
  figcaption: "Sesi Codewhale saat ini · mode Operate · postur izin Ask",

  proofHeading: "Shell terminal bawah laut. Netral terhadap model. Mengutamakan lokal.",
  proofBody:
    "Bawa model di-host, gateway, atau lokal yang sudah Anda pakai. Codewhale berjalan di mesin Anda dan memperlakukan model sebagai komponen yang dapat dipilih—bukan sebagai produk. Mode Plan / Act / Operate dan postur izin yang eksplisit menjaga penyelaman mendalam tetap dalam kendali Anda.",

  sealDecides: "法",
  decidesEyebrow: "Lihat bagaimana ia memutuskan",
  decidesHeading: "Aturan yang bisa Anda amati langsung di jejak",
  decidesLede:
    "Cuplikan setia dari sesi nyata — aturan proyek yang berjenjang dapat diamati di dalam penalaran model, bukan sekadar klaim di halaman depan.",

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
    "Instalasi → sesi pertama tanpa kunci → koneksi penyedia → workflow Fleet pertama. Semua istilahnya dijelaskan di halaman kosakata.",
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
    ["Fleet", "Kerja multi-agen yang tahan lama"],
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
