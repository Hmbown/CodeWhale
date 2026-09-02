<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale adalah agen pemrograman sumber terbuka untuk terminal Anda, dibuat dengan Rust dan dikembangkan secara terbuka bersama orang-orang yang menggunakannya.

![Codewhale berjalan di terminal](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## Instalasi

```bash
npm install -g codewhale
codewhale
```

Saat pertama dijalankan, Codewhale membantu Anda menghubungkan penyedia atau tetap bekerja secara luring. Codewhale juga mendukung Cargo, Docker, Nix, Scoop, arsip siap pakai, Android/Termux, dan mirror CNB. Lihat [panduan instalasi](docs/INSTALL.md).

Penyelesaian Tab cukup diaktifkan dengan satu perintah per shell — `codewhale completion bash|zsh|fish|powershell|elvish`. Lihat [penyelesaian shell](docs/INSTALL.md#8-shell-completions).

## Penggunaan

Bicaralah dengan Codewhale seperti Anda berbicara dengan rekan satu tim:

```text
Fix the failing tests and explain what changed.
```

Atau jalankan tugas tanpa membuka TUI:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale dapat membaca repositori Anda, mengedit berkas, menjalankan perintah, memeriksa hasil, dan terus bekerja menuju tujuan. Anda menentukan seberapa besar akses yang dimilikinya.

## Mengapa Codewhale

- **Gunakan model yang Anda inginkan.** Hubungkan penyedia terkelola atau model lokal melalui Ollama, vLLM, atau SGLang. Ganti penyedia dan model dengan `/model`.
- **Tetap memegang kendali.** Plan hanya dapat membaca. Ask, Auto-Review, dan Full Access menampilkan perilaku persetujuan dengan jelas. `/undo` membatalkan giliran terakhir dan `/restore` mengembalikan ruang kerja ke snapshot sebelumnya.
- **Jaga agar pekerjaan panjang tetap teratur.** Simpan sesi, tetapkan `/goal` yang bertahan lama, tinjau alur kerja sebelum dijalankan, dan koordinasikan agen tanpa memasukkan instruksi internal mereka ke transkrip Anda.
- **Perluas agen yang sudah Anda miliki.** Hubungkan server MCP dan keterampilan, konfigurasikan hook, dan simpan peran agen sebagai berkas yang mudah dibaca di proyek atau pengaturan pribadi Anda.

Jalankan `/help` di TUI untuk melihat perintah dan pintasan papan ketik.

## Keamanan

Codewhale berjalan di mesin Anda dengan akses yang Anda berikan. Mode persetujuan dan aturan repositori membatasi tindakan agen; sandbox OS opsional menambahkan batas eksekusi yang lebih kuat jika didukung. Harga model yang belum diketahui tetap ditampilkan sebagai tidak diketahui, bukan dilaporkan gratis.

Baca [urutan otorisasi](docs/AUTHORIZATION_ORDER.md) untuk susunan kebijakan yang tepat dan [konfigurasi](docs/CONFIGURATION.md) untuk pengaturan lokal.

## Dokumentasi

- [Penyedia dan model lokal](docs/PROVIDERS.md)
- [Tim agen](docs/FLEET.md)
- [MCP](docs/MCP.md), [hook](docs/HOOKS.md), dan [konfigurasi](docs/CONFIGURATION.md)
- [Klien web lokal](docs/WEB.md)
- [Semua dokumentasi](docs)

## Bergabung dengan komunitas

Codewhale menjadi lebih baik ketika orang menggunakannya, melaporkan hal yang terasa kurang tepat, dan membantu memperbaikinya. Jika penyedia belum tersedia, alur kerja terasa janggal, atau UI terminal menghambat Anda, [buat issue](https://github.com/Hmbown/CodeWhale/issues). Jika Anda tahu cara memperbaikinya, [buat pull request](CONTRIBUTING.md). Kontribusi pertama sangat disambut, dan kontributor tetap menerima kredit untuk pekerjaan yang digabungkan.

Bergabunglah di [Discord](https://discord.gg/37gfS3ksug), atau tambahkan Hunter di WeChat (`hunterbown`) dan mintalah untuk bergabung dengan grup Whale Brothers.

## Riwayat proyek

Codewhale bermula sebagai `deepseek-tui` dan tetap mempertahankan kompatibilitas konfigurasi serta sesinya. Kini Codewhale netral terhadap penyedia, dikelola secara independen, dan tidak berafiliasi dengan penyedia model mana pun.

Terima kasih kepada setiap kontributor dan komunitas sumber terbuka yang membantu proyek ini tumbuh. Lihat [catatan kontributor](docs/CONTRIBUTORS.md).

## Lisensi

[MIT](LICENSE). Bagian yang diadaptasi dari proyek sumber terbuka lain dicatat dalam [pemberitahuan pihak ketiga](docs/THIRD_PARTY_NOTICES.md).
