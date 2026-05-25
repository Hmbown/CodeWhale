const fs=require('fs');const p=require('path');const h=require('os').homedir();
const c=JSON.parse(fs.readFileSync(p.join(h,'.claude.json'),'utf8'));
console.log('provider:',c.provider);
console.log('baseUrl:',c.baseUrl);
console.log('model:',c.model);
// Check .claude/ for any config
const cd=p.join(h,'.claude');
function check(f){
  try{const t=fs.readFileSync(f,'utf8');if(/deepseek|provider|api.?key|base.?url/i.test(t))console.log(f+':',t.substring(0,500))}catch(e){}
}
for(const e of fs.readdirSync(cd,{withFileTypes:true})){
  if(e.isFile())check(p.join(cd,e.name));
  else if(e.name==='settings'){for(const f of fs.readdirSync(p.join(cd,e.name),{withFileTypes:true})){if(f.isFile())check(p.join(cd,e.name,f.name))}}
}
// Check env
console.log('\nEnv vars:');
['ANTHROPIC_API_KEY','ANTHROPIC_BASE_URL','ANTHROPIC_PROVIDER','ANTHROPIC_MODEL','DEEPSEEK_API_KEY'].forEach(k=>console.log(k+':',process.env[k]?'set':'not set'));
