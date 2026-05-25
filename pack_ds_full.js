const{execSync}=require('child_process');const fs=require('fs');const p=require('path');const h=require('os').homedir();
const tmp=p.join(require('os').tmpdir(),'ds_final2');
const outZip=p.join(h,'Desktop','deepseek-tui-full.zip');
console.log('=== Repacking with API key ===');
fs.rmSync(tmp,{recursive:true,force:true});fs.mkdirSync(tmp,{recursive:true});
const nd=p.join(h,'AppData','Roaming','npm','node_modules','deepseek-tui');
const cd=p.join(h,'.deepseek');
fs.mkdirSync(p.join(tmp,'bin'),{recursive:true});
fs.copyFileSync(p.join(nd,'bin','downloads','deepseek.exe'),p.join(tmp,'bin','deepseek.exe'));
fs.copyFileSync(p.join(nd,'bin','downloads','deepseek-tui.exe'),p.join(tmp,'bin','deepseek-tui.exe'));
fs.writeFileSync(p.join(tmp,'launch_deepseek.cmd'),'@echo off\r\n"%~dp0bin\\deepseek.exe" %*');
fs.copyFileSync(p.join(cd,'config.toml'),p.join(tmp,'config.toml'));
fs.copyFileSync(p.join(cd,'.env'),p.join(tmp,'.env'));
fs.copyFileSync(p.join(cd,'mcp.json'),p.join(tmp,'mcp.json'));
fs.cpSync(p.join(cd,'skills'),p.join(tmp,'skills'),{recursive:true});
// deploy.ps1 inline
const ps=[
'$s=Split-Path -Parent $MyInvocation.MyCommand.Path',
'$u=$env:USERPROFILE',
'$b=Join-Path $u ".deepseek\\bin"',
'New-Item -ItemType Directory -Path $b -Force|Out-Null',
'Copy-Item "$s\\bin\\*" $b -Force',
'$l=Join-Path $u "AppData\\Roaming\\npm"',
'New-Item -ItemType Directory -Path $l -Force|Out-Null',
'$exe=Join-Path $b "deepseek.exe"',
'Set-Content (Join-Path $l "deepseek.cmd") "@echo off`r`n`"`"$exe`"`" %*"',
'$p=[Environment]::GetEnvironmentVariable("PATH","User")',
'if($p -notlike "*$l*"){[Environment]::SetEnvironmentVariable("PATH","$p;$l","User")}',
'$d=Join-Path $u ".deepseek"',
'New-Item -ItemType Directory -Path $d -Force|Out-Null',
'Copy-Item "$s\\config.toml" $d -Force',
'Copy-Item "$s\\.env" $d -Force',
'Copy-Item "$s\\mcp.json" $d -Force',
'Copy-Item "$s\\skills\\*" (Join-Path $d "skills") -Recurse -Force',
'Write-Host "Done! Restart terminal, run: deepseek"',
];
fs.writeFileSync(p.join(tmp,'deploy.ps1'),ps.join('\r\n'),'utf8');
console.log('Zipping...');
if(fs.existsSync(outZip))fs.rmSync(outZip,{force:true});
execSync('powershell -ExecutionPolicy Bypass -Command "Compress-Archive -Path \''+tmp+'\\*\' -DestinationPath \''+outZip+'\' -Force"',{stdio:'inherit',timeout:300000});
console.log('DONE:',outZip,'Size:',(fs.statSync(outZip).size/(1024*1024)).toFixed(0),'MB');
fs.rmSync(tmp,{recursive:true,force:true});
