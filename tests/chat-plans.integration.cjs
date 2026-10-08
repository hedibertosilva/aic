// Isolated repository and simulated providers; no generation/OAuth calls.
const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs'), path=require('node:path');
const {fixture}=require('./chat-quotas.integration.cjs');
const options={skip:!process.env.AIC_TEST_BINARY};
const sample=(id='chat', log=[])=>({id,title:'Working conversation',participants:[{name:'Dev',agent:'codex',write:true,session:'',seen:0},{name:'Reviewer',agent:'claude',write:true,session:'',seen:0}],log});
const send=async(f,url,body,expected=200)=>{
  const response=await fetch(f.base+url,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
  const raw=await response.text(); assert.equal(response.status,expected,raw);
  return raw && response.headers.get('content-type')?.includes('application/json')?JSON.parse(raw):raw;
};
async function prepare(f,history=[]) {
  const saved=await send(f,'/api/chat/state',{state:{convs:[sample('chat',history)],current:'chat'},drafts:{chat:'Do not lose this draft'}});
  const p=await send(f,'/api/plans',{slug:'feature',title:'Shared feature',content:'# Shared feature\n\nImplement a reliable feature.'});
  await send(f,'/api/plans/feature/tasks',{version:p.version,title:'Implement backend'});
  return send(f,'/api/conversations/chat/plan',{version:0,state_revision:saved.revision,plan_slug:'feature',confirm_fork:history.length>0});
}

test('planned discussion, task execution and explicit validation persist through restart',options,async()=>{
 const f=await fixture();
 try {
  const linked=await prepare(f);
  const blocked=await send(f,'/api/chat',{conversation_id:'chat',participant:'Dev',binding_version:linked.binding.version,agent:'codex',prompt:'implement',write:true},409);
  assert.match(blocked.error,/task/);
  assert.equal(fs.existsSync(path.join(f.root,'turns')),false,'no provider launch without a task');
  const discussion=await send(f,'/api/chat',{conversation_id:'chat',participant:'Dev',binding_version:linked.binding.version,agent:'codex',prompt:'discuss the design',write:true,intent:'discussion'});
  assert.match(discussion,/Fixture reply/);
  let p=await send(f,'/api/plans/feature');assert.equal(p.executions.length,0,'discussion produces no edit attempt');
  const result=await send(f,'/api/chat',{conversation_id:'chat',participant:'Dev',binding_version:linked.binding.version,agent:'codex',prompt:'implement',write:true,intent:'execute',task_id:'T001'});
  const events=result.trim().split('\n').filter(Boolean).map(JSON.parse);
  const execution=events.find(e=>e.type==='execution'); assert.ok(execution.id);
  p=await send(f,'/api/plans/feature');assert.equal(p.tasks[0].checked,false);assert.equal(p.tasks[0].status,'validation_pending');
  assert.equal(p.executions[0].participant,'Dev');assert.equal(p.executions[0].conversation_id,'chat');assert.equal(p.executions[0].id,execution.id);
  assert.match(fs.readFileSync(path.join(f.root,'.aic/plans/feature/TASKS.md'),'utf8'),/\[ \] T001/);
  await send(f,'/api/plans/feature/tasks/T001/complete',{version:p.version,validation:'Agent says success'},400);
  await send(f,'/api/plans/feature/tasks/T001/complete',{version:p.version,human_confirmed:true,execution_id:execution.id,validation:'Reviewed the captured test output: all pass'});
  await f.stop();await f.start();
  p=await send(f,'/api/plans/feature');assert.equal(p.tasks[0].checked,true);assert.equal(p.executions[0].state,'completed');
  assert.match(fs.readFileSync(path.join(f.root,'.aic/plans/feature/EXECUTION.md'),'utf8'),/Reviewed the captured test output/);
  const state=await send(f,'/api/chat/state');assert.equal(state.drafts.chat,'Do not lose this draft');assert.equal(state.state.convs[0].plan_binding.plan_slug,'feature');
 }finally{await f.close();}
});

test('plan changes fork populated chats, preserve source and drafts, and version saves',options,async()=>{
 const f=await fixture();
 try {
  const linked=await prepare(f,[{from:'you',text:'Earlier discussion'}]);
  assert.equal(linked.forked,true);assert.notEqual(linked.conversation.id,'chat');
  const current=await send(f,'/api/chat/state');assert.equal(current.state.convs.length,2);assert.equal(current.drafts.chat,'Do not lose this draft');
  assert.equal(current.state.convs.find(c=>c.id==='chat').plan_binding.plan_slug,null);
  const copy=linked.conversation;
  assert.deepEqual(copy.plan_binding.selections,{});assert.ok(copy.participants.every(p=>p.session===''));
  assert.equal(copy.plan_binding.origin.conversation_id,'chat');assert.equal(copy.plan_binding.origin.message_count,1);
  await send(f,'/api/chat/state',{state:current.state,drafts:current.drafts,revision:current.revision-1},409);
  await send(f,'/api/chat/state',{state:current.state,drafts:current.drafts},409);
  const attempts=await Promise.all(['Attempt A','Attempt B'].map(title=>fetch(f.base+'/api/plans/feature/tasks',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({version:2,title})})));
  assert.deepEqual(attempts.map(r=>r.status).sort(),[200,409]);
  await Promise.all(attempts.map(r=>r.text()));
  const undo=await send(f,`/api/conversations/${copy.id}/plan/undo`,{version:copy.plan_binding.version,state_revision:current.revision});assert.equal(undo.undone,true);
  const after=await send(f,'/api/chat/state');assert.equal(after.state.convs.length,1);assert.equal(after.drafts.chat,'Do not lose this draft');
 }finally{await f.close();}
});

test('free chat writes need no plan or task and planned failures stay partial',options,async()=>{
 const f=await fixture();
 try {
  await send(f,'/api/chat/state',{state:{convs:[sample()],current:'chat'},drafts:{}});
  await send(f,'/api/chat',{conversation_id:'chat',agent:'codex',prompt:'small free change',write:true});
  assert.deepEqual((await send(f,'/api/plans')).plans,[]);
  const linked=await prepare(f);
  const failed=await send(f,'/api/chat',{conversation_id:'chat',participant:'Reviewer',binding_version:linked.binding.version,agent:'claude',prompt:'implement',write:true,task_id:'T001'});
  assert.match(failed,/"ok":false/);
  const p=await send(f,'/api/plans/feature');assert.equal(p.tasks[0].checked,false);assert.equal(p.tasks[0].status,'partial');assert.equal(p.executions[0].participant,'Reviewer');
  assert.match(fs.readFileSync(path.join(f.root,'.aic/plans/feature/TASKS.md'),'utf8'),/Reviewer · partial/);
 }finally{await f.close();}
});

test('durable run survives a disconnected response and replays missed events',options,async()=>{
 const {execFileSync}=require('node:child_process');
 const f=await fixture();try{
  const cli=path.join(f.root,'codex');
  fs.writeFileSync(cli,`#!${process.execPath}
const fs=require('fs');const send=value=>process.stdout.write(JSON.stringify(value)+'\\n');
if(process.argv.some(arg=>arg.includes('single word READY'))){send({type:'item.completed',item:{type:'agent_message',text:'READY'}});send({type:'turn.completed'});process.exit(0);}
fs.writeFileSync(__dirname+'/durable-started','yes');send({type:'thread.started',thread_id:'durable-session'});
setTimeout(()=>{send({type:'item.completed',item:{type:'agent_message',text:'Reply after refresh'}});send({type:'turn.completed'});},700);
`,{mode:0o755});
  execFileSync(path.resolve(process.env.AIC_TEST_BINARY),['agent','test','codex','--yes'],{cwd:f.root,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1'}});
  const runId='refresh-safe-run';const controller=new AbortController();
  const response=await fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},signal:controller.signal,
    body:JSON.stringify({run_id:runId,conversation_id:'chat',participant:'Dev',agent:'codex',prompt:'keep running'})});
  assert.equal(response.status,200);
  for(let i=0;i<100&&!fs.existsSync(path.join(f.root,'durable-started'));i++)await new Promise(resolve=>setTimeout(resolve,10));
  assert.ok(fs.existsSync(path.join(f.root,'durable-started')));controller.abort();
  let snapshot;
  for(let i=0;i<100;i++){
    snapshot=await (await fetch(f.base+'/api/chat/runs/'+runId)).json();
    if(snapshot.state==='completed')break;
    await new Promise(resolve=>setTimeout(resolve,25));
  }
  assert.equal(snapshot.state,'completed');assert.equal(snapshot.events.at(-1).type,'done');
  assert.ok(snapshot.events.some(event=>event.type==='delta'&&event.text==='Reply after refresh'));
  const after=await (await fetch(f.base+'/api/chat/runs/'+runId+'?after=1')).json();
  assert.ok(after.events.every(event=>event.seq>1));
 }finally{await f.close();}
});

test('plan APIs reject cross-origin, traversal, unknown participants and raw checkout changes',options,async()=>{
 const f=await fixture();
 try {
  const linked=await prepare(f);
  const forbidden=await fetch(f.base+'/api/plans',{method:'POST',headers:{'Content-Type':'application/json',Origin:'https://example.invalid'},body:JSON.stringify({slug:'bad',title:'bad'})});assert.equal(forbidden.status,403);
  await send(f,'/api/plans',{slug:'../escape',title:'bad'},400);
  await send(f,'/api/plans?checkout='+encodeURIComponent('/tmp'),undefined,400);
  await send(f,'/api/conversations/chat/tasks',{version:linked.binding.version,participant:'unknown',task_id:'T001'},404);
  const state=await send(f,'/api/chat/state');state.state.convs[0].checkout='/tmp';
  await send(f,'/api/chat/state',{state:state.state,drafts:state.drafts,revision:state.revision},409);
  assert.equal(fs.existsSync(path.join(f.root,'.aic/escape')),false);
 }finally{await f.close();}
});

test('server restart stops orphaned providers and recovers the task as partial',options,async()=>{
 const {execFileSync}=require('node:child_process');
 const f=await fixture();let stream;
 try {
  const cli=path.join(f.root,'codex');
  fs.writeFileSync(cli,fs.readFileSync(cli,'utf8').replace('else{const turns=',
    "else if(process.argv.some(a=>a.includes('block execution'))){fs.writeFileSync(root+'/provider.pid',String(process.pid));setTimeout(()=>{},30000);}else{const turns="));
  execFileSync(path.resolve(process.env.AIC_TEST_BINARY),['agent','test','codex','--yes'],{cwd:f.root,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1'}});
  const linked=await prepare(f);
  stream=fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({conversation_id:'chat',participant:'Dev',binding_version:linked.binding.version,agent:'codex',prompt:'block execution',write:true,task_id:'T001'})}).then(r=>r.text()).catch(()=>null);
  const until=Date.now()+5000;
  while(!fs.existsSync(path.join(f.root,'provider.pid'))){assert.ok(Date.now()<until,'provider never started');await new Promise(r=>setTimeout(r,20));}
  const pid=Number(fs.readFileSync(path.join(f.root,'provider.pid'),'utf8'));
  await send(f,'/api/conversations/chat/plan',{version:linked.binding.version,state_revision:linked.revision,plan_slug:null},409);
  await f.stop();await f.start();await stream;
  const p=await send(f,'/api/plans/feature');assert.equal(p.executions[0].state,'partial');assert.equal(p.tasks[0].checked,false);
  const stat=fs.existsSync(`/proc/${pid}/stat`)?fs.readFileSync(`/proc/${pid}/stat`,'utf8'):null;
  assert.ok(stat===null||stat.slice(stat.lastIndexOf(')')+2).startsWith('Z'),'orphaned provider must not continue writing');
  await send(f,'/api/chat',{conversation_id:'chat',participant:'Dev',binding_version:linked.binding.version,agent:'codex',prompt:'retry task',write:true,task_id:'T001'});
  const retry=await send(f,'/api/plans/feature');assert.equal(retry.executions.length,2);assert.equal(retry.tasks[0].status,'validation_pending');
 }finally{await f.close();await stream;}
});
