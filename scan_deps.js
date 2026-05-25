// scan_deps.js
const fs=require('fs');
const p=require('path');
const h=require('os').homedir();

const dirs=[p.join(h,'.claude','skills'),p.join(h,'.agents','skills')];
const pats={python:/\bpython[23]?\b|\.py\b/gi,node:/\bnode\b|npm\b|npx\b|\.js\b/gi,git:/\bgit\b/gi,curl:/\bcurl\b/gi,gh:/\bgh\b/gi,docker:/\bdocker\b/gi,pwsh:/\bpowershell\b|\.ps1\b/gi,wechat:/\bwechat-cli\b/gi,browser:/\bchrome\b|playwright\b|selenium\b/gi,ffmpeg:/\bffmpeg\b/gi,excel:/\bopenpyxl\b|pandas\b|\.xlsx\b/gi};
const deps={};
for(const d of dirs){
  if(!fs.existsSync(d))continue;
  for(const e of fs.readdirSync(d,{withFileTypes:true})){
    if(!e.isDirectory())continue;
    const sk=p.join(d,e.name);
    function scan(f){
      let c='';
      try{c=fs.readFileSync(f,'utf8')}catch(ex){return}
      for(const [k,r] of Object.entries(pats)){
        if(r.test(c)){
          if(!deps[e.name])deps[e.name]=new Set();
          deps[e.name].add(k);
        }
      }
    }
    const smd=p.join(sk,'SKILL.md');if(fs.existsSync(smd))scan(smd);
    function w(dd){
      if(!fs.existsSync(dd))return;
      for(const f of fs.readdirSync(dd,{withFileTypes:true})){
        const fp=p.join(dd,f.name);
        if(f.isDirectory())w(fp);
        else if(/\.(py|js|sh|bat|ps1|json)$/.test(f.name))scan(fp);
      }
    }
    w(sk);
  }
}
console.log('=== Skills needing external tools ===');
for(const [k,v] of Object.entries(deps))console.log(k,':',[...v].join(','));
