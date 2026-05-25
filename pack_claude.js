// pack_claude.js
const{execSync}=require('child_process');
const fs=require('fs');
const p=require('path');
const h=require('os').homedir();
const tmp=p.join(require('os').tmpdir(),'cc_pack');
console.log('Home:',h);
fs.rmSync(tmp,{recursive:true,force:true});
fs.mkdirSync(tmp,{recursive:true});
function cp(s,d){if(!fs.existsSync(s)){console.log('MISS:',s);return}console.log('OK:',s);fs.cpSync(s,d,{recursive:true})}
// AppData = C:\Users\xxx\AppData\Roaming
const ad=p.join(h,'AppData','Roaming');
// 1
cp(p.join(ad,'npm','node_modules','@anthropic-ai','claude-code'),p.join(tmp,'claude-code-pkg'));
// 2
cp(p.join(h,'.claude.json'),p.join(tmp,'.claude.json'));
// 3
cp(p.join(h,'.claude','skills'),p.join(tmp,'.claude','skills'));
// 4
cp(p.join(h,'.agents','skills'),p.join(tmp,'.agents','skills'));
// 5
const sl=p.join(h,'.agents','.skill-lock.json');
if(fs.existsSync(sl)){fs.mkdirSync(p.join(tmp,'.agents'),{recursive:true});fs.copyFileSync(sl,p.join(tmp,'.agents','.skill-lock.json'))}
// README
const readme='Claude Code Deployment Package\n===============================\nExported: '+(new Date()).toISOString().substring(0,16)+'\nSource: '+process.env.COMPUTERNAME+'\n\nContents:\n- claude-code-pkg/   : npm global package @anthropic-ai/claude-code\n- .claude.json       : Claude Code user config\n- .claude/skills/    : Custom Claude skills\n- .agents/skills/    : Agent skills\n\nNOT included: sessions, backups, plans, projects, API keys\n';
fs.writeFileSync(p.join(tmp,'README.txt'),readme,'utf8');
// ZIP
const zip=p.join(h,'Desktop','claudecode.zip');
console.log('ZIP to:',zip);
try{execSync('powershell -ExecutionPolicy Bypass -Command "Compress-Archive -Path \''+tmp+'\\*\' -DestinationPath \''+zip+'\' -Force"',{stdio:'inherit',timeout:300000})}catch(e){console.log('ZIP ERR:',e.message)}
if(fs.existsSync(zip)){const sz=fs.statSync(zip).size;console.log('DONE:',zip,'Size:',(sz/(1024*1024)).toFixed(2),'MB')}else{console.log('FAIL: zip not created')}
fs.rmSync(tmp,{recursive:true,force:true})
