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

test('Loop Engineering persists and validates implementer-to-reviewer transitions',options,async()=>{
 const {execFileSync}=require('node:child_process');
 const f=await fixture();try{
  const cli=path.join(f.root,'codex');
  fs.writeFileSync(cli,`#!${process.execPath}
const args=process.argv.slice(2).join(' ');const send=value=>process.stdout.write(JSON.stringify(value)+'\\n');
if(args.includes('single word READY')){send({type:'item.completed',item:{type:'agent_message',text:'READY'}});send({type:'turn.completed'});process.exit(0);}
send({type:'thread.started',thread_id:'loop-session'});
const text=args.includes('You are the reviewer')
  ?(args.includes('Request changes')?'Changes. <!-- aic-loop {"verdict":"changes_requested","target":"Impl"} -->':'Approved. <!-- aic-loop {"verdict":"approved"} -->')
  :'Implemented without selecting or calling a reviewer.';
send({type:'item.completed',item:{type:'agent_message',text}});send({type:'turn.completed'});
`,{mode:0o755});
  execFileSync(path.resolve(process.env.AIC_TEST_BINARY),['agent','test','codex','--yes'],{cwd:f.root,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1'}});
  const conversation={id:'loop-chat',title:'Loop',participants:[
    {name:'Impl',agent:'codex',write:true,session:'',seen:0},
    {name:'Review',agent:'codex',write:false,session:'',seen:0,role:'Reviewer'},
  ],log:[],round:0,checkout:null,updated:Date.now()};
  await send(f,'/api/chat/state',{state:{convs:[conversation],current:'loop-chat'},drafts:{}});
  const initial={id:'loop-http',state:'running',phase:'implementing',round:1,turns:0,maxRounds:2,maxTurns:4,implementer:'Impl',reviewer:'Review',current:'Impl',objective:'Build the feature'};
  const implemented=await send(f,'/api/chat',{run_id:'loop-http-1',conversation_id:'loop-chat',participant:'Impl',agent:'codex',prompt:'Start the loop',write:true,engineering_loop:initial});
  assert.match(implemented,/without selecting/);
  let snapshot=await send(f,'/api/chat/loops/loop-http');
  assert.equal(snapshot.state,'running');assert.equal(snapshot.phase,'reviewing');assert.equal(snapshot.turns,1);assert.equal(snapshot.current,'Review');
  const review={...initial,phase:'reviewing',turns:1,current:'Review'};
  const reviewed=await send(f,'/api/chat',{run_id:'loop-http-2',conversation_id:'loop-chat',participant:'Review',agent:'codex',session_id:'loop-session',prompt:'Review the implementation',write:false,engineering_loop:review});
  assert.match(reviewed,/approved/);
  snapshot=await send(f,'/api/chat/loops/loop-http');
  assert.equal(snapshot.state,'approved');assert.equal(snapshot.turns,2);assert.ok(snapshot.finished_at);
  const replay=await send(f,'/api/chat',{run_id:'loop-http-3',conversation_id:'loop-chat',participant:'Review',agent:'codex',prompt:'Run again',write:false,engineering_loop:review},409);
  assert.match(replay,/state changed|not running/);

  const limited={...initial,id:'loop-round-http',maxRounds:1,maxTurns:2};
  await send(f,'/api/chat',{run_id:'loop-round-http-1',conversation_id:'loop-chat',participant:'Impl',agent:'codex',prompt:'Start limited loop',write:true,engineering_loop:limited});
  await send(f,'/api/chat',{run_id:'loop-round-http-2',conversation_id:'loop-chat',participant:'Review',agent:'codex',session_id:'loop-session',prompt:'Request changes',write:false,
    engineering_loop:{...limited,phase:'reviewing',turns:1,reviewer:'Review',current:'Review'}});
  snapshot=await send(f,'/api/chat/loops/loop-round-http');assert.equal(snapshot.state,'awaiting_round_approval');
  await send(f,'/api/chat/loops/loop-round-http/round',{conversation_id:'other',decision:'approve'},409);
  const extra=await send(f,'/api/chat/loops/loop-round-http/round',{conversation_id:'loop-chat',decision:'approve'});
  assert.equal(extra.state,'running');assert.equal(extra.round,2);assert.equal(extra.max_rounds,2);assert.equal(extra.max_turns,4);

  const reviewFirst={id:'review-first-http',mode:'review_first',state:'running',phase:'reviewing',round:1,turns:0,
    maxRounds:2,maxTurns:4,implementer:null,reviewer:'Review',current:'Review',objective:'Review existing implementation'};
  await send(f,'/api/chat',{run_id:'review-first-http-1',conversation_id:'loop-chat',participant:'Review',agent:'codex',
    prompt:'Request changes to existing implementation',write:false,engineering_loop:reviewFirst});
  snapshot=await send(f,'/api/chat/loops/review-first-http');
  assert.equal(snapshot.state,'awaiting_implementer');assert.equal(snapshot.implementer,null);assert.equal(snapshot.mode,'review_first');
  await send(f,'/api/chat/loops/review-first-http/implementer',{conversation_id:'loop-chat',participant:'Review'},409);
  const selected=await send(f,'/api/chat/loops/review-first-http/implementer',{conversation_id:'loop-chat',participant:'Impl'});
  assert.equal(selected.state,'running');assert.equal(selected.phase,'implementing');assert.equal(selected.round,2);assert.equal(selected.current,'Impl');
 }finally{await f.close();}
});

test('worktree writer lock groups one conversation while queuing other conversations and allowing readers',options,async()=>{
 const {execFileSync}=require('node:child_process');
 const f=await fixture();try{
  const cli=path.join(f.root,'codex');
  fs.writeFileSync(cli,`#!${process.execPath}
const fs=require('fs'),path=require('path');const root=__dirname,args=process.argv.slice(2).join(' ');
const send=value=>process.stdout.write(JSON.stringify(value)+'\\n');
if(args.includes('single word READY')){send({type:'item.completed',item:{type:'agent_message',text:'READY'}});send({type:'turn.completed'});process.exit(0);}
const finish=text=>{send({type:'thread.started',thread_id:'lock-session'});send({type:'item.completed',item:{type:'agent_message',text}});send({type:'turn.completed'});};
if(args.includes('hold writer')){fs.writeFileSync(path.join(root,'writer-one-started'),'yes');const timer=setInterval(()=>{if(fs.existsSync(path.join(root,'release-writer'))){clearInterval(timer);finish('First writer done');}},20);}
else if(args.includes('second writer')){fs.writeFileSync(path.join(root,'writer-two-started'),'yes');finish('Second writer done');}
else if(args.includes('forced writer')){fs.writeFileSync(path.join(root,'forced-writer-started'),'yes');finish('Forced writer done');}
else if(args.includes('peer writer')){fs.writeFileSync(path.join(root,'peer-writer-started'),'yes');finish('Peer writer done');}
else if(args.includes('cancelled writer')){fs.writeFileSync(path.join(root,'cancelled-writer-started'),'yes');finish('Cancelled writer should not start');}
else if(args.includes('read while locked')){fs.writeFileSync(path.join(root,'reader-started'),'yes');finish('Reader done');}
else finish('Fixture reply');
`,{mode:0o755});
  execFileSync(path.resolve(process.env.AIC_TEST_BINARY),['agent','test','codex','--yes'],{cwd:f.root,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1'}});
  const post=body=>fetch(f.base+'/api/chat',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
  const waitRunEvent=async(id,predicate)=>{
    for(let i=0;i<100;i++){
      const response=await fetch(f.base+'/api/chat/runs/'+id);
      if(response.ok){const snapshot=await response.json();if(snapshot.events.some(predicate))return snapshot;}
      await new Promise(resolve=>setTimeout(resolve,10));
    }
    throw new Error('run event did not arrive for '+id);
  };
  const first=await post({run_id:'writer-one',conversation_id:'one',participant:'Writer-One',agent:'codex',prompt:'hold writer',write:true});
  assert.equal(first.status,200);
  for(let i=0;i<100&&!fs.existsSync(path.join(f.root,'writer-one-started'));i++)await new Promise(resolve=>setTimeout(resolve,10));
  assert.ok(fs.existsSync(path.join(f.root,'writer-one-started')));
  const lockPath=path.join(f.root,'.aic/chat/worktree-write.lock');
  assert.equal(JSON.parse(fs.readFileSync(lockPath,'utf8')).owners[0].participant,'Writer-One');
  const holderForce=await fetch(f.base+'/api/chat/runs/writer-one/force',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});
  assert.equal(holderForce.status,409,'a run already executing cannot be forced');

  const peer=await post({run_id:'writer-peer',conversation_id:'one',participant:'Writer-Peer',agent:'codex',prompt:'peer writer',write:true});
  assert.match(await peer.text(),/Peer writer done/);
  assert.ok(fs.existsSync(path.join(f.root,'peer-writer-started')),'writers in the same conversation run together');
  assert.equal(fs.existsSync(path.join(f.root,'release-writer')),false,'the first writer is still holding its lease');

  let openTimer;
  const secondResponse=await Promise.race([
    post({run_id:'writer-two',conversation_id:'two',participant:'Writer-Two',agent:'codex',prompt:'second writer',write:true}),
    new Promise((_,reject)=>{openTimer=setTimeout(()=>reject(new Error('queued writer stream did not open')),2000);}),
  ]);
  clearTimeout(openTimer);
  assert.equal(secondResponse.status,200);
  const secondText=secondResponse.text();
  const queued=await waitRunEvent('writer-two',event=>event.type==='waiting');
  const waiting=queued.events.find(event=>event.type==='waiting'&&event.waiting!==false);
  assert.equal(waiting.reason,'worktree_lock');assert.equal(waiting.holder,'Writer-One');
  assert.equal(fs.existsSync(path.join(f.root,'writer-two-started')),false);

  const forcedText=post({run_id:'writer-forced',conversation_id:'forced',participant:'Writer-Forced',agent:'codex',prompt:'forced writer',write:true}).then(async response=>{
    assert.equal(response.status,200);return response.text();
  });
  await waitRunEvent('writer-forced',event=>event.type==='waiting'&&event.waiting!==false);
  const force=await fetch(f.base+'/api/chat/runs/writer-forced/force',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});
  assert.equal(force.status,202);assert.equal((await force.json()).forced,true);
  assert.match(await forcedText,/Forced writer done/);
  assert.ok(fs.existsSync(path.join(f.root,'forced-writer-started')),'a forced writer starts before the holder releases');
  assert.equal(JSON.parse(fs.readFileSync(lockPath,'utf8')).owners[0].participant,'Writer-One','forcing a waiter does not alter the holder lease');
  assert.equal(fs.existsSync(path.join(f.root,'writer-two-started')),false,'other queued writers remain serialized');

  const cancelledText=post({run_id:'writer-cancelled',conversation_id:'four',participant:'Writer-Cancelled',agent:'codex',prompt:'cancelled writer',write:true}).then(response=>response.text());
  await waitRunEvent('writer-cancelled',event=>event.type==='waiting');
  const stopped=await fetch(f.base+'/api/chat/runs/writer-cancelled/stop',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});
  assert.equal(stopped.status,202);assert.match(await cancelledText,/Stopped/);
  assert.equal(fs.existsSync(path.join(f.root,'cancelled-writer-started')),false,'a queued writer can be stopped before it starts');

  const reader=await post({run_id:'reader',conversation_id:'three',participant:'Reader',agent:'codex',prompt:'read while locked',write:false});
  assert.match(await reader.text(),/Reader done/);
  assert.ok(fs.existsSync(path.join(f.root,'reader-started')),'read-only run starts while a writer holds the lock');
  assert.equal(fs.existsSync(path.join(f.root,'writer-two-started')),false);

  const holderStopped=await fetch(f.base+'/api/chat/runs/writer-one/stop',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});
  assert.equal(holderStopped.status,202);
  assert.match(await first.text(),/Stopped/);
  const second=await secondText;assert.match(second,/Second writer done/);
  assert.ok(fs.existsSync(path.join(f.root,'writer-two-started')),'cancelling the holder releases the next queued writer');
  assert.match(second,/"waiting":false/);
  for(let i=0;i<50&&fs.existsSync(lockPath);i++)await new Promise(resolve=>setTimeout(resolve,10));
  assert.equal(fs.existsSync(lockPath),false,'the writer removes its worktree flag when it finishes');
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
