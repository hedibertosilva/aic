// Frontend contract tests: current web assets + isolated fixture API.
// AIC_PLAYWRIGHT_MODULE=/path/to/playwright node --test tests/chat-plans.browser.cjs
// These do not claim to validate the Rust implementation or provider execution.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http'), fs = require('node:fs'), path = require('node:path');
let chromium; try { ({chromium} = require(process.env.AIC_PLAYWRIGHT_MODULE || 'playwright')); } catch {}
const options = {skip: !chromium};
const clone = value => structuredClone(value);
const participant = name => ({name, agent:'codex', role:'Review carefully.\nKeep instructions.', write:true, model:'', effort:'', session:'old-native-session', seen:0});
const makePlan = (id, checkout='/repo') => ({slug:id,title:'Plan '+id,checkout,version:1,
  content:'Objective: implement '+id+'\nAcceptance: tests pass',tasks:[{id:'T001',title:'Implement change',status:'pending',checked:false,cancelled:false,latest_execution:null}],executions:[]});

async function fixture({linked=false, history=false, old=false, team=false}={}) {
  const c={id:'source',title:'Original',participants:[participant('Dev'),...(team?[participant('Reviewer')]:[])],
    log:history?[{from:'you',text:'Old objective',round:1,t0:1},{from:'Dev',text:'Previous answer',round:1,t0:2}]:[],
    round:history?1:0,updated:Date.now(),checkout:'/repo',keepEmpty:true};
  const bindings=new Map([['source',{conversation_id:'source',checkout:'/repo',plan_slug:linked?'alpha':null,version:linked?1:0,selections:{},origin:null}]]);
  if(linked)c.plan_binding=clone(bindings.get('source'));
  const plans=new Map([['alpha',makePlan('alpha')],['beta',makePlan('beta')],['other',makePlan('other','/repo-other')]]);
  const f={saved:{state:{convs:[c],current:c.id,layout:'auto'},drafts:{}},requests:[],plans,bindings,runs:new Map(),
    fail:null,old,errors:[],revision:1,managed:linked,used:new Set()};
  const send=(res,status,data) => {res.writeHead(status,{'Content-Type':typeof data==='string'?'text/plain':'application/json'});res.end(typeof data==='string'?data:JSON.stringify(data));};
  const git=checkout=>({repository:'repo',checkout,branch:'main',sha:'abc123',label:'main'});
  const server=http.createServer(async(req,res)=>{
    try{
      const url=new URL(req.url,'http://localhost');let body=null;
      if(req.method==='POST'){let text='';for await(const chunk of req)text+=chunk;body=JSON.parse(text);}
      f.requests.push({method:req.method,path:url.pathname,body});
      if(f.fail===url.pathname)return send(res,503,'Fixture storage unavailable');
      if(url.pathname==='/api/agents')return send(res,200,{agents:['codex'],details:{codex:{kind:'codex',model:'',models:[]}},default:'codex',repo:url.searchParams.get('checkout')||'/repo',sandboxed:false});
      if(url.pathname==='/api/skills')return send(res,200,[{name:'write',requires_write:true,description:'Writes files'}]);
      if(url.pathname==='/api/quotas')return send(res,200,{agents:[],consumption:{}});
      if(url.pathname==='/api/chat/preflight'||url.pathname==='/api/diagnostics')return send(res,200,{default_agent:'codex',git:git(body?.checkout||url.searchParams.get('checkout')||'/repo'),repository:{state:'ready'},runtime:{state:'not_required'},image:{state:'not_required'},agents:[{agent:'codex',readiness:'ready'}]});
      if(url.pathname==='/api/worktrees')return send(res,200,[{checkout:'/repo',label:'main'},{checkout:'/repo-other',label:'feature'}]);
      if(url.pathname==='/api/chat/state'){
        if(body){if((body.revision!=null&&body.revision!==f.revision)||(f.managed&&body.revision==null))return send(res,409,'Snapshot changed');f.saved=clone(body);for(const item of f.saved.state.convs){if(bindings.has(item.id))item.plan_binding=clone(bindings.get(item.id));}f.revision++;return send(res,200,{revision:f.revision});}
        return send(res,200,{...f.saved,revision:f.revision});
      }
      const runRoute=url.pathname.match(/^\/api\/chat\/runs\/([^/]+)(?:\/(stop))?$/);
      if(runRoute){
        const run=f.runs.get(runRoute[1]);if(!run)return send(res,404,'run not found');
        if(runRoute[2]==='stop'){run.state='stopped';run.events.push({type:'done',ok:false,error:'Stopped.',stopped:true,seq:run.events.length+1});return send(res,202,{id:runRoute[1],state:'stopping'});}
        const after=Number(url.searchParams.get('after')||0);return send(res,200,{id:runRoute[1],state:run.state,events:run.events.filter(event=>event.seq>after)});
      }
      if(url.pathname==='/api/plans'){
        if(body){const plan=makePlan(body.slug,body.checkout);plan.title=body.title;plan.content=body.content;plan.tasks=[];plans.set(plan.slug,plan);return send(res,200,plan);}
        return send(res,200,{plans:[...plans.values()].filter(p=>p.checkout===url.searchParams.get('checkout'))});
      }
      const conversationRoute=url.pathname.match(/^\/api\/conversations\/([^/]+)\/(.+)$/);
      if(conversationRoute){
        const [,name,action]=conversationRoute;
        if(f.old)return send(res,404,'Not found');
        const binding=bindings.get(name)||{conversation_id:name,plan_slug:null,version:0,checkout:'/repo',selections:{},origin:null};
        if(!body)return send(res,200,binding);
        if(action!=='fork'&&binding.version!==body.version)return send(res,409,'Binding changed');
        if(action==='tasks'){binding.selections[body.participant]=body.task_id;binding.version++;return send(res,200,binding);}
        if(body.state_revision!==f.revision)return send(res,409,'Snapshot changed');
        const source=f.saved.state.convs.find(c=>c.id===name);
        if(!source)return send(res,400,'Source conversation was not saved');
        if(action==='plan/undo'){
          const used=f.used.has(name),original=binding.origin.conversation_id;
          if(!used){f.saved.state.convs=f.saved.state.convs.filter(c=>c.id!==name);bindings.delete(name);delete f.saved.drafts[name];}
          f.saved.state.current=original;f.revision++;return send(res,200,{undone:!used,conversation_id:original,revision:f.revision});
        }
        const forked=action==='fork'||source.log.length>0;
        if(source.log.length&&action!=='fork'&&!body.confirm_fork)return send(res,409,'History requires confirmation');
        const next=clone(source),id=forked?'fork-'+f.saved.state.convs.length:source.id;
        const slug=action==='fork'?binding.plan_slug:body.plan_slug;
        const checkout=action==='checkout'?body.checkout:binding.checkout;
        if(action==='checkout'&&body.plan_action==='migrate'){
          const copied=clone(plans.get(binding.plan_slug));copied.slug=slug;copied.checkout=checkout;copied.version=1;copied.executions=[];copied.tasks.forEach(t=>t.latest_execution=null);plans.set(slug,copied);
        }
        const nextBinding={conversation_id:id,plan_slug:slug,version:forked?1:binding.version+1,checkout,selections:{},origin:null};
        next.id=id;next.plan_binding=nextBinding;next.planTasks={};next.planMode='discussion';
        next.participants.forEach(p=>{p.session='';p.seen=next.log.length;});
        next.checkout=checkout;
        if(forked){next.title='Fork: '+source.title;nextBinding.origin={conversation_id:source.id,previous_plan:binding.plan_slug,current_plan:slug,message_count:source.log.length,checkout:binding.checkout};f.saved.state.convs.unshift(next);}
        else Object.assign(source,next);
        bindings.set(id,nextBinding);f.managed=true;f.revision++;return send(res,200,{binding:nextBinding,conversation:next,forked,revision:f.revision});
      }
      const planPath=url.pathname.match(/^\/api\/plans\/([^/]+)(?:\/(.*))?$/);
      if(planPath){
        const plan=plans.get(planPath[1]);if(!body)return send(res,200,plan);
        if(plan.version!==body.version)return send(res,409,'Plan changed');
        if(planPath[2]==='reload'){plan.content=f.externalContent||plan.content;plan.version++;return send(res,200,plan);}
        if(url.pathname.endsWith('/complete')){
          assert.equal(body.human_confirmed,true);assert.ok(body.validation.trim());
          const taskId=planPath[2].split('/')[1];plan.tasks.find(t=>t.id===taskId).status='completed';plan.tasks.find(t=>t.id===taskId).checked=true;plan.executions.filter(e=>e.task_id===taskId).forEach(e=>{e.validation=body.validation;if(e.id===body.execution_id)e.state='completed';});
        }else plan.tasks.push({id:'T'+String(plan.tasks.length+1).padStart(3,'0'),title:body.title,status:'pending'});
        plan.version++;return send(res,200,plan);
      }
      if(url.pathname==='/api/chat'){
        f.used.add(body.conversation_id);
        const events=[{type:'session',id:'new-native-session'}];
        if(body.task_id&&body.write){
          const plan=plans.get(bindings.get(body.conversation_id).plan_slug),execution={id:'exec-'+(plan.executions.length+1),task_id:body.task_id,participant:body.participant,conversation_id:body.conversation_id,state:'validation_pending',branch:'main',checkout:body.checkout,started_at:new Date().toISOString(),files:['src/example.rs'],validation:''};
          plan.executions.push(execution);const task=plan.tasks.find(t=>t.id===body.task_id);task.status='validation_pending';task.latest_execution=execution.id;plan.version++;
          events.push({type:'execution',id:execution.id});
        }
        if(body.prompt.includes('refresh-safe')){
          const run={state:'running',events:[{type:'delta',text:'Part one',seq:1}]};f.runs.set(body.run_id,run);
          res.writeHead(200,{'Content-Type':'application/x-ndjson'});res.write(JSON.stringify(run.events[0])+'\n');
          setTimeout(()=>{run.events.push({type:'delta',text:' and two',seq:2},{type:'done',ok:true,text:'Part one and two',seq:3});run.state='completed';res.end(run.events.slice(1).map(event=>JSON.stringify(event)+'\n').join(''));},700);
          return;
        }
        events.push({type:'delta',text:'Fixture reply'},{type:'done',ok:true,text:'Fixture reply'});
        if(body.run_id){events.forEach((event,index)=>event.seq=index+1);f.runs.set(body.run_id,{state:'completed',events:clone(events)});}
        res.writeHead(200,{'Content-Type':'application/x-ndjson'});return res.end(events.map(e=>JSON.stringify(e)+'\n').join(''));
      }
      const file=url.pathname==='/'?'chat.html':url.pathname.slice(1);
      if(['chat.html','chat.css','chat.js','chat-onboarding.js','chat-quotas.js','chat-import.js'].includes(file)){
        res.writeHead(200,{'Content-Type':file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'text/html'});
        return res.end(fs.readFileSync(path.join(__dirname,'../src/chat/web',file)));
      }
      return send(res,404,'Not found');
    }catch(error){f.errors.push(error.message);send(res,500,error.message);}
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  f.base='http://127.0.0.1:'+server.address().port;
  f.close=()=>new Promise(resolve=>{server.close(resolve);server.closeAllConnections();});
  return f;
}
async function setup(config={},viewport) {
  const f=await fixture(config); let browser;
  try { browser=await chromium.launch({headless:true}); } catch(error) { await f.close(); throw error; }
  const page=await browser.newPage({viewport});
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.goto(f.base);await page.waitForSelector('#chips .chip');
  await page.waitForFunction(()=>!!cur()?.planBinding||!!document.querySelector('#planlabel').textContent.includes('Free'));
  if(config.linked)await page.waitForFunction(()=>document.querySelector('#planlabel').textContent.includes('Plan alpha'));
  return{f,page,errors,close:async()=>{await browser.close();await f.close();}};
}
const current=page=>page.evaluate(()=>structuredClone(cur()));
const mutationRequests=f=>f.requests.filter(r=>r.method==='POST'&&/^\/api\/conversations\/[^/]+\/plan$/.test(r.path));
async function chooser(page,mode='existing'){
  await page.locator('#planlink').click();await page.waitForSelector('#plandialog[open]');
  await page.waitForFunction(()=>!document.querySelector('#planchoice').disabled&&planEdit?.loading===false);
  await page.locator('#planchoice').selectOption(mode);
}
async function link(page,id){await chooser(page);await page.locator('#existingplan').selectOption(id);await page.locator('#planapply').click();await page.waitForSelector('#plandialog',{state:'hidden'});await page.waitForFunction(()=>document.querySelector('#planlabel').textContent.startsWith('Plan: Plan'));}
async function reply(page,text){await page.locator('#input').fill(text);await page.locator('#send').click();await page.waitForFunction(()=>cur().log.some(e=>e.text==='Fixture reply'&&!e.pending));}

test('free conversation writes without a plan; Cancel and Esc never create/link plans',options,async()=>{
  const t=await setup();try{
    assert.equal(await t.page.locator('dialog[open]').count(),0);
    await t.page.locator('#input').fill('My unsent draft');const before=await current(t.page);
    await chooser(t.page,'create');await t.page.locator('#newplantitle').fill('Discard me');await t.page.locator('#newplanobjective').fill('Unused objective');
    await t.page.locator('#plancancel').click();assert.deepEqual(await current(t.page),before);
    await chooser(t.page);await t.page.locator('#existingplan').selectOption('alpha');await t.page.keyboard.press('Escape');
    assert.equal(mutationRequests(t.f).length,0);assert.equal(await t.page.locator('#input').inputValue(),'My unsent draft');
    await reply(t.page,'Free work');const request=t.f.requests.find(r=>r.path==='/api/chat').body;
    assert.equal(request.write,true);assert.equal(request.plan_id,undefined);assert.equal(request.task_id,undefined);assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('empty chat creates a shared plan, adds a quick task, and retains link after reload',options,async()=>{
  const t=await setup();try{
    await chooser(t.page,'create');await t.page.locator('#newplantitle').fill('Authentication');await t.page.locator('#newplanobjective').fill('Authenticate without editing TOML');await t.page.locator('#newplancriteria').fill('Login test passes');
    assert.equal(await t.page.locator('#newplanslug').inputValue(),'authentication');
    await t.page.locator('#planapply').click();await t.page.waitForSelector('#plandialog',{state:'hidden'});
    assert.equal((await current(t.page)).id,'source');assert.equal((await current(t.page)).planBinding.plan_id,'authentication');
    await t.page.locator('#plantoggle').click();await t.page.locator('#quicktask').fill('Add login test');await t.page.locator('#quicktaskadd').click();
    await t.page.waitForFunction(()=>document.querySelector('#plantasks').textContent.includes('Add login test'));
    assert.match(await t.page.locator('#plancontent').textContent(),/Authenticate without editing TOML/);
    await t.page.waitForTimeout(700);await t.page.reload();await t.page.waitForFunction(()=>document.querySelector('#planlabel').textContent.includes('Authentication'));
    assert.equal(await t.page.locator('#planmode').inputValue(),'discussion');assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('changing plan with history forks only after confirmation and preserves original draft and link',options,async()=>{
  const t=await setup({linked:true,history:true});try{
    await t.page.locator('#planmode').selectOption('task');await t.page.locator('#plantargets select').selectOption('T001');await t.page.locator('#input').fill('Original draft');
    const before=await current(t.page);
    await chooser(t.page);await t.page.locator('#existingplan').selectOption('beta');assert.match(await t.page.locator('#planchangeeffect').innerText(),/agent sessions restart/);
    assert.equal(await t.page.locator('#planapply').innerText(),'Create fork and change plan');await t.page.locator('#planapply').click();await t.page.waitForSelector('#plandialog',{state:'hidden'});
    const fork=await current(t.page);assert.notEqual(fork.id,'source');assert.deepEqual(fork.log,before.log);assert.equal(fork.checkout,before.checkout);
    assert.equal(fork.planBinding.plan_id,'beta');assert.deepEqual(fork.planTasks,{});assert.equal(fork.participants[0].session,'');assert.equal(fork.participants[0].role,before.participants[0].role);
    assert.match(await t.page.locator('#planorigin').textContent(),/previous plan: alpha/);
    const original=await t.page.evaluate(()=>state.convs.find(c=>c.id==='source'));assert.equal(original.planBinding.plan_id,'alpha');assert.equal(original.participants[0].session,'old-native-session');
    assert.equal(await t.page.locator('#input').inputValue(),'');await t.page.locator('#undo').click();await t.page.waitForFunction(()=>cur().id==='source');
    assert.equal((await current(t.page)).id,'source');assert.equal(await t.page.locator('#input').inputValue(),'Original draft');assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('discussion overrides write permission; execution requires tasks and completion requires validation',options,async()=>{
  const t=await setup({linked:true});try{
    await reply(t.page,'Discuss approach');const discussion=t.f.requests.find(r=>r.path==='/api/chat').body;
    assert.equal(discussion.write,false);assert.equal(discussion.intent,'discussion');assert.ok(Number.isSafeInteger(discussion.binding_version));assert.equal(discussion.task_id,null);
    await t.page.locator('#planmode').selectOption('task');await t.page.locator('#input').fill('Execute change');const count=t.f.requests.filter(r=>r.path==='/api/chat').length;
    await t.page.locator('#send').click();await t.page.waitForFunction(()=>document.querySelector('#toasttext').textContent.includes('Select an available task'));
    assert.equal(await t.page.locator('#input').inputValue(),'Execute change');assert.equal(t.f.requests.filter(r=>r.path==='/api/chat').length,count);
    await t.page.locator('#plantargets select').selectOption('T001');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().log.some(e=>e.execution_id));
    const execution=t.f.requests.filter(r=>r.path==='/api/chat').at(-1).body;assert.equal(execution.task_id,'T001');assert.equal(execution.participant,'Dev');assert.equal(execution.write,true);
    assert.equal(t.f.plans.get('alpha').tasks[0].status,'validation_pending');
    await t.page.locator('#plantoggle').click();await t.page.locator('#plantasks button').click();await t.page.locator('#taskcompleteapply').click();
    assert.equal(t.f.plans.get('alpha').tasks[0].status,'validation_pending','empty confirmation cannot complete a task');
    await t.page.locator('#taskvalidation').fill('Ran tests; all 5 passed.');await t.page.locator('#taskconfirmed').check();await t.page.locator('#taskcompleteapply').click();
    await t.page.waitForSelector('#taskcompletedialog',{state:'hidden'});assert.equal(t.f.plans.get('alpha').tasks[0].status,'completed');assert.match(await t.page.locator('#planexecutions').innerText(),/Completed/);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('binding failure and task revision conflicts preserve input and require refresh',options,async()=>{
  const t=await setup({linked:true});try{
    await t.page.locator('#input').fill('Preserved draft');await chooser(t.page);await t.page.locator('#existingplan').selectOption('beta');t.f.fail='/api/conversations/source/plan';
    await t.page.locator('#planapply').click();await t.page.waitForFunction(()=>!document.querySelector('#planlinkerror').hidden);
    assert.equal((await current(t.page)).planBinding.plan_id,'alpha');assert.equal(await t.page.locator('#input').inputValue(),'Preserved draft');
    t.f.fail=null;await t.page.locator('#plancancel').click();await t.page.locator('#plantoggle').click();await t.page.locator('#planrefresh').click();await t.page.waitForFunction(()=>!document.querySelector('#planrefresh').disabled);t.f.plans.get('alpha').version++;
    await t.page.locator('#quicktask').fill('Retain this task');await t.page.locator('#quicktaskadd').click();await t.page.waitForFunction(()=>document.querySelector('#planerror').textContent.includes('Refresh'));
    assert.equal(await t.page.locator('#quicktask').inputValue(),'Retain this task');await t.page.locator('#planrefresh').click();await t.page.waitForFunction(()=>!document.querySelector('#planrefresh').disabled);
    await t.page.locator('#quicktaskadd').click();await t.page.waitForFunction(()=>document.querySelector('#plantasks').textContent.includes('Retain this task'));assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('checkout changes resolve the linked plan before changing sessions or checkout',options,async()=>{
  const t=await setup({linked:true,history:true});try{
    await t.page.locator('#conversationadvanced summary').click();await t.page.locator('#worktreebtn').click();await t.page.locator('#worktreeselect').selectOption('/repo-other');await t.page.locator('#worktreeconfirm').click();
    await t.page.waitForSelector('#plandialog[open]');await t.page.waitForFunction(()=>planEdit?.loading===false);
    assert.match(await t.page.locator('#plancheckoutnote').innerText(),/files are not migrated/);
    assert.equal((await current(t.page)).checkout,'/repo');await t.page.locator('#plancancel').click();assert.equal((await current(t.page)).checkout,'/repo');
    await t.page.locator('#conversationadvanced summary').click();await t.page.locator('#worktreebtn').click();await t.page.locator('#worktreeselect').selectOption('/repo-other');await t.page.locator('#worktreeconfirm').click();
    await t.page.waitForFunction(()=>planEdit?.loading===false);await t.page.locator('#planchoice').selectOption('existing');await t.page.locator('#existingplan').selectOption('other');await t.page.locator('#planapply').click();
    await t.page.waitForSelector('#plandialog',{state:'hidden'});assert.equal((await current(t.page)).checkout,'/repo-other');assert.equal((await current(t.page)).planBinding.plan_id,'other');assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('planned fork retains its plan and clears tasks; switching to free chat also forks',options,async()=>{
  const t=await setup({linked:true,history:true});try{
    await t.page.locator('#input').fill('/fork');await t.page.locator('#send').click();await t.page.waitForFunction(()=>cur().id!=='source');
    assert.equal((await current(t.page)).planBinding.plan_id,'alpha');assert.deepEqual((await current(t.page)).planTasks,{});
    const linkedId=(await current(t.page)).id;await chooser(t.page,'free');await t.page.locator('#planapply').click();await t.page.waitForSelector('#plandialog',{state:'hidden'});
    assert.notEqual((await current(t.page)).id,linkedId);assert.equal((await current(t.page)).planBinding.plan_id,null);assert.equal(await t.page.locator('#planexecution').isVisible(),false);
    assert.equal(t.f.bindings.get(linkedId).plan_slug,'alpha');assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('optional plans stay usable on old servers; linked conversations block unverifiable sends',options,async()=>{
  const free=await setup({old:true});try{await reply(free.page,'Free on old server');assert.deepEqual(free.errors,[]);}finally{await free.close();}
  const linked=await setup({linked:true});try{
    linked.f.old=true;await linked.page.locator('#input').fill('Keep this draft');await linked.page.locator('#send').click();await linked.page.waitForFunction(()=>document.querySelector('#toasttext').textContent.includes('Restart aic'));
    assert.equal(linked.f.requests.filter(r=>r.path==='/api/chat').length,0);assert.equal(await linked.page.locator('#input').inputValue(),'Keep this draft');assert.deepEqual(linked.errors,[]);
  }finally{await linked.close();}
});

test('task selection is per writing participant and disallows conflicting concurrent tasks',options,async()=>{
  const t=await setup({linked:true,team:true});try{
    t.f.plans.get('alpha').tasks.push({id:'T002',title:'Review change',status:'pending'});
    await t.page.locator('#planmode').selectOption('task');await t.page.locator('#plantoggle').click();await t.page.locator('#planrefresh').click();await t.page.waitForFunction(()=>!document.querySelector('#planrefresh').disabled);
    await t.page.locator('#planclose').click();
    assert.equal(await t.page.locator('#plantargets select').count(),2);
    await t.page.getByLabel('Task for Dev',{exact:true}).selectOption('T001');await t.page.getByLabel('Task for Reviewer',{exact:true}).selectOption('T001');await t.page.locator('#input').fill('Concurrent work');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>document.querySelector('#toasttext').textContent.includes('separate tasks'));assert.equal(t.f.requests.filter(r=>r.path==='/api/chat').length,0);
    await t.page.getByLabel('Task for Reviewer',{exact:true}).selectOption('T002');await t.page.locator('#send').click();await t.page.waitForFunction(()=>!busy(cur())&&cur().log.filter(e=>e.execution_id).length===2);
    assert.deepEqual(t.f.requests.filter(r=>r.path==='/api/chat').map(r=>[r.body.participant,r.body.task_id]),[['Dev','T001'],['Reviewer','T002']]);assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('plan dialogs fit short mobile screens and display untrusted plan text safely',options,async()=>{
  const t=await setup({linked:true},{width:390,height:450});try{
    t.f.plans.get('alpha').content='<script>window.injected=true</script>\n'+ 'Long objective '.repeat(80);
    await chooser(t.page,'create');await t.page.locator('#newplantitle').fill('Mobile plan');await t.page.locator('#newplanobjective').fill('Large objective');
    const metrics=await t.page.locator('#plandialog').evaluate(d=>{const rect=d.getBoundingClientRect(),form=d.querySelector('form');return{left:rect.left,right:rect.right,top:rect.top,bottom:rect.bottom,height:innerHeight,width:innerWidth,padding:parseFloat(getComputedStyle(form).paddingLeft),scroll:d.scrollHeight>d.clientHeight};});
    assert.ok(metrics.left>=0&&metrics.right<=metrics.width&&metrics.top>=0&&metrics.bottom<=metrics.height);assert.ok(metrics.padding>=20);assert.ok(metrics.scroll);
    await t.page.keyboard.press('Escape');await t.page.locator('#plantoggle').click();await t.page.waitForFunction(()=>document.querySelector('#plancontent').textContent.includes('<script>'));
    assert.equal(await t.page.evaluate(()=>window.injected),undefined);assert.equal(await t.page.locator('#plancontent script').count(),0);
    assert.deepEqual(t.errors,[]);if(process.env.AIC_PLAN_SCREENSHOT)await t.page.screenshot({path:process.env.AIC_PLAN_SCREENSHOT});
  }finally{await t.close();}
});

test('snapshot conflicts block stale writes and preserve the composer draft through reload',options,async()=>{
  const t=await setup({linked:true});try{
    await t.page.locator('#input').fill('Work typed before another tab saved');
    t.f.saved.state.convs[0].title='Changed in another window';t.f.revision++;
    await t.page.locator('#send').click();await t.page.waitForFunction(()=>remoteConflict);
    assert.equal(t.f.requests.filter(r=>r.path==='/api/chat').length,0);
    assert.equal(await t.page.locator('#input').inputValue(),'Work typed before another tab saved');
    assert.equal(t.f.saved.state.convs[0].title,'Changed in another window');
    await t.page.reload();await t.page.waitForFunction(()=>cur()?.title==='Changed in another window');
    assert.equal(await t.page.locator('#input').inputValue(),'Work typed before another tab saved');assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('a remotely changed binding never changes the selected plan silently during send',options,async()=>{
  const t=await setup({linked:true});try{
    const binding=t.f.bindings.get('source');binding.plan_slug='beta';binding.version++;
    await t.page.locator('#input').fill('Keep my current objective');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>document.querySelector('#toasttext').textContent.includes('linked plan changed'));
    assert.equal((await current(t.page)).planBinding.plan_id,'alpha');assert.equal(await t.page.locator('#input').inputValue(),'Keep my current objective');
    assert.equal(t.f.requests.filter(r=>r.path==='/api/chat').length,0);assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('checkout migration copies the plan; external Markdown can be explicitly reloaded',options,async()=>{
  const t=await setup({linked:true,history:true});try{
    await t.page.locator('#plantoggle').click();await t.page.locator('#planpanel summary').filter({hasText:'Objective and scope'}).click();
    t.f.externalContent='Objective changed deliberately in PLAN.md';await t.page.locator('#planreload').click();await t.page.waitForFunction(()=>document.querySelector('#plancontent').textContent.includes('changed deliberately'));
    await t.page.locator('#planclose').click();await t.page.locator('#conversationadvanced summary').click();await t.page.locator('#worktreebtn').click();
    await t.page.locator('#worktreeselect').selectOption('/repo-other');await t.page.locator('#worktreeconfirm').click();await t.page.waitForFunction(()=>planEdit?.loading===false);
    await t.page.locator('#planchoice').selectOption('migrate');await t.page.locator('#migrateplanslug').fill('copied-alpha');
    assert.match(await t.page.locator('#plancheckoutnote').innerText(),/original plan stays/);
    await t.page.locator('#planapply').click();await t.page.waitForSelector('#plandialog',{state:'hidden'});
    assert.equal((await current(t.page)).planBinding.plan_id,'copied-alpha');assert.equal((await current(t.page)).checkout,'/repo-other');
    assert.equal(t.f.plans.get('alpha').checkout,'/repo');assert.equal(t.f.plans.get('copied-alpha').checkout,'/repo-other');assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('refresh reconnects to an in-flight run and replays only missing events',options,async()=>{
  const t=await setup();try{
    await t.page.locator('#input').fill('refresh-safe');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().log.some(entry=>entry.pending&&entry.text==='Part one'&&entry.run_id));
    const runId=await t.page.evaluate(()=>cur().log.find(entry=>entry.pending)?.run_id);
    await t.page.reload();await t.page.waitForSelector('#chips .chip');
    await t.page.waitForFunction(()=>cur().log.some(entry=>!entry.pending&&entry.text==='Part one and two'));
    const posts=t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat');
    assert.equal(posts.length,1,'reload must reconnect instead of starting another provider turn');
    assert.ok(t.f.requests.some(request=>request.method==='GET'&&request.path==='/api/chat/runs/'+runId));
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('live Rust API: create plan, track a writing turn, validate files and fork the shared binding',
  {skip:!chromium||!process.env.AIC_TEST_BINARY},async()=>{
    const {fixture:liveFixture}=require('./chat-quotas.integration.cjs');
    const f=await liveFixture();let browser;
    try{
      const initial=await (await f.request('/api/chat/state')).json();
      const source={id:'live-source',title:'Live work',participants:[{...participant('Dev'),session:''}],log:[],round:0,checkout:f.root,updated:Date.now(),keepEmpty:true};
      await f.request('/api/chat/state',{state:{convs:[source],current:source.id,layout:'auto'},drafts:{},revision:initial.revision});
      browser=await chromium.launch({headless:true});const page=await browser.newPage();const errors=[];
      page.on('pageerror',error=>errors.push(error.message));
      await page.route(/\/(?:chat(?:\.html|\.css|\.js|-onboarding\.js|-quotas\.js|-import\.js))?(?:\?.*)?$/,async route=>{
        const url=new URL(route.request().url());const name=url.pathname==='/'?'chat.html':url.pathname.slice(1);
        await route.fulfill({contentType:name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html',body:fs.readFileSync(path.join(__dirname,'../src/chat/web',name),'utf8')});
      });
      await page.goto(f.base);await page.waitForSelector('#chips .chip');
      await chooser(page,'create');await page.locator('#newplantitle').fill('Live frontend plan');await page.locator('#newplanobjective').fill('Track work through the real API');await page.locator('#newplancriteria').fill('Validated fixture execution');
      await page.locator('#planapply').click();await page.waitForSelector('#plandialog',{state:'hidden'});
      await page.locator('#plantoggle').click();await page.locator('#quicktask').fill('Exercise tracked execution');await page.locator('#quicktaskadd').click();await page.waitForFunction(()=>document.querySelector('#plantasks').textContent.includes('T001'));
      await page.locator('#planclose').click();await page.locator('#planmode').selectOption('task');await page.locator('#plantargets select').selectOption('T001');
      await page.waitForFunction(()=>pendingTaskSelections===0);
      await reply(page,'Execute the selected task');await page.waitForFunction(()=>!!cur().log.find(e=>e.execution_id));
      const binding=await (await f.request('/api/conversations/live-source/plan')).json();
      assert.equal(binding.plan_slug,'live-frontend-plan');assert.equal(binding.selections.Dev,'T001');
      const plan=await (await f.request('/api/plans/live-frontend-plan')).json();assert.equal(plan.tasks[0].checked,false);assert.equal(plan.executions[0].state,'validation_pending');
      await page.locator('#plantoggle').click();await page.locator('#planrefresh').click();await page.waitForFunction(()=>!document.querySelector('#planrefresh').disabled);await page.locator('#plantasks button').click();
      await page.locator('#taskvalidation').fill('Verified the isolated fixture reply and API execution record.');await page.locator('#taskconfirmed').check();await page.locator('#taskcompleteapply').click();await page.waitForSelector('#taskcompletedialog',{state:'hidden'});
      assert.match(fs.readFileSync(path.join(f.root,'.aic/plans/live-frontend-plan/TASKS.md'),'utf8'),/\[x\] T001/);
      assert.match(fs.readFileSync(path.join(f.root,'.aic/plans/live-frontend-plan/EXECUTION.md'),'utf8'),/Verified the isolated fixture reply/);
      if(process.env.AIC_PLAN_DESKTOP_SCREENSHOT)await page.screenshot({path:process.env.AIC_PLAN_DESKTOP_SCREENSHOT});
      await page.locator('#planclose').click();await page.locator('#input').fill('/fork');await page.locator('#send').click();await page.waitForFunction(()=>cur().id!=='live-source');
      const fork=await current(page);assert.equal(fork.planBinding.plan_id,'live-frontend-plan');assert.equal(fork.participants[0].session,'');assert.deepEqual(fork.planTasks,{});
      await chooser(page,'free');await page.locator('#planapply').click();await page.waitForSelector('#plandialog',{state:'hidden'});assert.equal((await current(page)).planBinding.plan_id,null);
      assert.equal((await (await f.request('/api/conversations/live-source/plan')).json()).plan_slug,'live-frontend-plan');
      assert.deepEqual(errors,[]);
    }finally{await browser?.close();await f.close();}
  });
