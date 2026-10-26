const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path');
const {execFileSync,spawnSync}=require('node:child_process');
const {fixture}=require('./chat-quotas.integration.cjs');
const binary=process.env.AIC_TEST_BINARY && path.resolve(process.env.AIC_TEST_BINARY);
function cli(f,args,extra={}) {return spawnSync(binary,args,{cwd:f.root,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1',...extra},encoding:'utf8',timeout:15000});}
test('diagnostic schema, paid consent, cached setup and config invalidation', {skip:!binary}, async()=>{
 const f=await fixture();try{
  let d=JSON.parse(cli(f,['doctor','--json']).stdout);
  assert.equal(d.schema_version,1);assert.deepEqual(d.agents.map(a=>a.agent),['claude','codex','gemini']);
  const a=d.agents.find(a=>a.agent==='codex');assert.equal(a.readiness,'ready');assert.equal(a.context,'host');assert.equal(a.authentication,'authenticated');assert.ok(a.tested_at);
  assert.equal(JSON.parse(cli(f,['doctor','--json'],{CLAUDE_CONFIG_DIR:'/tmp/unused-claude-config'}).stdout).agents.find(a=>a.agent==='codex').readiness,'ready','another provider configuration must not force a paid retest');
  const cache=path.join(f.root,'.aic/diagnostics/tests.json');const before=fs.readFileSync(cache,'utf8');
  let run=cli(f,['agent','test','codex']);assert.equal(run.status,1);assert.match(run.stderr,/--yes/);assert.equal(fs.readFileSync(cache,'utf8'),before);
  assert.equal(cli(f,['setup','--agent','codex','--yes','--no-open']).status,0);
  assert.equal(fs.readFileSync(cache,'utf8'),before,'cached setup must not repeat a paid test');
  assert.equal(cli(f,['setup','--no-open']).status,1,'unattended setup must require an explicit agent and consent');
  assert.equal(cli(f,['agent','test','codex','--yes','--timeout','0']).status,1);
  fs.appendFileSync(path.join(f.root,'codex'),'\n// upgraded binary\n');
  d=JSON.parse(cli(f,['doctor','--json']).stdout);assert.equal(d.agents.find(a=>a.agent==='codex').readiness,'unknown');
  assert.equal(cli(f,['agent','test','codex','--yes']).status,0);
  const data=JSON.parse(fs.readFileSync(cache,'utf8'));data.tests.codex.result.tested_at-=90000;fs.writeFileSync(cache,JSON.stringify(data));
  assert.equal(JSON.parse(cli(f,['doctor','--json']).stdout).agents.find(a=>a.agent==='codex').readiness,'ready','a successful test does not expire merely with time');
  assert.equal(cli(f,['agent','test','unknown','--yes']).status,1);
  const env=cli(f,['agent','test','codex','--yes'],{AIC_IN_SANDBOX:'0'});assert.equal(env.status,0);
  const configFile=path.join(f.root,'config/config.toml');let config=fs.readFileSync(configFile,'utf8');fs.writeFileSync(configFile,config.replace(/runtime = "none"/,'runtime = "docker"').replace(/sandbox="none"/,'sandbox="docker"'));
  assert.equal(cli(f,['agent','test','codex','--yes'],{AIC_IN_SANDBOX:'0'}).status,1);
 }finally{await f.close();}
});
test('real failure downgrades readiness and never leaks provider stderr', {skip:!binary},async()=>{
 const f=await fixture();try{
  const turn=await f.request('/api/chat',{agent:'claude',conversation_id:'test',prompt:'fail'});assert.ok(!(await turn.text()).includes('fixture process failed'));
  let d=await (await f.request('/api/diagnostics')).json();assert.equal(d.agents.find(a=>a.agent==='claude').readiness,'blocked');
  let blocked=await fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent:'claude',prompt:'again'})});assert.equal(blocked.status,409);
  assert.equal(cli(f,['agent','test','claude','--yes']).status,0);
  assert.equal((await (await f.request('/api/diagnostics')).json()).agents.find(a=>a.agent==='claude').readiness,'ready');
  const foreign=await fetch(f.base+'/api/diagnostics/refresh',{method:'POST',headers:{Origin:'https://evil.example'}});assert.equal(foreign.status,403);
  const invalid=await fetch(f.base+'/api/diagnostics?checkout='+encodeURIComponent('/tmp'));assert.equal(invalid.status,400);
 }finally{await f.close();}
});
test('agent test handles authentication, quota, timeout and invalid response safely', {skip:!binary},async()=>{
 const f=await fixture();try{
  const codex=path.join(f.root,'codex');
  for (const [output,reason,auth] of [
   ["console.error('token expired PRIVATE_SECRET');process.exit(1)",'Login required','login_required'],
   ["console.error('quota exhausted PRIVATE_SECRET');process.exit(1)",'Quota exhausted','unknown'],
   ["console.error('network error PRIVATE_SECRET');process.exit(1)",'Provider network','unknown'],
   ["console.log('invalid output')",'Agent execution failed','unknown'],
   ["setTimeout(()=>{},10000)",'timed out','unknown'],
  ]) {
   fs.writeFileSync(codex,`#!${process.execPath}\nif(process.argv.includes('login'))process.exit(0);${output}\n`,{mode:0o755});
   const result=cli(f,['agent','test','codex','--yes','--timeout','1']);assert.equal(result.status,1);assert.match(result.stdout,new RegExp(reason));assert.ok(!(result.stdout+result.stderr).includes('PRIVATE_SECRET'));
   const d=JSON.parse(cli(f,['doctor','--json']).stdout).agents.find(a=>a.agent==='codex');assert.equal(d.authentication,auth);assert.equal(d.readiness,'blocked');
   if(reason==='Quota exhausted')assert.equal(d.quota,'exhausted');
  }
  fs.chmodSync(codex,0o644);assert.equal(JSON.parse(cli(f,['doctor','--json']).stdout).agents.find(a=>a.agent==='codex').installation,'missing');
 }finally{await f.close();}
});
test('Git context and worktree paths are validated; sessions cannot migrate silently', {skip:!binary},async()=>{
 const f=await fixture();try{
  execFileSync('git',['-c','user.name=Fixture','-c','user.email=fixture@example.test','commit','--allow-empty','-qm','initial'],{cwd:f.root});
  const before=await (await f.request('/api/git')).json();assert.ok(before.branch);assert.ok(before.sha);
  execFileSync('git',['checkout','-qb','feature'],{cwd:f.root});const next=await (await f.request('/api/git')).json();assert.equal(next.branch,'feature');
  let response=await fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent:'codex',prompt:'hello',git_context:before})});assert.equal(response.status,409);
  execFileSync('git',['checkout','--detach','-q'],{cwd:f.root});const detached=await (await f.request('/api/git')).json();assert.equal(detached.branch,null);assert.match(detached.label,/detached HEAD/);
  const worktree=path.join(f.root,'linked');execFileSync('git',['worktree','add','-qb','linked',worktree],{cwd:f.root});
  const trees=await (await f.request('/api/worktrees')).json();assert.ok(trees.some(t=>t.checkout===worktree));
  assert.equal(cli(f,['doctor','--json']).status,0);
  assert.equal(spawnSync(binary,['agent','test','codex','--yes'],{cwd:worktree,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1'},encoding:'utf8'}).status,0);
  await f.request('/api/chat',{agent:'codex',prompt:'hello'});
  response=await fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent:'codex',prompt:'other',checkout:worktree,session_id:'native-session'})});assert.equal(response.status,409);
  assert.ok(fs.existsSync(path.join(worktree,'.aic/diagnostics/tests.json')));assert.ok(fs.existsSync(path.join(f.root,'.aic/diagnostics/tests.json')));
 }finally{await f.close();}
});

test('host readiness cannot establish sandbox readiness; image changes invalidate test', {skip:!binary},async()=>{
 const f=await fixture();try{
  const config=path.join(f.root,'config/config.toml');let text=fs.readFileSync(config,'utf8').replace('sandbox="none"','sandbox="docker"');fs.writeFileSync(config,text);
  const docker=path.join(f.root,'docker');fs.writeFileSync(docker,`#!${process.execPath}
const fs=require('fs');if(process.argv.includes('inspect'))console.log(fs.readFileSync(__dirname+'/image-id','utf8'));else if(process.argv.includes('run')&&!process.argv.includes('--entrypoint')){console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'READY'}}));console.log(JSON.stringify({type:'turn.completed'}));}
`,{mode:0o755});
  fs.writeFileSync(path.join(f.root,'image-id'),'image-one');
  const env={AIC_IN_SANDBOX:'0',PATH:f.root+':'+process.env.PATH,XDG_DATA_HOME:path.join(f.root,'data')};
  let d=JSON.parse(cli(f,['doctor','--json'],env).stdout);assert.equal(d.agents.find(a=>a.agent==='codex').readiness,'unknown');
  assert.equal(cli(f,['agent','test','codex','--yes'],env).status,0);
  d=JSON.parse(cli(f,['doctor','--json'],env).stdout);assert.equal(d.agents.find(a=>a.agent==='codex').execution,'ready_sandbox');
  fs.writeFileSync(path.join(f.root,'image-id'),'image-two');d=JSON.parse(cli(f,['doctor','--json'],env).stdout);assert.equal(d.agents.find(a=>a.agent==='codex').readiness,'unknown');
 }finally{await f.close();}
});

test('interactive setup can finish later without losing an existing preference', {skip:!binary},async()=>{
 const f=await fixture();try{
  const {spawn}=require('node:child_process');const quote=s=>"'"+s.replaceAll("'","'\\''")+"'";
  const child=spawn('script',['-q','-e','-c',quote(binary)+' setup --no-open','/dev/null'],{cwd:f.root,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1'},stdio:['pipe','pipe','pipe']});
  let output='',cancelled=false;child.stdout.on('data',buf=>{output+=buf;if(output.includes('Environment')&&!cancelled){cancelled=true;child.stdin.write('\x1b');}});
  const status=await new Promise((resolve,reject)=>{const timeout=setTimeout(()=>{child.kill();reject(new Error('Setup did not respond to cancellation'));},10000);child.on('error',reject);child.on('exit',code=>{clearTimeout(timeout);resolve(code);});});
  assert.equal(status,0);assert.match(output,/Resume later with aic setup/);assert.equal(JSON.parse(cli(f,['doctor','--json']).stdout).default_agent,'codex');
 }finally{await f.close();}
});

test('send preflight validates recipients and cache without auth probes or changes to unrelated agents', {skip:!binary}, async()=>{
 const f=await fixture();try{
  for(const name of ['codex','claude']) {
   const file=path.join(f.root,name), source=fs.readFileSync(file,'utf8');
   fs.writeFileSync(file,source.replace("if(process.argv.includes('"+(name==='codex'?'login':'auth')+"'))", "if(process.argv.includes('"+(name==='codex'?'login':'auth')+"') && (require('fs').appendFileSync(__dirname+'/auth-probes','"+name+"\\n'),true))"));
   assert.equal(cli(f,['agent','test',name,'--yes']).status,0);
  }
  const probes=path.join(f.root,'auth-probes');fs.writeFileSync(probes,'');
  let report=await (await f.request('/api/chat/preflight',{agents:['codex']})).json();
  assert.deepEqual(report.agents.map(a=>a.agent),['codex']);assert.equal(report.agents[0].readiness,'ready');assert.ok(report.git);
  await f.request('/api/chat',{agent:'codex',prompt:'hello'});
  assert.equal(fs.readFileSync(probes,'utf8'),'','cached send must not run auth CLI probes');
  // A removed non-recipient cannot block a valid codex turn.
  fs.rmSync(path.join(f.root,'claude'));
  report=await (await f.request('/api/chat/preflight',{agents:['codex']})).json();assert.equal(report.agents[0].readiness,'ready');
  await f.request('/api/chat',{agent:'codex',prompt:'still works'});
  let response=await fetch(f.base+'/api/chat/preflight',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agents:['unknown']})});assert.equal(response.status,400);
  response=await fetch(f.base+'/api/chat/preflight',{method:'POST',headers:{'Content-Type':'application/json',Origin:'https://evil.example'},body:JSON.stringify({agents:['codex']})});assert.equal(response.status,403);
  // Changed binaries and missing executables block cached sends; age alone does not.
  fs.appendFileSync(path.join(f.root,'codex'),'\n// upgrade\n');
  report=await (await f.request('/api/chat/preflight',{agents:['codex']})).json();assert.equal(report.agents[0].readiness,'unknown');
  response=await fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent:'codex',prompt:'blocked'})});assert.equal(response.status,409);
  assert.equal(cli(f,['agent','test','codex','--yes']).status,0);
  const cacheFile=path.join(f.root,'.aic/diagnostics/tests.json'),data=JSON.parse(fs.readFileSync(cacheFile,'utf8'));data.tests.codex.result.tested_at-=90000;fs.writeFileSync(cacheFile,JSON.stringify(data));
  report=await (await f.request('/api/chat/preflight',{agents:['codex']})).json();assert.equal(report.agents[0].readiness,'ready');
  fs.chmodSync(path.join(f.root,'codex'),0o644);
  report=await (await f.request('/api/chat/preflight',{agents:['codex']})).json();assert.equal(report.agents[0].readiness,'blocked');
 }finally{await f.close();}
});

test('sandbox sends use cached recipient tests without launching diagnostic containers', {skip:!binary}, async()=>{
 const f=await fixture();try{
  await f.stop();
  const config=path.join(f.root,'config/config.toml');fs.writeFileSync(config,fs.readFileSync(config,'utf8').replace('sandbox="none"','sandbox="docker"'));
  const docker=path.join(f.root,'docker');fs.writeFileSync(docker,`#!${process.execPath}
const fs=require('fs');fs.appendFileSync(__dirname+'/docker-calls',process.argv.slice(2).join(' ')+'\\n');
if(process.argv.includes('inspect'))console.log(fs.readFileSync(__dirname+'/image-id','utf8'));
else if(process.argv.includes('run')&&!process.argv.includes('--entrypoint')){console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'READY'}}));console.log(JSON.stringify({type:'turn.completed'}));}
`,{mode:0o755});
  fs.writeFileSync(path.join(f.root,'image-id'),'image-one');
  const env={AIC_IN_SANDBOX:'0',PATH:f.root+':'+process.env.PATH,XDG_DATA_HOME:path.join(f.root,'data')};
  assert.equal(cli(f,['agent','test','codex','--yes'],env).status,0);
  await f.start(env);fs.writeFileSync(path.join(f.root,'docker-calls'),'');
  let report=await (await f.request('/api/chat/preflight',{agents:['codex']})).json();assert.equal(report.agents[0].execution,'ready_sandbox');
  await f.request('/api/chat',{agent:'codex',prompt:'hello'});
  let calls=fs.readFileSync(path.join(f.root,'docker-calls'),'utf8');
  assert.equal(calls.split('\n').filter(line=>line.startsWith('run ')).length,1,'only the actual response launches a container');
  assert.ok(!calls.includes('aic-check-')&&!calls.includes('--entrypoint sh'),'no diagnostic containers on send');
  fs.writeFileSync(path.join(f.root,'image-id'),'image-two');
  report=await (await f.request('/api/chat/preflight',{agents:['codex']})).json();assert.equal(report.agents[0].readiness,'unknown');
  const response=await fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent:'codex',prompt:'new image'})});assert.equal(response.status,409);
 }finally{await f.close();}
});
