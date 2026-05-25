// pack_claude_full.js
const{execSync}=require('child_process');
const fs=require('fs');
const p=require('path');
const h=require('os').homedir();
const tmp=p.join(require('os').tmpdir(),'cc_full');
const desktop=p.join(h,'Desktop');
const outZip=p.join(desktop,'claudecode_full.zip');

console.log('=== Packing Claude Code Full ===');
fs.rmSync(tmp,{recursive:true,force:true});
fs.mkdirSync(tmp,{recursive:true});
function cp(s,d){if(!fs.existsSync(s)){console.log('MISS:',s);return}console.log('OK:',s);fs.cpSync(s,d,{recursive:true})}
const ad=p.join(h,'AppData','Roaming');

// 1-3: Copy Claude Code, config, skills (same as before)
cp(p.join(ad,'npm','node_modules','@anthropic-ai','claude-code'),p.join(tmp,'claude-code-pkg'));
cp(p.join(h,'.claude.json'),p.join(tmp,'.claude.json'));
cp(p.join(h,'.claude','skills'),p.join(tmp,'.claude','skills'));
cp(p.join(h,'.agents','skills'),p.join(tmp,'.agents','skills'));
const sl=p.join(h,'.agents','.skill-lock.json');
if(fs.existsSync(sl)){fs.mkdirSync(p.join(tmp,'.agents'),{recursive:true});fs.copyFileSync(sl,p.join(tmp,'.agents','.skill-lock.json'))}

// 4. Write deploy.ps1 (avoid template literal)
const dps1 = [
'# Claude Code Full Deployment Script',
'# Right-click deploy.ps1 -> Run with PowerShell',
'Write-Host "=== Claude Code Deployment ===" -ForegroundColor Cyan',
'$ErrorActionPreference="Continue"',
'$ScriptDir=Split-Path -Parent $MyInvocation.MyCommand.Path',
'$UserProfile=$env:USERPROFILE',
'$AppData=$env:APPDATA',
'',
'Write-Host "[1/5] Installing Claude Code..."',
'$pkgDest=Join-Path $AppData "npm\\node_modules\\@anthropic-ai\\claude-code"',
'New-Item -ItemType Directory -Path $pkgDest -Force | Out-Null',
'Copy-Item "$ScriptDir\\claude-code-pkg\\*" $pkgDest -Recurse -Force',
'',
'$npmDir=Join-Path $AppData "npm"',
'New-Item -ItemType Directory -Path $npmDir -Force | Out-Null',
'$exeSrc=Join-Path $pkgDest "bin\\claude.exe"',
'$shim=Join-Path $npmDir "claude.cmd"',
'Set-Content $shim "@`"$exeSrc`" %*"',
'',
'Write-Host "[2/5] Deploying config..."',
'Copy-Item "$ScriptDir\\.claude.json" $UserProfile -Force',
'',
'Write-Host "[3/5] Deploying skills..."',
'$cs=Join-Path $UserProfile ".claude\\skills"',
'Copy-Item "$ScriptDir\\.claude\\skills\\*" $cs -Recurse -Force',
'$as=Join-Path $UserProfile ".agents\\skills"',
'Copy-Item "$ScriptDir\\.agents\\skills\\*" $as -Recurse -Force',
'if(Test-Path "$ScriptDir\\.agents\\.skill-lock.json"){',
'  Copy-Item "$ScriptDir\\.agents\\.skill-lock.json" (Join-Path $UserProfile ".agents\\") -Force',
'}',
'',
'Write-Host "[4/5] Checking git (essential)..."',
'$gitOk=$false',
'try{$v=git --version 2>&1;if($LASTEXITCODE -eq 0){$gitOk=$true;Write-Host "  git: OK"}}catch{}',
'if(!$gitOk){',
'  Write-Host "  git not found. Trying winget install..."',
'  try{',
'    winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements',
'    Write-Host "  git installed. RESTART terminal after script."',
'  }catch{',
'    Write-Host "  FAILED. Install manually: https://git-scm.com/download/win"',
'  }',
'}',
'',
'Write-Host "[5/5] Checking Python (optional)..."',
'$pyOk=$false',
'try{$v=python --version 2>&1;if($LASTEXITCODE -eq 0){$pyOk=$true;Write-Host "  Python: OK"}}catch{}',
'if(!$pyOk){',
'  Write-Host "  Python not found (optional). winget install Python.Python.3.12"',
'}',
'',
'Write-Host ""',
'Write-Host "Next steps:" -ForegroundColor Yellow',
'Write-Host "  1. Restart terminal"',
'Write-Host "  2. Run: claude"',
'Write-Host "  3. Set API key: claude config set apiKey YOUR_KEY"',
'Write-Host ""',
'Write-Host "=== Done ===" -ForegroundColor Green'
];
fs.writeFileSync(p.join(tmp,'deploy.ps1'), dps1.join('\r\n'), 'utf8');

// README
const readme=[
'Claude Code Full Deployment Package',
'===================================',
'Exported: '+new Date().toISOString().substring(0,16),
'Source: '+(process.env.COMPUTERNAME||'unknown'),
'',
'Contents:',
'- claude-code-pkg/  : Claude Code (248MB, self-contained, Node.js embedded)',
'- .claude.json      : User config & project trust',
'- .claude/skills/   : Custom Claude skills (30+)',
'- .agents/skills/   : Agent skills (OpenClaw etc.)',
'- deploy.ps1        : One-click deployment script',
'',
'How to deploy on new Windows PC:',
'1. Extract claudecode_full.zip anywhere',
'2. Right-click deploy.ps1 -> Run with PowerShell',
'3. Restart terminal, run: claude',
'',
'What deploy.ps1 does:',
'1. Copies Claude Code to %%APPDATA%%\\npm\\node_modules\\',
'2. Creates claude.cmd shim in %%APPDATA%%\\npm\\',
'3. Deploys .claude.json and skills to your home folder',
'4. Installs git via winget (if missing) - ESSENTIAL',
'5. Checks for Python (optional - for skill companion scripts)',
'',
'Dependencies bundled IN the zip:',
'- Node.js  -> EMBEDDED in claude.exe (no external install needed)',
'- npm       -> NOT needed (Claude Code is self-contained)',
'',
'Dependencies auto-installed by script:',
'- git       -> winget install Git.Git (ESSENTIAL)',
'',
'Manual install required:',
'- Python 3  -> winget install Python.Python.3.12 (optional)',
'- GitHub CLI -> winget install GitHub.cli (optional)',
'',
'The 4 .js files in skills use Claude Code built-in JS engine (no Node.js needed).',
'The 5 .py files in skills need Python for companion scripts (optional).',
'PowerShell comes with Windows 10+, curl comes with Windows 10+.'
].join('\r\n');
fs.writeFileSync(p.join(tmp,'README.txt'), readme, 'utf8');

// ZIP
console.log('Creating zip...');
if(fs.existsSync(outZip))fs.rmSync(outZip,{force:true});
try{
    execSync('powershell -ExecutionPolicy Bypass -Command "Compress-Archive -Path \''+tmp+'\\*\' -DestinationPath \''+outZip+'\' -Force"',{stdio:'inherit',timeout:300000});
}catch(e){console.log('ZIP ERR:',e.message)}

if(fs.existsSync(outZip)){
    const sz=fs.statSync(outZip).size;
    console.log('\nDONE:',outZip,'Size:',(sz/(1024*1024)).toFixed(0),'MB');
}else{
    console.log('\nFAIL');
}
fs.rmSync(tmp,{recursive:true,force:true});
