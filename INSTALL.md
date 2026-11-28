# Memasang WIN MAC

Panduan untuk pengguna akhir (Windows 11). Ada dua cara memasang:

- **Installer (`winmac-setup.exe`)** — disarankan untuk pengguna umum: wizard, shortcut Start Menu, dan entri *Add/Remove Programs*. Per-user, tanpa perlu admin.
- **Portabel (`winmac.exe`)** — cukup jalankan satu file; tanpa installer.

Keduanya sama-sama aplikasi native Rust, tanpa dependensi runtime eksternal.

## Cara 1 — Installer (disarankan)

1. Unduh **`winmac-setup.exe`** dari halaman rilis.
2. Jalankan installer → klik **Next**.
3. Pada halaman **License Agreement**, **centang** *"I have read and accept the license agreement"* (wajib) agar tombol lanjut aktif.
4. Pilih opsi: *Start Menu shortcut*, *desktop shortcut*, *jalankan setelah pemasangan*, dan *Start WIN MAC automatically when you log in* (opsional) → klik **Install**.
5. Selesai. WIN MAC terpasang di `%LOCALAPPDATA%\Programs\WIN MAC` dan terdaftar di **Add/Remove Programs**.

**Uninstall (via installer):** buka **Settings → Apps → Installed apps** (atau Control Panel → Programs) → cari **WIN MAC** → **Uninstall**. Uninstaller otomatis menutup WIN MAC, memulihkan taskbar, menghapus file & shortcut, lalu **menanyakan** apakah ikut menghapus data pengaturan (`%APPDATA%\WinMac`) — pilih **Ya** untuk reset penuh, atau **Tidak** untuk mempertahankan setelan. Mode senyap (`/S`) selalu mempertahankan data.

## 1. Persyaratan

- **Windows 11** (x64). Beberapa fitur (live glass capture) membutuhkan Windows 11; di Windows 10 beberapa efek dinonaktifkan otomatis.
- Tidak perlu Rust, .NET, Visual Studio, atau runtime apa pun.

## 2. Unduh

1. Buka halaman rilis: **https://github.com/Maxodus126/WinMac/releases/latest**
2. Unduh **`winmac.exe`** (dan opsional `winmac.exe.sha256` untuk verifikasi).

> Periksa checksum (PowerShell):
> ```powershell
> Get-FileHash .\winmac.exe -Algorithm SHA256
> ```
> Bandingkan dengan isi `winmac.exe.sha256` (64 karakter hex).

## 3. Tempatkan file

Jalankan dari **folder tetap**, bukan langsung dari folder unduhan/zip:

```powershell
# contoh: buat folder program Anda sendiri
New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\Programs\WIN MAC"
Move-Item .\winmac.exe "$env:LOCALAPPDATA\Programs\WIN MAC\winmac.exe"
```

Lalu jalankan:

```powershell
& "$env:LOCALAPPDATA\Programs\WIN MAC\winmac.exe"
```

> **Penting:** jangan jalankan dari folder bersandi rendah (low integrity), mis. folder yang di-sandbox. Jika aplikasi berjalan pada low integrity, ia akan menampilkan peringatan dan menolak menyimpan pengaturan / memesan ruang layar. Salin exe ke folder normal seperti di atas.

## 4. Mengatasi peringatan SmartScreen

Karena EXE **belum ditandatangani digital**, Windows mungkin menampilkan **"Windows protected your PC"** (SmartScreen) saat pertama kali:

1. Klik **"More info"** (Informasi lainnya).
2. Klik **"Run anyway"** (Tetap jalankan).

Ini hanya muncul sekali per mesin (atau setelah OS me-reset reputasi file). Semakin banyak pengguna yang mengunduh & menjalankan, semakin cepat reputasi file membaik.

## 5. Pertama kali dijalankan

- Dock **Liquid Glass** muncul di bawah; menu bar muncul di atas.
- Pengaturan tersimpan otomatis di **`%APPDATA%\WinMac\config.json`**.
- **Default baru (v1.0.2)** bersifat aman: taskbar Windows **tidak** disembunyikan dan Alt+Tab **tidak** diganti otomatis.
  - Untuk mengaktifkannya: buka **Settings** (klik kanan dock → *Dock Settings…*), lalu nyalakan *Hide the Windows taskbar* dan/atau *Mac-style Alt+Tab*.
- Pengaturan lain bisa diubah dari jendela Settings (dengan pratinjau langsung).

## 6. Berhenti & memulihkan taskbar

- **Keluar normal:** klik kanan dock → **Quit WIN MAC** (atau klik kanan menu bar → Quit). Taskbar kembali normal.
- **Jika aplikasi mati paksa** (taskkill /F, reboot bermasalah) saat taskbar disembunyikan, pulihkan taskbar dengan:
  ```powershell
  winmac.exe --restore
  ```
  (Ini juga menjalankan ulang animasi minimize Windows jika sempat terhenti.)

## 7. Menghapus instalasi

- **Jika memakai installer:** gunakan **Settings → Apps → Installed apps → WIN MAC → Uninstall** (uninstaller otomatis memulihkan taskbar & menghapus file).
- **Jika portabel:** 1) tutup WIN MAC, 2) hapus folder program (contoh `%LOCALAPPDATA%\Programs\WIN MAC`), 3) (opsional) hapus `%APPDATA%\WinMac` untuk reset penuh.

---

## Checklist Smoke Test (sebelum promosi luas)

Jalankan di **Windows 11 bersih** (mesin uji), lalu centang:

- [ ] App menyala tanpa peringatan selain SmartScreen; dock & menu bar muncul.
- [ ] **Minimize genie**: klik tombol minimize jendela → jendela "menyusut" ke ikon dock; restore kembali mulus.
- [ ] **Game mode / fullscreen**: buka video/game layar penuh → dock & menu bar menghilang; keluar fullscreen → kembali.
- [ ] **Auto-hide** (jika diaktifkan): pointer ke tepi bawah memunculkan dock; saat menjauh dock menyembunyikan diri.
- [ ] **Drag & drop file**: seret file ke ikon aplik → terbuka; ke Trash → masuk Recycle Bin.
- [ ] **Multi-monitor**: sambungkan monitor kedua; pindahkan jendela; pastikan dock tetap di monitor utama dan tidak crash.
- [ ] **Ganti wallpaper**: klik kanan desktop → *Next desktop background* → glass/backdrop tidak crash.
- [ ] **Alt+Tab & Spotlight**: `Alt+Space` memunculkan Spotlight; pencarian berfungsi.
- [ ] **Settings**: geser slider ikon/magnifikasi; pratinjau ikut berubah; klik *Restart* berfungsi.
- [ ] **Quit/restart**: keluar normal; jalankan ulang; tidak ada `panic.log` baru di `%APPDATA%\WinMac\`.
- [ ] **Taskbar pulih** setelah keluar normal.

---

## Kebijakan Privasi (ringkas)

WIN MAC **tidak mengirim telemetry** dan tidak mengunggah data pribadi apa pun. Data yang disentuh:

- **Cek update** — menghubungi **API GitHub** (hanya menanyakan versi terbaru; tidak mengirim data pribadi). Dapat **dimatikan** lewat toggle *"Check for updates"* di Settings (kunci `CheckUpdates`, default aktif).
- **AI usage (Claude Code / Codex)** — hanya **membaca log lokal** di mesin Anda (`%USERPROFILE%\.claude` dan `%USERPROFILE%\.codex`). Tidak ada yang dikirim ke jaringan. Sebuah **optional** fitur *"Claude limits from your Claude login"* (default **mati**) menghubungi `api.anthropic.com` untuk menampilkan batas pemakaian — dan hanya jika Anda mengaktifkannya.
- **Semua pengaturan** tersimpan lokal di `%APPDATA%\WinMac\config.json`. Tidak ada akun, tidak ada cookie, tidak ada data browsing.

Jaringan hanya digunakan bila: cek update aktif, atau fitur login Claude diaktifkan. Yang lain berjalan sepenuhnya lokal.
