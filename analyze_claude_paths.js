// analyze_claude_paths.js
const fs=require('fs');
const p=require('path');
const h=require('os').homedir();
const pkgDir=p.join(h,'AppData','Roaming','npm','node_modules','@anthropic-ai','claude-code');
console.log('Exists:',fs.existsSync(pkgDir));
if(!fs.existsSync(pkgDir))process.exit(1);
const pkg=JSON.parse(fs.readFileSync(p.join(pkgDir,'package.json'),'utf8'));
console.log('Main:',pkg.main,'Bin:',JSON.stringify(pkg.bin));
const entry=p.join(pkgDir,pkg.bin.claude);
if(fs.existsSync(entry)){
  const c=fs.readFileSync(entry,'utf8');
  console.log('Entry first 800:',c.substring(0,800));
}
function search(dir,pattern,depth){
  if(depth>3||!fs.existsSync(dir))return;
  const es=fs.readdirSync(dir,{withFileTypes:true});
  for(const e of es){
    const f=p.join(dir,e.name);
    if(e.isDirectory()&&!e.name.startsWith('node_modules'))search(f,pattern,depth+1);
    else if(e.isFile()&&/\.(js|ts|mjs)$/.test(e.name)){
      try{const c=fs.readFileSync(f,'utf8');if(c.includes(pattern))console.log('  MATCH:',p.relative(pkgDir,f),'for',pattern)}catch(ex){}
    }
  }
}
console.log('\n-- Searching dist/ --');
['.claude.json','.claude/skills','homedir','.claude'].forEach(pat=>search(p.join(pkgDir,'dist'),pat,0));
