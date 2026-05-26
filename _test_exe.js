const{execSync}=require('child_process');
const h=require('os').homedir();
const exe=h+'/AppData/Roaming/npm/node_modules/deepseek-tui/bin/downloads/deepseek.exe';
try{
  const r=execSync('"'+exe+'" --version',{encoding:'utf8',timeout:10000});
  console.log('Direct exe OK:',r.trim());
}catch(e){
  console.log('Direct FAIL:',e.message);
}
// Also try --help
try{
  const r=execSync('"'+exe+'" --help',{encoding:'utf8',timeout:10000});
  console.log('--help first 500:',r.substring(0,500));
}catch(e){
  console.log('--help FAIL:',e.message);
}
