const fs=require('fs');const p=require('path');
const z='C:/Users/ljm37/Desktop/deepseek-tui-full.zip';
console.log('File:',fs.existsSync(z),'Size:',(fs.statSync(z).size/(1024*1024)).toFixed(0),'MB');
// Quick check with PowerShell listing
const{execSync}=require('child_process');
try{
  const r=execSync('powershell -Command "Add-Type -A System.IO.Compression.FileSystem;[System.IO.Compression.ZipFile]::OpenRead(\\\"'+z+'\\\").Entries|?{$_.Name -match \\\"config|env\\\"}|Select Name,Length|ft -a"',{encoding:'utf8',timeout:15000});
  console.log(r);
}catch(e){console.log('list err:',e.message)}
