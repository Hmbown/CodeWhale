# Codewhale Classroom / Lab Install Checklist

A step-by-step checklist for IT admins deploying Codewhale on lab or classroom
machines running Windows.

> **Audience**: IT staff, teaching assistants, lab managers.
> **Prereq**: Each target machine runs Windows 10 (1809+) or Windows 11.

---

## Pre-install checklist (run once per machine)

| # | Task | Done? |
|---|------|-------|
| 1 | Confirm Windows version: `winver` → 10 build 17763+ or 11 | ☐ |
| 2 | Ensure the user account is a **standard user** (not a local admin). The installer does not require elevation. | ☐ |
| 3 | Verify outbound HTTPS (port 443) is open to `api.openai.com` (or whichever LLM provider the course uses). | ☐ |
| 4 | Obtain the installer: download `CodeWhaleSetup.exe` from a v0.8.50+ [release](https://github.com/Hmbown/CodeWhale/releases/latest) or from your department mirror. | ☐ |
| 5 | Verify SHA-256 hash against `codewhale-artifacts-sha256.txt` before deploying. | ☐ |
| 6 | Note that the public installer is currently unsigned and may trigger Windows SmartScreen unless your organization signs it before deployment. | ☐ |

---

## Installation

### Option A — Silent install (recommended for imaging / SCCM / Intune)

```powershell
# Run as the target user or via a per-user deployment tool
CodeWhaleSetup.exe /S
```

The silent installer:
- Installs to `%LOCALAPPDATA%\Programs\CodeWhale\bin`
- Adds the bin directory to the **current user** PATH
- Registers in Windows "Apps & Features" for uninstall

### Option B — Interactive install

1. Double-click `CodeWhaleSetup.exe`.
2. Accept the license.
3. Choose the install directory (default is fine for most setups).
4. Click **Install**.

### Option C — Manual fallback (no installer)

If the NSIS installer is blocked by group policy, install manually:

```powershell
# 1. Create directory
$binDir = "$env:LOCALAPPDATA\Programs\CodeWhale\bin"
New-Item -ItemType Directory -Force -Path $binDir

# 2. Download binaries (adjust URL to your mirror or release tag)
$tag = (Invoke-RestMethod -Uri "https://api.github.com/repos/Hmbown/CodeWhale/releases/latest").tag_name
Invoke-WebRequest -Uri "https://github.com/Hmbown/CodeWhale/releases/download/$tag/codewhale-windows-x64.exe"     -OutFile "$binDir\codewhale.exe"
Invoke-WebRequest -Uri "https://github.com/Hmbown/CodeWhale/releases/download/$tag/codewhale-tui-windows-x64.exe" -OutFile "$binDir\codewhale-tui.exe"

# 3. Add to user PATH (persistent)
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathParts = @($currentPath -split ";" | Where-Object { $_ })
if ($pathParts -notcontains $binDir) {
    $newPath = (@($pathParts) + $binDir) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
}

# 4. Refresh current session PATH
$env:Path = [Environment]::GetEnvironmentVariable("Path", "User") + ";" + [Environment]::GetEnvironmentVariable("Path", "Machine")
```

---

## Post-install verification

Run these on **each machine** (or spot-check a sample):

| # | Command | Expected output | Done? |
|---|---------|-----------------|-------|
| 1 | `codewhale --version` | Prints version string | ☐ |
| 2 | `codewhale doctor` | All checks pass | ☐ |
| 3 | `codewhale-tui --version` | Prints version string | ☐ |

If `codewhale` is not found, the user may need to open a **new** terminal window for PATH changes to take effect.

## Lab validation checklist

Run this once on a clean lab machine, and again on a machine that already has a
previous Codewhale install:

| # | Scenario | Expected result | Done? |
|---|----------|-----------------|-------|
| 1 | Install with no existing CodeWhale PATH entry | Adds exactly `%LOCALAPPDATA%\Programs\CodeWhale\bin` | ☐ |
| 2 | Install twice | PATH is not duplicated | ☐ |
| 3 | Install with a neighboring PATH entry such as `C:\Tools\CodeWhale\bin-extra` | Neighboring entry is preserved | ☐ |
| 4 | Upgrade by installing a newer `CodeWhaleSetup.exe` over an older one | Apps & Features version and both `--version` outputs match the new build | ☐ |
| 5 | Silent uninstall with `Uninstall.exe /S` | Files, uninstall registry entry, and only the exact installer PATH entry are removed | ☐ |

---

## API key provisioning

Each student needs an API key. Options:

| Method | Pros | Cons |
|--------|------|------|
| **Per-student key** | Individual usage tracking | More key management |
| **Shared lab key** | Simple to deploy | Harder to audit; rate limits shared |

### Deploying a shared key via environment variable

```powershell
# Set for current user (persists across reboots)
[Environment]::SetEnvironmentVariable("OPENAI_API_KEY", "sk-...", "User")
```

Or create a `config.toml` in `%APPDATA%\codewhale\`:

```toml
[provider]
api_key = "sk-..."
base_url = "https://api.openai.com/v1"
```

### Deploying per-student keys with Intune / GPO

Use a Group Policy Preference or Intune PowerShell script to set the
`OPENAI_API_KEY` environment variable per user. The variable name depends on
your LLM provider — see [CONFIGURATION.md](CONFIGURATION.md).

---

## Uninstall

### Silent uninstall

```powershell
& "$env:LOCALAPPDATA\Programs\CodeWhale\Uninstall.exe" /S
```

### Manual uninstall (if installer was not used)

```powershell
$binDir = "$env:LOCALAPPDATA\Programs\CodeWhale\bin"
Remove-Item -Recurse -Force (Split-Path $binDir)

# Remove from PATH
$currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
$newPath = ($currentPath -split ";" | Where-Object { $_ -and ($_ -ne $binDir) }) -join ";"
[Environment]::SetEnvironmentVariable("Path", $newPath, "User")
```

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `codewhale` not found after install | Open a **new** terminal. If still missing, check PATH: `echo $env:Path` |
| `MISSING_COMPANION_BINARY` | Ensure both `codewhale.exe` and `codewhale-tui.exe` are in the same directory |
| `TLS handshake` errors | Check proxy settings or use the CNB mirror (see [INSTALL.md](INSTALL.md)) |
| Antivirus quarantines binaries | Add the install directory to AV exclusions |
| `codewhale doctor` fails API check | Verify `OPENAI_API_KEY` is set or `config.toml` exists |

---

## Imaging / Golden Image Notes

If building a golden image (WIM/FFU):

1. Install Codewhale using Option A (silent) or Option C (manual).
2. Do **not** set API keys in the image — these are per-user/per-student.
3. The install directory (`%LOCALAPPDATA%\Programs\CodeWhale\bin`) is per-user,
   so it will be present for the user who installed it. For other users on the
   same machine, run the installer again or use Option C.
4. Alternatively, install to a shared location like `C:\Tools\CodeWhale\bin`
   and add it to the **machine** PATH:
   ```powershell
   [Environment]::SetEnvironmentVariable("Path", "$env:Path;C:\Tools\CodeWhale\bin", "Machine")
   ```

---

## Quick Reference: All file paths

| Item | Default location |
|------|-----------------|
| Binaries | `%LOCALAPPDATA%\Programs\CodeWhale\bin\` |
| User config | `%APPDATA%\codewhale\config.toml` |
| Uninstaller | `%LOCALAPPDATA%\Programs\CodeWhale\Uninstall.exe` |
| PATH entry | `HKCU\Environment\Path` (current user) |

---

*Last updated: 2026-06-02*
