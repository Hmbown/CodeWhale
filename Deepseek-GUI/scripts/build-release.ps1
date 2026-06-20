# DeepSeek GUI 一键 release 构建脚本（Windows）
# 用法：在 PowerShell 中执行 .\scripts\build-release.ps1
$ErrorActionPreference = "Stop"

# Rust / MinGW 工具链（按本机 D 盘安装路径配置）
$RustBin = "D:\Config\rust\rustup\toolchains\stable-x86_64-pc-windows-gnu\bin"
$MingwBin = "D:\Config\mingw64\bin"
$CargoHome = "D:\Config\rust\cargo\bin"
$env:Path = "$MingwBin;$RustBin;$CargoHome;" + $env:Path

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$GuiRoot = Join-Path $RepoRoot "Deepseek-GUI"
$TuiOut = Join-Path $RepoRoot "target\release\deepseek-tui.exe"
$SidecarDir = Join-Path $GuiRoot "src-tauri\bin"
$ReleaseDir = Join-Path $GuiRoot "src-tauri\target\release"

Write-Host "==> 停止可能占用文件的进程..."
Get-Process deepseek-gui, deepseek-tui -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Host "==> 构建 deepseek-tui (release)..."
Push-Location $RepoRoot
cargo build --release -p deepseek-tui
Pop-Location

Write-Host "==> 构建前端 dist..."
Push-Location $GuiRoot
npm run build
Pop-Location

Write-Host "==> 复制 sidecar 到 src-tauri/bin..."
New-Item -ItemType Directory -Force -Path $SidecarDir | Out-Null
Copy-Item -Force $TuiOut (Join-Path $SidecarDir "deepseek-tui.exe")
Copy-Item -Force $TuiOut (Join-Path $ReleaseDir "deepseek-tui.exe")

Write-Host "==> Tauri 打包 (NSIS + MSI)..."
Push-Location $GuiRoot
npm run tauri:build
Pop-Location

Write-Host ""
Write-Host "构建完成。产物："
Write-Host "  GUI:       $ReleaseDir\deepseek-gui.exe"
Write-Host "  Sidecar:   $ReleaseDir\deepseek-tui.exe"
Write-Host "  NSIS:      $ReleaseDir\bundle\nsis\DeepSeek GUI_0.1.0_x64-setup.exe"
Write-Host "  MSI:       $ReleaseDir\bundle\msi\DeepSeek GUI_0.1.0_x64_en-US.msi"
