# Rebrand: DeepSeek TUI → Codewhale

Mulai dari versi **v0.8.41**, proyek ini dirilis dengan nama baru: `codewhale`.

Dokumen ini menjelaskan apa saja yang berubah, apa yang tetap sama, dan cara melakukan migrasi. Seluruh integrasi penyedia (provider) DeepSeek tidak mengalami perubahan — hanya nama merek CLI / TUI lokal yang diperbarui.

---

## Ringkasan Migrasi

```bash
# 1. Hapus instalasi paket atau biner lama.
npm uninstall -g deepseek-tui      # atau:
cargo uninstall deepseek-tui-cli 2>/dev/null || true
cargo uninstall deepseek-tui 2>/dev/null || true

# 2. Pasang dengan nama baru.
npm install -g codewhale            # atau:
cargo install codewhale-cli --locked

# 3. Jalankan perintah baru.
codewhale doctor
codewhale
```

Berkas dan direktori Anda yang ada seperti `~/.deepseek/config.toml`, `~/.deepseek/sessions/`, `~/.deepseek/skills/`, `~/.deepseek/tasks/`, dan `~/.deepseek/mcp.json` **tidak akan dihapus**. Instalasi baru Codewhale mengutamakan `~/.codewhale/`, sementara direktori lama `~/.deepseek/` tetap dibaca sebagai fallback selama masa transisi. Variabel lingkungan `DEEPSEEK_*` tetap berfungsi sebagaimana mestinya.

---

## Apa Saja yang Berubah Nama

| Komponen | Sebelum | Sesudah |
|---|---|---|
| Perintah Terpasang | `deepseek` / `deepseek-tui` | `codewhale` / `codew` |
| Paket Wrapper npm | `deepseek-tui` | `codewhale` |
| Crate Crates.io | `deepseek-tui-cli` / `deepseek-tui` | `codewhale-cli` / `codewhale-tui` |
| Aset Rilis | `deepseek-<platform>` / `deepseek-tui-<platform>` | `codewhale-<platform>` / `codew-<platform>`; `codewhale-tui-<platform>` hanya nama kompatibilitas |
| Manifest Checksum | `deepseek-artifacts-sha256.txt` | `codewhale-artifacts-sha256.txt` |

---

## Apa yang TIDAK Berubah

Semua hal yang berkaitan dengan API penyedia DeepSeek tetap berjalan persis seperti sebelumnya:
- **Variabel Lingkungan**: `DEEPSEEK_API_KEY`, `DEEPSEEK_BASE_URL`, `DEEPSEEK_MODEL`, `DEEPSEEK_PROVIDER`, `DEEPSEEK_PROFILE`, dll. tetap didukung sepenuhnya.
- **`DEEPSEEK_YOLO`**: kini usang, tetapi masih dibaca sebagai alias dari `CODEWHALE_YOLO` selama 0.9.x agar skrip lama tetap berjalan (jika keduanya diisi, `CODEWHALE_YOLO` yang menang). Dihapus di 0.10 (#5443); gunakan `CODEWHALE_YOLO` untuk skrip baru.
- **Konfigurasi Penyedia**: Pengaturan rute `[providers.deepseek]` pada `config.toml` tetap valid.
