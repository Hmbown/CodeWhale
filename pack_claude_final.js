const{execSync}=require('child_process');const fs=require('fs');const p=require('path');const h=require('os').homedir();
const tmp=p.join(require('os').tmpdir(),'cc_ds');
const outZip=p.join(h,'Desktop','claudecode_full.zip');
console.log('=== Repacking Claude Code with DeepSeek key ===');
fs.rmSync(tmp,{recursive:true,force:true});fs.mkdirSync(tmp,{recursive:true});
const ad=p.join(h,'AppData','Roaming');
fs.cpSync(p.join(ad,'npm','node_modules','@anthropic-ai','claude-code'),p.join(tmp,'claude-code-pkg'),{recursive:true});
fs.copyFileSync(p.join(h,'.claude.json'),p.join(tmp,'.claude.json'));
fs.cpSync(p.join(h,'.claude','skills'),p.join(tmp,'.claude','skills'),{recursive:true});
fs.cpSync(p.join(h,'.agents','skills'),p.join(tmp,'.agents','skills'),{recursive:true});
const sl=p.join(h,'.agents','.skill-lock.json');
if(fs.existsSync(sl)){fs.mkdirSync(p.join(tmp,'.agents'),{recursive:true});fs.copyFileSync(sl,p.join(tmp,'.agents','.skill-lock.json'))}

// Write .env with DeepSeek config for Claude Code
const envContent=[
'# Claude Code -> DeepSeek API',
'ANTHROPIC_BASE_URL=https://api.deepseek.com/v1',
'ANTHROPIC_API_KEY=sk-f1bccc35f03d4e90be027a54ef02399a',
'# Optional: set default model',
'# ANTHROPIC_MODEL=deepseek-v4-pro',
'# ANTHROPIC_SMALL_FAST_MODEL=deepseek-v4-flash',
].join('\r\n');
fs.writeFileSync(p.join(tmp,'.env'),envContent,'utf8');

// Updated deploy.ps1
const dps1=[
'# Claude Code Deployment -> DeepSeek API',
'Write-Host "============================================" -ForegroundColor Cyan',
'Write-Host "  Claude Code + DeepSeek V4" -ForegroundColor Cyan',
'Write-Host "============================================" -ForegroundColor Cyan',
'',
'$ErrorActionPreference="Continue"',
'$s=Split-Path -Parent $MyInvocation.MyCommand.Path',
'$u=$env:USERPROFILE',
'$a=$env:APPDATA',
'$n=Join-Path $a "npm"',
'',
'Write-Host "[1/5] Installing Claude Code..."',
'$d=Join-Path $a "npm\\node_modules\\@anthropic-ai\\claude-code"',
'New-Item -ItemType Directory -Path $d -Force|Out-Null',
'Copy-Item "$s\\claude-code-pkg\\*" $d -Recurse -Force',
'$e=Join-Path $d "bin\\claude.exe"',
'New-Item -ItemType Directory -Path $n -Force|Out-Null',
'Set-Content (Join-Path $n "claude.cmd") "@echo off`r`n`"`"$e`"`" %*"',
'',
'Write-Host "[2/5] Adding to PATH..."',
'$p=[Environment]::GetEnvironmentVariable("PATH","User")',
'if($p -notlike "*$n*"){[Environment]::SetEnvironmentVariable("PATH","$p;$n","User");$env:PATH="$env:PATH;$n"}',
'',
'Write-Host "[3/5] Deploying config + skills..."',
'Copy-Item "$s\\.claude.json" $u -Force',
'Copy-Item "$s\\.claude\\skills\\*" (Join-Path $u ".claude\\skills") -Recurse -Force',
'Copy-Item "$s\\.agents\\skills\\*" (Join-Path $u ".agents\\skills") -Recurse -Force',
'',
'Write-Host "[4/5] Setting up DeepSeek API..."',
'Copy-Item "$s\\.env" $u -Force',
'[Environment]::SetEnvironmentVariable("ANTHROPIC_BASE_URL","https://api.deepseek.com/v1","User")',
'[Environment]::SetEnvironmentVariable("ANTHROPIC_API_KEY","sk-f1bccc35f03d4e90be027a54ef02399a","User")',
'Write-Host "  Key configured -> DeepSeek V4"',
'',
'Write-Host "[5/5] Checking git..."',
'try{$v=git --version 2>&1;if($LASTEXITCODE -eq 0){Write-Host "  git: OK"}}catch{try{winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements}catch{Write-Host "  Install git: https://git-scm.com/download/win"}}',
'',
'Write-Host "=== Done ===" -ForegroundColor Green',
'Write-Host "Restart terminal, run: claude"',
];
fs.writeFileSync(p.join(tmp,'deploy.ps1'),dps1.join('\r\n'),'utf8');

console.log('Zipping...');
if(fs.existsSync(outZip))fs.rmSync(outZip,{force:true});
execSync('powershell -ExecutionPolicy Bypass -Command "Compress-Archive -Path \''+tmp+'\\*\' -DestinationPath \''+outZip+'\' -Force"',{stdio:'inherit',timeout:300000});
console.log('DONE:',outZip,'Size:',(fs.statSync(outZip).size/(1024*1024)).toFixed(0),'MB');
fs.rmSync(tmp,{recursive:true,force:true});
