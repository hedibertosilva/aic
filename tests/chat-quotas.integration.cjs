// AIC_TEST_BINARY=/absolute/path/to/aic node --test tests/chat-quotas.integration.cjs
// Uses isolated config/repository and fake CLIs; never generates provider tokens.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const { spawn, execFileSync } = require('node:child_process');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'aic-quotas-'));
  const config = path.join(root, 'config'); fs.mkdirSync(config);
  execFileSync('git', ['init', '-q', root]);
  const cli = path.join(root, 'codex');
  const claude = path.join(root, 'claude');
  fs.writeFileSync(cli, `#!${process.execPath}
const fs=require('fs'),path=require('path');const root=__dirname;
const send=x=>process.stdout.write(JSON.stringify(x)+'\\n');
if(process.argv.includes('login')) { console.log('Logged in'); process.exit(0); }
if(process.argv.some(a=>a.includes('single word READY'))){ send({type:'item.completed',item:{type:'agent_message',text:'READY'}});send({type:'turn.completed'});process.exit(0); }
if(process.argv.includes('app-server')){
  require('readline').createInterface({input:process.stdin}).on('line',line=>{
    const q=JSON.parse(line);if(!q.id)return;
    if(q.method==='initialize')send({id:q.id,result:{}});
    if(q.method==='account/read')send({id:q.id,result:{account:{type:'chatgpt'}}});
    if(q.method==='account/rateLimits/read'){
      fs.appendFileSync(path.join(root,'reads'),'read\\n');
      const state=JSON.parse(fs.readFileSync(path.join(root,'quota.json')));
      setTimeout(()=>send(state.fail?{id:q.id,error:{message:'private provider error'}}:{id:q.id,result:{rateLimits:{primary:{usedPercent:state.pct,windowDurationMins:300,resetsAt:Math.floor(Date.now()/1000)+3600},secondary:{usedPercent:12,windowDurationMins:10080,resetsAt:Math.floor(Date.now()/1000)+172800}}}}),state.delay||150);
    }
  });
}else if(process.argv.includes('debug'))send({models:[]});
else{const turns=path.join(root,'turns');const n=(fs.existsSync(turns)?Number(fs.readFileSync(turns)):0)+1;fs.writeFileSync(turns,String(n));send({type:'thread.started',thread_id:'native-session'});send({type:'item.completed',item:{type:'agent_message',text:'Fixture reply'}});send({type:'turn.completed',usage:{input_tokens:100*n,cached_input_tokens:40*n,output_tokens:10*n}});}
`, { mode: 0o755 });
  // /usage is answered with captures of the real CLIs; any other call fails after starting.
  const usageReport={rate_limits:{limits:[{kind:'session',percent:0,resets_at:'2026-10-03T23:30:00+00:00',severity:'normal'},{kind:'weekly_all',percent:8,resets_at:'2026-10-09T03:00:00+00:00',severity:'normal'}]}};
  fs.writeFileSync(claude, `#!${process.execPath}
const fs=require('fs');
if(process.argv.includes('auth')){console.log(JSON.stringify({loggedIn:true}));process.exit(0);}
if(process.argv.some(a=>a.includes('single word READY'))){console.log(JSON.stringify({type:'result',subtype:'success',is_error:false,result:'READY'}));process.exit(0);}
if(process.argv.includes('/usage')){fs.appendFileSync(__dirname+'/claude-args',process.argv.slice(2).join(' ')+'\\n');console.log(JSON.stringify({type:'system',subtype:'init'}));console.log(JSON.stringify({type:'assistant',usage_report:${JSON.stringify(usageReport)}}));process.exit(0);}
process.stderr.write('fixture process failed after start\\n');process.exit(1);
`, { mode: 0o755 });
  const agyData={groups:[{name:'Gemini Models',description:'Models within this group: Gemini Flash, Gemini Pro',buckets:[{id:'gemini-weekly',window:'weekly',remaining_fraction:0,reset_time:'2026-10-08T00:35:13Z'}]},{name:'Claude and GPT models',description:'Models within this group: Claude Opus, Claude Sonnet, GPT-OSS',buckets:[{id:'3p-weekly',window:'weekly',remaining_fraction:1,reset_time:'2026-10-10T18:37:32Z'}]}]};
  const agy = path.join(root, 'agy');
  fs.writeFileSync(agy, `#!${process.execPath}\nif(process.argv.some(a=>a.includes('single word READY'))){console.log(JSON.stringify({event:'result',result:{status:'SUCCESS',response:'READY'}}));}else console.log(JSON.stringify({status:'SUCCESS',command:{name:'usage',data:${JSON.stringify(agyData)}}}));\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(root,'quota.json'), JSON.stringify({pct:34}));
  fs.writeFileSync(path.join(config,'config.toml'), `sandbox="none"\nexecutor="codex"\n[chat]\ndefault_agent="codex"\n[agents.codex]\nbinary=${JSON.stringify(cli)}\n[agents.claude]\nbinary=${JSON.stringify(claude)}\n[agents.gemini]\nbinary=${JSON.stringify(agy)}\n`);
  for (const name of ["claude", "codex", "gemini"]) execFileSync(path.resolve(process.env.AIC_TEST_BINARY), ["agent", "test", name, "--yes"], {cwd:root,env:{...process.env,AIC_CONFIG_DIR:config,AIC_IN_SANDBOX:"1"}});
  const listener=net.createServer(); await new Promise(resolve=>listener.listen(0,'127.0.0.1',resolve));
  const port=listener.address().port; await new Promise(resolve=>listener.close(resolve));
  const base=`http://127.0.0.1:${port}`; let server;
  async function start(extraEnv = {}) {
    server=spawn(path.resolve(process.env.AIC_TEST_BINARY),['chat','--no-open','--port',String(port)],{cwd:root,env:{...process.env,AIC_CONFIG_DIR:config,AIC_IN_SANDBOX:'1',...extraEnv},stdio:['ignore','ignore','pipe']});
    let errors='';server.stderr.on('data',chunk=>errors+=chunk);
    for(let i=0;i<100;i++){try{if((await fetch(base+'/api/agents')).ok)return;}catch{}if(server.exitCode!==null)throw new Error(errors);await delay(50);}
    throw new Error('Fixture server failed to start: '+errors);
  }
  async function stop(){if(server && server.exitCode===null){const done=new Promise(resolve=>server.once('exit',resolve));server.kill();await done;}}
  async function request(route,body){const res=await fetch(base+route,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});assert.ok(res.ok,await res.clone().text());return res;}
  async function snapshot(conversation='conversation'){return (await request('/api/quotas?conversation='+encodeURIComponent(conversation))).json();}
  async function settled(){for(let i=0;i<500;i++){const s=await snapshot();if(!s.agents.some(q=>q.reading))return s;await delay(30);}throw new Error('Quota read never settled');}
  await start();
  async function close() {
    // Let quota helpers finish before stopping their owner. A helper may still
    // append a fixture file after the server's process has exited.
    if (server?.exitCode === null) { try { await settled(); } catch {} }
    await stop();
    for (let attempt=0;;attempt++) {
      try { fs.rmSync(root,{recursive:true,force:true}); return; }
      catch (err) {
        if (err.code!=='ENOTEMPTY' || attempt===4) throw err;
        // Retry the entire traversal, so newly created files are enumerated.
        await delay(50);
      }
    }
  }
  return {root,base,start,stop,request,snapshot,settled,close};
}

if(require.main===module) test('quota cache and durable conversation counters work through the HTTP server', {skip:!process.env.AIC_TEST_BINARY}, async()=>{
  const f=await fixture();
  try{
    const initial=await f.settled();
    assert.deepEqual(initial.agents.map(q=>q.agent),['claude','codex','gemini']);
    const selected=await (await fetch(f.base+'/api/quotas?agents='+encodeURIComponent(JSON.stringify(['codex'])))).json();
    assert.deepEqual(selected.agents.map(q=>q.agent),['codex']);
    const codex=initial.agents.find(q=>q.agent==='codex');
    assert.equal(codex.windows[0].used_pct,34);
    const claudeQuota=initial.agents[0];
    assert.equal(claudeQuota.state,'available');
    assert.deepEqual(claudeQuota.windows.map(w=>[w.label,w.used_pct]),[['5-hour',0],['Weekly',8]]);
    assert.ok(fs.readFileSync(path.join(f.root,'claude-args'),'utf8').includes('--no-session-persistence'),'/usage must not persist a session');
    const agyQuota=initial.agents[2];
    assert.deepEqual(agyQuota.windows.map(w=>[w.bucket,w.used_pct]),[['Gemini Models',100],['Claude and GPT models',0]]);
    assert.deepEqual(agyQuota.windows[0].models,['Gemini Flash','Gemini Pro']);
    const visibleReads=fs.readFileSync(path.join(f.root,'reads'),'utf8').trim().split('\n').length;
    await f.request('/api/quotas/refresh',{agents:['claude']});await f.settled();
    assert.equal(fs.readFileSync(path.join(f.root,'reads'),'utf8').trim().split('\n').length,visibleReads,'bulk refresh must only read selected agents');
    await f.snapshot();assert.equal(fs.readFileSync(path.join(f.root,'reads'),'utf8').trim().split('\n').length,1);
    const turn=await f.request('/api/chat',{agent:'codex',conversation_id:'conversation',prompt:'hello'});
    const events=(await turn.text()).trim().split('\n').filter(Boolean).map(JSON.parse);
    assert.deepEqual(events.find(e=>e.type==='done').usage,{input_tokens:60,output_tokens:10,cache_read_tokens:40});
    let s=await f.settled();assert.equal(s.consumption.codex.turns,1);assert.equal(s.consumption.codex.usage.cost_usd,undefined);
    await f.request('/api/chat/state',{state:{convs:[],current:null},drafts:{}}); // deletion doesn't refund
    await f.request('/api/chat',{agent:'codex',conversation_id:'conversation',session_id:'native-session',prompt:'regenerate'});
    s=await f.settled();assert.equal(s.consumption.codex.usage.input_tokens,120);
    assert.deepEqual((await f.snapshot('copied-conversation')).consumption,{});
    await f.request('/api/chat',{agent:'claude',conversation_id:'conversation',prompt:'started but failed'});
    s=await f.settled();assert.equal(s.consumption.claude.missing_usage,1);
    fs.rmSync(path.join(f.root,'claude'));
    const blocked = await fetch(f.base + '/api/chat', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent:'claude',conversation_id:'conversation',prompt:'could not start'})});
    assert.equal(blocked.status,409);
    s=await f.settled();assert.equal(s.consumption.claude.turns,1);
    const fetched=s.agents.find(q=>q.agent==='codex').fetched_at;
    const readsBefore=fs.readFileSync(path.join(f.root,'reads'),'utf8').trim().split('\n').length;
    await f.request('/api/chat',{agent:'codex',prompt:'title helper'});
    await delay(250);
    assert.equal(fs.readFileSync(path.join(f.root,'reads'),'utf8').trim().split('\n').length,readsBefore);
    fs.writeFileSync(path.join(f.root,'quota.json'),JSON.stringify({fail:true,delay:1500}));
    await f.request('/api/quotas/refresh',{agent:'codex'});
    s=await f.settled();const failed=s.agents.find(q=>q.agent==='codex');
    assert.equal(failed.state,'read_error');assert.equal(failed.fetched_at,fetched);assert.equal(failed.windows[0].used_pct,34);
    assert.ok(!failed.error.includes('private'));
    assert.equal(fs.readFileSync(path.join(f.root,'reads'),'utf8').trim().split('\n').length,readsBefore+1);
    await f.stop();fs.writeFileSync(path.join(f.root,'quota.json'),JSON.stringify({pct:86}));await f.start();
    s=await f.settled();assert.equal(s.consumption.codex.turns,2);assert.equal(s.consumption.codex.usage.input_tokens,120);assert.equal(s.agents.find(q=>q.agent==='codex').windows[0].used_pct,86);
  }finally{await f.close();}
});
module.exports={fixture};
