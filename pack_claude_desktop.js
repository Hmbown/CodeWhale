// pack_claude_desktop.js
// Packages the FULL Claude Desktop (MS Store) experience:
// - Claude-3p user data (sessions, configs, embedded claude.exe)
// - Chrome Native Host
// - npm Claude Code (newer version, as fallback)
// - Skills and configs
// - Deployment script

const { execSync } = require('child_process');
const fs = require('fs');
const p = require('path');
const h = require('os').homedir();
const tmp = p.join(require('os').tmpdir(), 'cc_desktop_pack');
const outZip = p.join(h, 'Desktop', 'claude_desktop_full.zip');

console.log('=== Packing Claude Desktop (MS Store) - Full Migration Package ===\n');
fs.rmSync(tmp, { recursive: true, force: true });
fs.mkdirSync(tmp, { recursive: true });

function cp(src, dst) {
  if (!fs.existsSync(src)) { console.log('  MISS:', src); return false; }
  const dstDir = p.dirname(dst);
  if (!fs.existsSync(dstDir)) fs.mkdirSync(dstDir, { recursive: true });
  fs.cpSync(src, dst, { recursive: true });
  let size = 0;
  try {
    const stat = fs.statSync(src);
    if (stat.isFile()) size = stat.size;
    else {
      function calcDir(d) {
        fs.readdirSync(d, { withFileTypes: true }).forEach(e => {
          const fp = p.join(d, e.name);
          if (e.isFile()) size += fs.statSync(fp).size;
          else if (e.isDirectory()) calcDir(fp);
        });
      }
      calcDir(src);
    }
  } catch(e) {}
  console.log('  OK:', src.replace(h, '~'), '->', (size/(1024*1024)).toFixed(1), 'MB');
  return true;
}

function cpFile(src, dst) {
  if (!fs.existsSync(src)) { console.log('  MISS:', src); return false; }
  const dstDir = p.dirname(dst);
  if (!fs.existsSync(dstDir)) fs.mkdirSync(dstDir, { recursive: true });
  fs.copyFileSync(src, dst);
  const size = fs.statSync(src).size;
  console.log('  OK:', src.replace(h, '~'), '->', (size/(1024*1024)).toFixed(1), 'MB');
  return true;
}

// 1. Claude Desktop User Data (Claude-3p)
console.log('\n[1/6] Claude Desktop user data (Claude-3p)');
const claude3pSrc = p.join(h, 'AppData', 'Local', 'Packages', 'Claude_pzs8sxrjxfjjc', 'LocalCache', 'Roaming', 'Claude-3p');
const claude3pDst = p.join(tmp, 'Claude-3p');

if (fs.existsSync(claude3pSrc)) {
  const essentialItems = [
    'claude-code', 'claude-code-sessions', 'claude-code-vm', 'vm_bundles',
    'blob_storage', 'claude_desktop_config.json', 'config.json', 'configLibrary',
    'Preferences', 'developer_settings.json', 'window-state.json',
    'cowork-enabled-cli-ops.json', 'local-agent-mode-sessions',
    'Local Storage', 'Session Storage', 'IndexedDB', 'Partitions',
    'Network', 'Shared Dictionary', 'SharedStorage', 'SharedStorage-wal',
    'DIPS', 'DIPS-wal', 'fcache', 'git-worktrees.json', 'ant-did',
    'title-gen', 'Dictionaries', 'Local State', 'lockfile', 'WebStorage',
    'en-US-10-1.bdic',
  ];
  
  essentialItems.forEach(item => {
    const src = p.join(claude3pSrc, item);
    const dst = p.join(claude3pDst, item);
    if (fs.existsSync(src)) {
      const stat = fs.statSync(src);
      if (stat.isDirectory()) cp(src, dst);
      else cpFile(src, dst);
    }
  });
  console.log('  (Skipped: Cache, Code Cache, GPUCache, Dawn*Cache, Crashpad, logs, sentry)');
} else {
  console.log('  WARNING: Claude-3p not found');
}

// 2. Chrome Native Host
console.log('\n[2/6] Chrome Native Host');
const cnhSrc = p.join(h, 'AppData', 'Local', 'Packages', 'Claude_pzs8sxrjxfjjc', 'LocalCache', 'Roaming', 'Claude', 'ChromeNativeHost');
if (fs.existsSync(cnhSrc)) {
  cp(cnhSrc, p.join(tmp, 'ChromeNativeHost'));
}

// 3. npm Claude Code
console.log('\n[3/6] npm Claude Code (v2.1.126)');
const npmCcSrc = p.join(h, 'AppData', 'Roaming', 'npm', 'node_modules', '@anthropic-ai', 'claude-code');
cp(npmCcSrc, p.join(tmp, 'claude-code-pkg'));

const npmDir = p.join(h, 'AppData', 'Roaming', 'npm');
['claude', 'claude.cmd', 'claude.ps1'].forEach(f => {
  cpFile(p.join(npmDir, f), p.join(tmp, 'npm-shims', f));
});

// 4. Skills and Configs
console.log('\n[4/6] Skills and configurations');
cp(p.join(h, '.claude', 'skills'), p.join(tmp, '.claude', 'skills'));
cp(p.join(h, '.agents', 'skills'), p.join(tmp, '.agents', 'skills'));
cpFile(p.join(h, '.claude.json'), p.join(tmp, '.claude.json'));

const sl = p.join(h, '.agents', '.skill-lock.json');
if (fs.existsSync(sl)) cpFile(sl, p.join(tmp, '.agents', '.skill-lock.json'));

// 5. Deploy script
console.log('\n[5/6] Creating deployment script');
const deployPs1 = [
'# Claude Desktop + Claude Code Deployment Script',
'# Right-click -> Run with PowerShell',
'',
'Write-Host "============================================" -ForegroundColor Cyan',
'Write-Host "  Claude Desktop Full Migration" -ForegroundColor Cyan',
'Write-Host "============================================" -ForegroundColor Cyan',
'',
'$ErrorActionPreference = "Continue"',
'$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path',
'$UserProfile = $env:USERPROFILE',
'$AppData = $env:APPDATA',
'$LocalAppData = $env:LOCALAPPDATA',
'',
'Write-Host "[1/5] Claude Desktop (MS Store)" -ForegroundColor Yellow',
'Write-Host "  Opening Microsoft Store..."',
'Start-Process "ms-windows-store://pdp/?ProductId=9N9WZP3L0T9H"',
'Write-Host "  Install Claude Desktop from the Store, then press ENTER..."',
'Read-Host',
'',
'Write-Host "[2/5] Restoring Claude Desktop user data..." -ForegroundColor Yellow',
'$claude3pSrc = Join-Path $ScriptDir "Claude-3p"',
'$pkgDirs = Get-ChildItem "$LocalAppData\\Packages" -Directory -Filter "Claude_*" -ErrorAction SilentlyContinue',
'if ($pkgDirs) {',
'  $pkgDir = $pkgDirs[0].FullName',
'  $targetDir = Join-Path $pkgDir "LocalCache\\Roaming\\Claude-3p"',
'  New-Item -ItemType Directory -Path $targetDir -Force | Out-Null',
'  Copy-Item "$claude3pSrc\\*" $targetDir -Recurse -Force',
'  Write-Host "  Data restored." -ForegroundColor Green',
'} else {',
'  Write-Host "  Claude Desktop not found. Install from Store first." -ForegroundColor Red',
'}',
'',
'$cnhSrc = Join-Path $ScriptDir "ChromeNativeHost"',
'if (Test-Path $cnhSrc) {',
'  if ($pkgDirs) {',
'    $cnhTarget = Join-Path $pkgDirs[0].FullName "LocalCache\\Roaming\\Claude\\ChromeNativeHost"',
'    Copy-Item "$cnhSrc\\*" $cnhTarget -Recurse -Force',
'  }',
'}',
'',
'Write-Host "[3/5] Installing Claude Code CLI..." -ForegroundColor Yellow',
'$ccTarget = Join-Path $AppData "npm\\node_modules\\@anthropic-ai\\claude-code"',
'New-Item -ItemType Directory -Path $ccTarget -Force | Out-Null',
'Copy-Item (Join-Path $ScriptDir "claude-code-pkg\\*") $ccTarget -Recurse -Force',
'',
'$npmBinDir = Join-Path $AppData "npm"',
'New-Item -ItemType Directory -Path $npmBinDir -Force | Out-Null',
'Copy-Item (Join-Path $ScriptDir "npm-shims\\*") $npmBinDir -Force',
'',
'$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")',
'if ($userPath -notlike "*$npmBinDir*") {',
'  [Environment]::SetEnvironmentVariable("PATH", "$userPath;$npmBinDir", "User")',
'  $env:PATH = "$env:PATH;$npmBinDir"',
'}',
'',
'Write-Host "[4/5] Restoring skills and configs..." -ForegroundColor Yellow',
'Copy-Item (Join-Path $ScriptDir ".claude\\skills\\*") (Join-Path $UserProfile ".claude\\skills") -Recurse -Force',
'Copy-Item (Join-Path $ScriptDir ".agents\\skills\\*") (Join-Path $UserProfile ".agents\\skills") -Recurse -Force',
'Copy-Item (Join-Path $ScriptDir ".claude.json") $UserProfile -Force',
'if (Test-Path (Join-Path $ScriptDir ".agents\\.skill-lock.json")) {',
'  Copy-Item (Join-Path $ScriptDir ".agents\\.skill-lock.json") (Join-Path $UserProfile ".agents\\") -Force',
'}',
'Write-Host "  Skills restored." -ForegroundColor Green',
'',
'Write-Host "[5/5] Checking dependencies..." -ForegroundColor Yellow',
'try { git --version 2>&1; Write-Host "  git: OK" } catch {',
'  Write-Host "  Installing git via winget..."',
'  winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements',
'}',
'',
'Write-Host "=== Done ===" -ForegroundColor Green',
'Write-Host "  Launch Claude Desktop from Start Menu (GUI) or run: claude (terminal)"',
];
fs.writeFileSync(p.join(tmp, 'deploy.ps1'), deployPs1.join('\r\n'), 'utf8');

const readme = [
'Claude Desktop Full Migration Package',
'=====================================',
'Exported: ' + new Date().toISOString().substring(0, 16),
'Source: ' + (process.env.COMPUTERNAME || 'unknown'),
'',
'Contents:',
'- Claude-3p/         : Claude Desktop user data (sessions, configs)',
'- ChromeNativeHost/  : Chrome browser integration',
'- claude-code-pkg/   : Claude Code CLI (v2.1.126)',
'- npm-shims/         : CLI shim scripts',
'- .claude.json       : Claude Code config',
'- .claude/skills/    : Custom skills',
'- .agents/skills/    : Agent skills',
'- deploy.ps1         : Deployment script',
'',
'Deploy: Extract zip, right-click deploy.ps1 -> Run with PowerShell',
'Install Claude Desktop from MS Store when prompted.',
].join('\r\n');
fs.writeFileSync(p.join(tmp, 'README.txt'), readme, 'utf8');

// 6. ZIP
console.log('\n[6/6] Creating ZIP...');
if (fs.existsSync(outZip)) fs.rmSync(outZip, { force: true });
try {
  execSync(
    'powershell -ExecutionPolicy Bypass -Command "Compress-Archive -Path \'' + tmp + '\\*\' -DestinationPath \'' + outZip + '\' -Force"',
    { stdio: 'inherit', timeout: 600000 }
  );
} catch (e) { console.log('ZIP ERR:', e.message); }

if (fs.existsSync(outZip)) {
  console.log('\nDONE:', outZip, 'Size:', (fs.statSync(outZip).size/(1024*1024)).toFixed(0), 'MB');
} else {
  console.log('\nFAILED');
}
fs.rmSync(tmp, { recursive: true, force: true });
