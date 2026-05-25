const{execSync}=require('child_process');const fs=require('fs');const p=require('path');const h=require('os').homedir();
const zip=p.join(h,'Desktop','claudecode_full.zip');
const tmp=p.join(require('os').tmpdir(),'v2');fs.rmSync(tmp,{recursive:true,force:true});fs.mkdirSync(tmp);
execSync('powershell -Command "Expand-Archive -Path \''+zip+'\' -DestinationPath \''+tmp+'\' -Force"',{timeout:30000});
console.log('=== .env ===');
console.log(fs.readFileSync(p.join(tmp,'.env'),'utf8'));
console.log('=== deploy.ps1 ANTHROPIC lines ===');
fs.readFileSync(p.join(tmp,'deploy.ps1'),'utf8').split('\n').filter(l=>/ANTHROPIC|deepseek|api.*key|base.*url/i.test(l)).forEach(l=>console.log(l.trim()));
