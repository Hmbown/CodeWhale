const{execSync}=require('child_process');
const fs=require('fs');
const p=require('path');

// Git size check
const gitDir='C:\\Program Files\\Git';
if(fs.existsSync(gitDir)){
  function getSize(dir){
    let total=0;
    try{
      const es=fs.readdirSync(dir,{withFileTypes:true});
      for(const e of es){
        const fp=p.join(dir,e.name);
        if(e.isDirectory())total+=getSize(fp);
        else total+=fs.statSync(fp).size;
      }
    }catch(ex){}
    return total;
  }
  const mb=getSize(gitDir)/(1024*1024);
  console.log('Git size:',mb.toFixed(0),'MB');
  
  // Show top-level
  const es=fs.readdirSync(gitDir,{withFileTypes:true});
  es.forEach(e=>console.log(e.isDirectory()?'[DIR]':'[FILE]',e.name));
}

// Check gh
console.log('\ngh CLI:');
try{console.log(execSync('gh --version',{encoding:'utf8'}))}catch(ex){console.log('Not installed')}
