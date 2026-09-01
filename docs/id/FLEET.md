# Agent Pod (Armada Agen)

Agent Pod adalah control plane yang mengutamakan lokal (*local-first*) untuk eksekusi banyak pekerja (*multi-worker*) yang tahan lama. Pod **bukanlah** mesin eksekusi terpisah: worker Pod adalah eksekusi `codewhale exec` tanpa antarmuka yang diluncurkan dan dilacak secara permanen oleh Runtime.

**Pod** adalah nama yang ditampilkan kepada pengguna. **Fleet** tetap menjadi
nama kompatibilitas untuk artefak tersimpan: `.codewhale/fleet.jsonl`,
`.codewhale/fleet/`, tabel konfigurasi `[fleet]`, dan flag Workflow `--fleet`.
Perintah `codewhale fleet …` dan `/fleet …` tetap diterima sebagai alias, tetapi
dokumentasi dan bantuan baru menggunakan `codewhale pod …` dan `/pod …`.

Gunakan Pod daripada pembagian tugas agen yang berumur pendek ketika pekerjaan membutuhkan percobaan ulang (*retry*), ketahanan terhadap mode tidur/restart komputer, eksekusi jarak jauh, bukti tanda terima (*receipts*), atau jejak audit ber-ledger.

---

## Perintah Dasar CLI Pod

```sh
codewhale pod init
codewhale pod run tasks.json --max-workers 4
codewhale pod status
codewhale pod inspect <worker-id>
codewhale pod logs <worker-id>
codewhale pod artifacts <worker-id>
codewhale pod interrupt <worker-id>
codewhale pod restart <worker-id>
codewhale pod resume <run-id>
codewhale pod stop --all
```

`codewhale pod resume <run-id>` adalah perintah pemulihan setelah sistem terhenti: perintah ini memutar ulang ledger, merekonsiliasi tugas yang terhenti (Mencoba lagi sesuai anggaran tugas, atau melaporkannya jika gagal), lalu menampilkan status setelah pemulihan. Perintah ini aman dijalankan setelah laptop terbangun dari mode tidur atau setelah restart runtime.

---

## Lokasi Penyimpanan Status

Status Pod disimpan di dalam ruang kerja di bawah `.codewhale/fleet.jsonl`. Log pekerja dan log adapter disimpan di bawah `.codewhale/fleet/` dan `.codewhale/fleet-host/`.

### Perbedaan Status Pod dan Worker Sesi

- Perintah TUI `/pod status` dan perintah shell `codewhale pod status` membaca ledger Pod persisten yang sama di `.codewhale/fleet.jsonl`.
- Gunakan `/subagents` atau `/pod workers` untuk menampilkan sub-agen yang hanya terhubung ke sesi TUI saat ini.
