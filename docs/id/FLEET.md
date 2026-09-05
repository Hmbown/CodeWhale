# Agent fleet (Armada Agen)

Agent fleet adalah control plane yang mengutamakan lokal (*local-first*) untuk eksekusi banyak pekerja (*multi-worker*) yang tahan lama. Fleet **bukanlah** mesin eksekusi terpisah: worker fleet adalah eksekusi `codewhale exec` tanpa antarmuka yang diluncurkan dan dilacak secara permanen oleh Runtime.

**Fleet** adalah nama publik untuk inventaris model pengguna: siapa yang ada di
roster dan anggota mana yang dipilih. Ledger `.codewhale/fleet.jsonl`,
`.codewhale/fleet/`, tabel konfigurasi `[fleet]`, dan flag Workflow `--fleet`
menggunakan nama yang sama.

Gunakan fleet daripada pembagian tugas agen yang berumur pendek ketika pekerjaan membutuhkan percobaan ulang (*retry*), ketahanan terhadap mode tidur/restart komputer, eksekusi jarak jauh, bukti tanda terima (*receipts*), atau jejak audit ber-ledger.

---

## Perintah Dasar CLI fleet

```sh
codewhale fleet init
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet artifacts <worker-id>
codewhale fleet interrupt <worker-id>
codewhale fleet restart <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all
```

`codewhale fleet resume <run-id>` adalah perintah pemulihan setelah sistem terhenti: perintah ini memutar ulang ledger, merekonsiliasi tugas yang terhenti (Mencoba lagi sesuai anggaran tugas, atau melaporkannya jika gagal), lalu menampilkan status setelah pemulihan. Perintah ini aman dijalankan setelah laptop terbangun dari mode tidur atau setelah restart runtime.

---

## Lokasi Penyimpanan Status

Status fleet disimpan di dalam ruang kerja di bawah `.codewhale/fleet.jsonl`. Log pekerja dan log adapter disimpan di bawah `.codewhale/fleet/` dan `.codewhale/fleet-host/`.

### Perbedaan Status fleet dan Worker Sesi

- Perintah TUI `/fleet status` dan perintah shell `codewhale fleet status` membaca ledger fleet persisten yang sama di `.codewhale/fleet.jsonl`.
- Gunakan `/subagents` atau `/fleet workers` untuk menampilkan sub-agen yang hanya terhubung ke sesi TUI saat ini.
