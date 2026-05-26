// check_scripts.js - Find actual script files in skills
const fs=require('fs');
const p=require('path');
const h=require('os').homedir();
const dirs=[p.join(h,'.claude','skills'),p.join(h,'.agents','skills')];
const scripts=[];
for(const d of dirs){
  if(!fs.existsSync(d))continue;
  function w(dd,name){
    for(const f of fs.readdirSync(dd,{withFileTypes:true})){
      const fp=p.join(dd,f.name);
      if(f.isDirectory())w(fp,name);
      else if(/\.(py|js|sh|bat|ps1)$/.test(f.name)&&!f.name.includes('node_modules')){
        const s=fs.statSync(fp).size;
        scripts.push({skill:name,file:f.name,path:fp,size:s});
      }
    }
  }
  for(const e of fs.readdirSync(d,{withFileTypes:true})){
    if(e.isDirectory())w(p.join(d,e.name),e.name);
  }
}
scripts.sort((a,b)=>b.size-a.size);
for(const s of scripts){
  console.log(`${s.skill}: ${s.file} (${(s.size/1024).toFixed(1)}KB)`);
}
console.log(`\nTotal: ${scripts.length} script files`);

// Categorize by type
const byExt={};
for(const s of scripts){
  const ext=p.extname(s.file);
  byExt[ext]=(byExt[ext]||0)+1;
}
console.log('\nBy type:',JSON.stringify(byExt));
