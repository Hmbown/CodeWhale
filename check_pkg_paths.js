const fs=require('fs');const p=require('path');const h=require('os').homedir();
const pkg=p.join(h,'AppData','Roaming','npm','node_modules','deepseek-tui');
console.log('Package:',fs.existsSync(pkg));
if(fs.existsSync(pkg)){
  function getSize(dir){let t=0;for(const e of fs.readdirSync(dir,{withFileTypes:true})){const fp=p.join(dir,e.name);if(e.isDirectory())t+=getSize(fp);else t+=fs.statSync(fp).size}return t}
  console.log('Size:',(getSize(pkg)/(1024*1024)).toFixed(1),'MB');
  console.log('\nStructure:');
  function show(dir,indent){
    for(const e of fs.readdirSync(dir,{withFileTypes:true}).slice(0,30)){
      const fp=p.join(dir,e.name);
      if(e.isDirectory()){console.log(indent+'[DIR]',e.name);show(fp,indent+'  ')}
      else{const s=fs.statSync(fp).size;console.log(indent+e.name,(s>1024?(s/1024).toFixed(0)+'KB':s+'B'))}
    }
  }
  show(pkg,'');
}
// Check shim binary sizes
console.log('\nShim sizes:');
['deepseek','deepseek.cmd','deepseek-tui','deepseek-tui.cmd'].forEach(f=>{
  const fp=p.join(h,'AppData','Roaming','npm',f);
  if(fs.existsSync(fp))console.log(f,fs.statSync(fp).size,'bytes');
});
