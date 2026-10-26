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
  const c={id:'source',title:'Original',participants:[participant('Dev'),...(team?[participant('Reviewer'),participant('Other')]:[])],
    log:history?[{from:'you',text:'Old objective',round:1,t0:1},{from:'Dev',text:'Previous answer',round:1,t0:2}]:[],
    round:history?1:0,updated:Date.now(),checkout:'/repo',keepEmpty:true};
  const bindings=new Map([['source',{conversation_id:'source',checkout:'/repo',plan_slug:linked?'alpha':null,version:linked?1:0,selections:{},origin:null}]]);
  if(linked)c.plan_binding=clone(bindings.get('source'));
  const plans=new Map([['alpha',makePlan('alpha')],['beta',makePlan('beta')],['other',makePlan('other','/repo-other')]]);
  const paths=new Map([['source',[]]]);
  const f={saved:{state:{convs:[c],current:c.id,layout:'auto'},drafts:{}},requests:[],plans,bindings,paths,runs:new Map(),
    fail:null,missingLoop:false,old,errors:[],revision:1,managed:linked,used:new Set()};
  const send=(res,status,data) => {res.writeHead(status,{'Content-Type':typeof data==='string'?'text/plain':'application/json'});res.end(typeof data==='string'?data:JSON.stringify(data));};
  const git=checkout=>({repository:'repo',checkout,branch:'main',sha:'abc123',label:'main'});
  const server=http.createServer(async(req,res)=>{
    try{
      const url=new URL(req.url,'http://localhost');let body=null;
      if(req.method!=='GET'){let text='';for await(const chunk of req)text+=chunk;if(text)body=JSON.parse(text);}
      f.requests.push({method:req.method,path:url.pathname,body});
      if(f.fail===url.pathname)return send(res,503,'Fixture storage unavailable');
      if(url.pathname==='/api/agents')return send(res,200,{agents:['codex'],details:{codex:{kind:'codex',model:'',models:[]}},default:'codex',repo:url.searchParams.get('checkout')||'/repo',sandboxed:true});
      if(url.pathname==='/api/skills')return send(res,200,[{name:'write',requires_write:true,description:'Writes files'}]);
      if(url.pathname==='/api/quotas')return send(res,200,{agents:[],consumption:{}});
      if(url.pathname==='/api/chat/preflight'||url.pathname==='/api/diagnostics')return send(res,200,{default_agent:'codex',git:git(body?.checkout||url.searchParams.get('checkout')||'/repo'),repository:{state:'ready'},runtime:{state:'not_required'},image:{state:'not_required'},agents:[{agent:'codex',readiness:'ready'}]});
      if(url.pathname==='/api/worktrees')return send(res,200,[{checkout:'/repo',label:'main'},{checkout:'/repo-other',label:'feature'}]);
      if(url.pathname==='/api/chat/state'){
        if(body){if((body.revision!=null&&body.revision!==f.revision)||(f.managed&&body.revision==null))return send(res,409,'Snapshot changed');f.saved=clone(body);for(const item of f.saved.state.convs){if(bindings.has(item.id))item.plan_binding=clone(bindings.get(item.id));}f.revision++;return send(res,200,{revision:f.revision});}
        return send(res,200,{...f.saved,revision:f.revision});
      }
      const runRoute=url.pathname.match(/^\/api\/chat\/runs\/([^/]+)(?:\/(stop|force))?$/);
      if(runRoute){
        const run=f.runs.get(runRoute[1]);if(!run)return send(res,404,'run not found');
        if(runRoute[2]==='stop'){run.state='stopped';run.events.push({type:'done',ok:false,error:'Stopped.',stopped:true,seq:run.events.length+1});return send(res,202,{id:runRoute[1],state:'stopping'});}
        if(runRoute[2]==='force')return send(res,202,{id:runRoute[1],state:'forcing'});
        const after=Number(url.searchParams.get('after')||0);return send(res,200,{id:runRoute[1],state:run.state,events:run.events.filter(event=>event.seq>after)});
      }
      if(/^\/api\/chat\/loops\/[^/]+\/implementer$/.test(url.pathname)){
        if(f.missingLoop)return send(res,404,'loop not found');
        const conversation=f.saved.state.convs.find(item=>item.id===body.conversation_id);
        const loop=conversation?.engineeringLoop;
        const selected=conversation?.participants.find(item=>item.name===body.participant);
        if(!loop)return send(res,404,'loop not found');
        if(!selected||selected.inactive||!selected.write||selected.name===loop.reviewer)return send(res,409,'invalid implementer');
        return send(res,200,{
          state:'running',phase:'implementing',round:2,turns:1,max_rounds:3,max_turns:6,mode:'review_first',
          implementer:selected.name,reviewer:loop.reviewer,current:selected.name,reason:null,
        });
      }
      if(/^\/api\/chat\/loops\/[^/]+\/stop$/.test(url.pathname))return send(res,202,{state:'stopped'});
      if(/^\/api\/chat\/loops\/[^/]+\/round$/.test(url.pathname))return send(res,202,body.decision==='approve'
        ?{state:'running',round:2,max_rounds:2,max_turns:4}:{state:'declined',reason:'Another round was declined by the user.'});
      if(url.pathname==='/api/plans'){
        if(body){const plan=makePlan(body.slug,body.checkout);plan.title=body.title;plan.content=body.content;plan.tasks=[];plans.set(plan.slug,plan);return send(res,200,plan);}
        return send(res,200,{plans:[...plans.values()].filter(p=>p.checkout===url.searchParams.get('checkout'))});
      }
      const conversationRoute=url.pathname.match(/^\/api\/conversations\/([^/]+)\/(.+)$/);
      if(conversationRoute){
        const [,name,action]=conversationRoute;
        if(f.old)return send(res,404,'Not found');
        if(action==='paths'){
          const items=paths.get(name)||[];
          if(req.method==='GET')return send(res,200,{paths:items});
          if(req.method==='POST'){
            if(body.access==='rw'&&body.confirmed!==true)return send(res,409,{error:'Writable path access requires explicit confirmation'});
            const item={path:body.path,access:body.access||'ro'},index=items.findIndex(entry=>entry.path===item.path);
            if(index<0)items.push(item);else items[index]=item;paths.set(name,items);return send(res,200,item);
          }
          if(req.method==='DELETE'){
            const index=items.findIndex(entry=>entry.path===body.path);if(index<0)return send(res,404,{error:'Path is not mounted in this conversation'});
            items.splice(index,1);return send(res,200,{path:body.path,removed:true});
          }
        }
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
        next.id=id;next.plan_binding=nextBinding;
        next.participants.forEach(p=>{p.session='';p.seen=next.log.length;});
        next.checkout=checkout;
        if(forked){next.title='Fork: '+source.title;nextBinding.origin={conversation_id:source.id,previous_plan:binding.plan_slug,current_plan:slug,message_count:source.log.length,checkout:binding.checkout};f.saved.state.convs.unshift(next);paths.set(id,clone(paths.get(name)||[]));}
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
        const normalizedPrompt=body.prompt.toLowerCase();
        if(normalizedPrompt.includes('lock-wait')){
          const waiting={type:'waiting',reason:'worktree_lock',holder:'Writer',seq:1};
          const run={state:'running',events:[waiting]};
          const publish=()=>{
            f.runs.set(body.run_id,run);
            if(!normalizedPrompt.includes('buffered'))res.write(JSON.stringify(waiting)+'\n');
            setTimeout(()=>{const tail=[{type:'waiting',reason:'worktree_lock',waiting:false,seq:2},{type:'delta',text:'Lock acquired',seq:3},{type:'done',ok:true,text:'Lock acquired',seq:4}];run.events.push(...tail);run.state='completed';res.end(tail.map(event=>JSON.stringify(event)+'\n').join(''));},700);
          };
          res.writeHead(200,{'Content-Type':'application/x-ndjson'});
          if(normalizedPrompt.includes('buffered'))res.flushHeaders();
          if(normalizedPrompt.includes('delayed'))setTimeout(publish,700);else publish();
          return;
        }
        if(body.task_id&&body.write){
          const plan=plans.get(bindings.get(body.conversation_id).plan_slug),execution={id:'exec-'+(plan.executions.length+1),task_id:body.task_id,participant:body.participant,conversation_id:body.conversation_id,state:'validation_pending',branch:'main',checkout:body.checkout,started_at:new Date().toISOString(),files:['src/example.rs'],validation:''};
          plan.executions.push(execution);const task=plan.tasks.find(t=>t.id===body.task_id);task.status='validation_pending';task.latest_execution=execution.id;plan.version++;
          events.push({type:'execution',id:execution.id});
        }
        if(normalizedPrompt.includes('refresh-safe')){
          const run={state:'running',events:[{type:'delta',text:'Part one',seq:1}]};f.runs.set(body.run_id,run);
          res.writeHead(200,{'Content-Type':'application/x-ndjson'});res.write(JSON.stringify(run.events[0])+'\n');
          setTimeout(()=>{run.events.push({type:'delta',text:' and two',seq:2},{type:'done',ok:true,text:'Part one and two',seq:3});run.state='completed';res.end(run.events.slice(1).map(event=>JSON.stringify(event)+'\n').join(''));},700);
          return;
        }
        let reply='Fixture reply';
        if(body.engineering_loop?.phase==='implementing'){
          if(body.engineering_loop.objective==='Implement the responsive header')reply='Implemented. <!-- aic-loop {"action":"review","target":"Reviewer"} -->';
          if(body.engineering_loop.objective==='Change reviewer')reply='Implemented. <!-- aic-loop {"action":"review","target":"Other"} -->';
          if(body.engineering_loop.objective.startsWith('Review '))reply='Implemented. <!-- aic-loop {"action":"review","target":"Reviewer"} -->';
        }
        if(body.engineering_loop?.phase==='reviewing'){
          if(body.engineering_loop.objective==='Approve existing work')reply='Approved. <!-- aic-loop {"verdict":"approved"} -->';
          if(body.engineering_loop.objective==='Needs an implementer')reply=body.engineering_loop.turns===0
            ?'Changes needed. <!-- aic-loop {"verdict":"changes_requested"} -->'
            :'Approved after the fix. <!-- aic-loop {"verdict":"approved"} -->';
          if(body.engineering_loop.objective==='Review without target')reply='Changes needed. <!-- aic-loop {"verdict":"changes_requested"} -->';
          if(body.engineering_loop.objective==='Review wrong target')reply='Changes needed. <!-- aic-loop {"verdict":"changes_requested","target":"Other"} -->';
          if(body.engineering_loop.objective==='Review at limit')reply='Changes needed. <!-- aic-loop {"verdict":"changes_requested","target":"Dev"} -->';
        }
        events.push({type:'delta',text:reply},{type:'done',ok:true,text:reply});
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
async function link(page,id){await chooser(page);await page.locator('#existingplan').selectOption(id);await page.locator('#planapply').click();await page.waitForSelector('#plandialog',{state:'hidden'});await page.waitForFunction(()=>document.querySelector('#planlabel').textContent.startsWith('Plan '));}
async function reply(page,text){await page.locator('#input').fill(text);await page.locator('#send').click();await page.waitForFunction(()=>cur().log.some(e=>e.text==='Fixture reply'&&!e.pending));}

test('reply layout and three chat widths stay independent and persist globally',options,async()=>{
  const t=await setup({history:true},{width:1536,height:900});try{
    const side=t.page.getByRole('button',{name:'Replies side by side',exact:true});
    const stack=t.page.getByRole('button',{name:'Replies stacked',exact:true});
    const width=t.page.locator('#layout .chatwidth');
    assert.equal(await side.count(),1,'side-by-side remains available with one participant');
    assert.equal(await stack.count(),1,'stacked remains available with one participant');
    assert.equal(await width.getAttribute('data-level'),'1');
    assert.equal(await side.getAttribute('aria-pressed'),'true');
    assert.equal(await t.page.locator('.expandbtn').count(),1);
    assert.equal(await t.page.locator('.user .to').innerText(),'TO: Dev','legacy messages addressed to everyone list the participants');
    assert.equal(await t.page.locator('.user').evaluate(node=>getComputedStyle(node).borderRightWidth),'3px');
    const sideBySideRatio=()=>t.page.locator('.row').evaluate(node=>node.getBoundingClientRect().width/node.parentElement.getBoundingClientRect().width);
    const sideBySideLeftOffset=()=>t.page.locator('.row').evaluate(node=>node.getBoundingClientRect().left-node.parentElement.getBoundingClientRect().left);
    assert.ok(Math.abs(await sideBySideRatio()-.75)<.01,'side-by-side replies consume 75% of chat width level 1');
    assert.ok(Math.abs(await sideBySideLeftOffset())<1,'side-by-side replies align to the left');

    await width.click();
    assert.equal(await width.getAttribute('data-level'),'2');
    assert.ok(Math.abs(await sideBySideRatio()-.75)<.01,'side-by-side replies consume 75% of chat width level 2');
    await width.click();
    assert.equal(await width.getAttribute('data-level'),'3');
    assert.ok(Math.abs(await sideBySideRatio()-.75)<.01,'side-by-side replies consume 75% of chat width level 3');

    await width.click();
    assert.equal(await width.getAttribute('data-level'),'1');

    await stack.click();
    assert.equal(await stack.getAttribute('aria-pressed'),'true');
    assert.equal(await t.page.locator('.expandbtn').count(),0,'stacked replies have no per-message expansion control');
    const stacked=await t.page.locator('.wrap, .user, .row.solo').evaluateAll(nodes=>nodes.map(node=>node.getBoundingClientRect().width));
    assert.ok(stacked.every(value=>Math.abs(value-stacked[0])<1),'stacked user messages and lone replies fill the same conversation column');
    await t.page.getByRole('button',{name:'Edit and Resend'}).click();
    const editWidths=await t.page.locator('.user').evaluate(node=>{
      const textarea=node.querySelector('textarea'),style=getComputedStyle(node);
      return {actual:textarea.getBoundingClientRect().width,available:node.clientWidth-parseFloat(style.paddingLeft)-parseFloat(style.paddingRight)};
    });
    assert.ok(Math.abs(editWidths.actual-editWidths.available)<1,`stacked message editor fills the available width: ${editWidths.actual} of ${editWidths.available}`);
    await t.page.getByRole('button',{name:'Cancel'}).click();

    const widths=[],composerWidths=[];
    widths.push(await t.page.locator('.wrap').evaluate(node=>node.getBoundingClientRect().width));
    composerWidths.push(await t.page.locator('#form').evaluate(node=>node.getBoundingClientRect().width));
    await width.click();widths.push(await t.page.locator('.wrap').evaluate(node=>node.getBoundingClientRect().width));
    composerWidths.push(await t.page.locator('#form').evaluate(node=>node.getBoundingClientRect().width));
    assert.equal(await width.getAttribute('data-level'),'2');
    await width.click();widths.push(await t.page.locator('.wrap').evaluate(node=>node.getBoundingClientRect().width));
    composerWidths.push(await t.page.locator('#form').evaluate(node=>node.getBoundingClientRect().width));
    assert.equal(await width.getAttribute('data-level'),'3');
    assert.ok(widths[0]<widths[1]&&widths[1]<widths[2],`chat area grows at every level: ${widths.join(', ')}`);
    assert.ok(composerWidths[0]<composerWidths[1]&&composerWidths[1]<composerWidths[2],`composer grows at every level: ${composerWidths.join(', ')}`);
    assert.equal(await side.getAttribute('aria-pressed'),'false','width changes do not alter reply layout');

    await t.page.waitForFunction(async()=>{const saved=await(await fetch('/api/chat/state')).json();return saved.state?.layout==='stack'&&saved.state?.chatWidth===3;});
    await t.page.reload();await t.page.waitForSelector('#chips .chip');
    assert.equal(await t.page.getByRole('button',{name:'Replies stacked',exact:true}).getAttribute('aria-pressed'),'true');
    assert.equal(await t.page.locator('#layout .chatwidth').getAttribute('data-level'),'3');
    assert.equal(await t.page.locator('.expandbtn').count(),0);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('free conversation writes without a plan; Cancel and Esc never create/link plans',options,async()=>{
  const t=await setup();try{
    assert.equal(await t.page.locator('dialog[open]').count(),0);
    assert.equal(await t.page.locator('#planbar, #contextrepo, #contextcheckout').count(),0);
    assert.equal(await t.page.locator('#contextbar #branchicon, #contextbar #planicon, #contextbar #planlabel').count(),3);
    await t.page.locator('#input').fill('My unsent draft');const before=await current(t.page);
    await chooser(t.page,'create');await t.page.locator('#newplantitle').fill('Discard me');await t.page.locator('#newplanobjective').fill('Unused objective');
    await t.page.locator('#plancancel').click();assert.deepEqual(await current(t.page),before);
    await chooser(t.page);await t.page.locator('#existingplan').selectOption('alpha');await t.page.keyboard.press('Escape');
    assert.equal(mutationRequests(t.f).length,0);assert.equal(await t.page.locator('#input').inputValue(),'My unsent draft');
    await reply(t.page,'Free work');const request=t.f.requests.find(r=>r.path==='/api/chat').body;
    assert.deepEqual((await current(t.page)).log.findLast(entry=>entry.from==='you').to,['Dev']);
    assert.equal(request.write,true);assert.equal(request.plan_id,undefined);assert.equal(request.task_id,undefined);assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('native path commands stay out of chat, confirm writes and autocomplete removals',options,async()=>{
  const t=await setup();try{
    const input=t.page.locator('#input'),send=t.page.locator('#send');
    await input.fill('/path add /repo with spaces');await send.click();
    await t.page.waitForFunction(()=>document.querySelector('#input').value==='');
    assert.deepEqual(t.f.paths.get('source'),[{path:'/repo with spaces',access:'ro'}]);
    assert.equal((await current(t.page)).log.length,0,'native commands are not chat messages');

    await input.fill('/path add-rw /writable repo');await send.click();
    await t.page.waitForSelector('#pathsdialog[open]');
    assert.equal(await input.inputValue(),'/path add-rw /writable repo','the draft stays until confirmation succeeds');
    assert.match(await t.page.locator('#pathswarning').innerText(),/writable repo/);
    await t.page.locator('#pathsconfirm').click();await t.page.waitForSelector('#pathsdialog',{state:'hidden'});
    assert.deepEqual(t.f.paths.get('source'),[{path:'/repo with spaces',access:'ro'},{path:'/writable repo',access:'rw'}]);
    const write=t.f.requests.findLast(request=>request.method==='POST'&&request.path.endsWith('/paths'));
    assert.equal(write.body.confirmed,true);

    await input.fill('/path remove ');await t.page.waitForSelector('#ac.open .opt');
    assert.deepEqual(await t.page.locator('#ac .opt b').allTextContents(),['/repo with spaces','/writable repo']);
    await t.page.locator('#ac .opt').first().click();assert.equal(await input.inputValue(),'/path remove /repo with spaces');
    await send.click();await t.page.waitForFunction(()=>document.querySelector('#input').value==='');
    assert.deepEqual(t.f.paths.get('source'),[{path:'/writable repo',access:'rw'}]);

    await input.fill('/path list');await send.click();await t.page.waitForSelector('#pathsdialog[open]');
    assert.equal(await t.page.locator('#pathsrows .pathsrow code').innerText(),'/writable repo');
    assert.equal(await t.page.locator('#pathsrows .pathsaccess').innerText(),'rw');
    assert.equal((await current(t.page)).log.length,0);assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('empty chat creates a shared plan and retains the link after reload',options,async()=>{
  const t=await setup();try{
    await chooser(t.page,'create');await t.page.locator('#newplantitle').fill('Authentication');await t.page.locator('#newplanobjective').fill('Authenticate without editing TOML');await t.page.locator('#newplancriteria').fill('Login test passes');
    assert.equal(await t.page.locator('#newplanslug').inputValue(),'authentication');
    await t.page.locator('#planapply').click();await t.page.waitForSelector('#plandialog',{state:'hidden'});
    assert.equal((await current(t.page)).id,'source');assert.equal((await current(t.page)).planBinding.plan_id,'authentication');
    assert.equal(await t.page.locator('#plantoggle, #planexecution, #quicktaskform').count(),0);
    await t.page.waitForTimeout(700);await t.page.reload();await t.page.waitForFunction(()=>document.querySelector('#planlabel').textContent.includes('Authentication'));
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('changing plan with history forks only after confirmation and preserves original draft and link',options,async()=>{
  const t=await setup({linked:true,history:true});try{
    await t.page.locator('#input').fill('Original draft');
    const before=await current(t.page);
    await chooser(t.page);await t.page.locator('#existingplan').selectOption('beta');assert.match(await t.page.locator('#planchangeeffect').innerText(),/agent sessions restart/);
    assert.equal(await t.page.locator('#planapply').innerText(),'Create Fork and Change Plan');await t.page.locator('#planapply').click();await t.page.waitForSelector('#plandialog',{state:'hidden'});
    const fork=await current(t.page);assert.notEqual(fork.id,'source');assert.deepEqual(fork.log,before.log);assert.equal(fork.checkout,before.checkout);
    assert.equal(fork.planBinding.plan_id,'beta');assert.equal(fork.participants[0].session,'');assert.equal(fork.participants[0].role,before.participants[0].role);
    assert.equal(await t.page.locator('#planorigin').evaluate(node=>node.parentElement.id),'contextbar');
    assert.equal(await t.page.locator('#conversationadvanced summary svg.i, #planorigin summary svg.i').count(),2);
    await t.page.locator('#planorigin summary').click();
    assert.equal(await t.page.locator('#forkconversation').textContent(),'Original');assert.equal(await t.page.locator('#forkplan').textContent(),'alpha');assert.equal(await t.page.locator('#forkmessages').textContent(),'2 messages');
    await t.page.locator('#historyclose').click();assert.equal(await t.page.locator('#planorigin').evaluate(node=>node.open),false);
    await t.page.locator('#conversationadvanced summary').click();await t.page.keyboard.press('Escape');assert.equal(await t.page.locator('#conversationadvanced').evaluate(node=>node.open),false);
    await t.page.locator('#planorigin summary').click();await t.page.locator('#contextbranch').click();assert.equal(await t.page.locator('#planorigin').evaluate(node=>node.open),false);
    const original=await t.page.evaluate(()=>state.convs.find(c=>c.id==='source'));assert.equal(original.planBinding.plan_id,'alpha');assert.equal(original.participants[0].session,'old-native-session');
    assert.equal(await t.page.locator('#input').inputValue(),'');await t.page.locator('#undo').click();await t.page.waitForFunction(()=>cur().id==='source');
    assert.equal((await current(t.page)).id,'source');assert.equal(await t.page.locator('#input').inputValue(),'Original draft');assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('linked plans stay read only and expose no per-agent task controls',options,async()=>{
  const t=await setup({linked:true});try{
    await reply(t.page,'Discuss approach');const discussion=t.f.requests.find(r=>r.path==='/api/chat').body;
    assert.equal(discussion.write,false);assert.equal(discussion.intent,'discussion');assert.ok(Number.isSafeInteger(discussion.binding_version));assert.equal(discussion.task_id,undefined);
    assert.equal(await t.page.locator('#planmode, #plantargets, #plantoggle, #taskcompletedialog').count(),0);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('binding failures preserve the selected plan and draft',options,async()=>{
  const t=await setup({linked:true});try{
    await t.page.locator('#input').fill('Preserved draft');await chooser(t.page);await t.page.locator('#existingplan').selectOption('beta');t.f.fail='/api/conversations/source/plan';
    await t.page.locator('#planapply').click();await t.page.waitForFunction(()=>!document.querySelector('#planlinkerror').hidden);
    assert.equal((await current(t.page)).planBinding.plan_id,'alpha');assert.equal(await t.page.locator('#input').inputValue(),'Preserved draft');
    t.f.fail=null;await t.page.locator('#plancancel').click();assert.deepEqual(t.errors,[]);
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

test('planned fork retains its plan; switching to free chat also forks',options,async()=>{
  const t=await setup({linked:true,history:true});try{
    await t.page.locator('#input').fill('/fork');await t.page.locator('#send').click();await t.page.waitForFunction(()=>cur().id!=='source');
    assert.equal((await current(t.page)).planBinding.plan_id,'alpha');
    const linkedId=(await current(t.page)).id;await chooser(t.page,'free');await t.page.locator('#planapply').click();await t.page.waitForSelector('#plandialog',{state:'hidden'});
    assert.notEqual((await current(t.page)).id,linkedId);assert.equal((await current(t.page)).planBinding.plan_id,null);
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

test('linked team conversations keep every participant in discussion mode',options,async()=>{
  const t=await setup({linked:true,team:true});try{
    await reply(t.page,'Review together');
    const requests=t.f.requests.filter(r=>r.path==='/api/chat').map(r=>r.body);
    assert.equal(requests.length,3);assert.ok(requests.every(r=>r.intent==='discussion'&&r.write===false&&r.task_id===undefined));
    assert.equal(await t.page.locator('#planmode, #plantargets').count(),0);assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('plan dialog fits short mobile screens',options,async()=>{
  const t=await setup({linked:true},{width:390,height:450});try{
    t.f.plans.get('alpha').content='<script>window.injected=true</script>\n'+ 'Long objective '.repeat(80);
    await chooser(t.page,'create');await t.page.locator('#newplantitle').fill('Mobile plan');await t.page.locator('#newplanobjective').fill('Large objective');
    const metrics=await t.page.locator('#plandialog').evaluate(d=>{const rect=d.getBoundingClientRect(),form=d.querySelector('form');return{left:rect.left,right:rect.right,top:rect.top,bottom:rect.bottom,height:innerHeight,width:innerWidth,padding:parseFloat(getComputedStyle(form).paddingLeft),scroll:d.scrollHeight>d.clientHeight};});
    assert.ok(metrics.left>=0&&metrics.right<=metrics.width&&metrics.top>=0&&metrics.bottom<=metrics.height);assert.ok(metrics.padding>=20);assert.ok(metrics.scroll);
    await t.page.keyboard.press('Escape');assert.equal(await t.page.locator('#planpanel, #plancontent').count(),0);
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

test('checkout migration copies the plan',options,async()=>{
  const t=await setup({linked:true,history:true});try{
    await t.page.locator('#conversationadvanced summary').click();await t.page.locator('#worktreebtn').click();
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

test('a stopped participant reply can be deleted while other replies continue',options,async()=>{
  const t=await setup({team:true});try{
    await t.page.locator('#input').fill('lock-wait');await t.page.locator('#send').click();
    await t.page.getByRole('button',{name:'Stop Dev',exact:true}).click();
    await t.page.waitForFunction(()=>{
      const c=cur(),stopped=c.log.find(entry=>entry.from==='Dev');
      return stopped?.stopped&&!stopped.pending&&c.log.some(entry=>entry.pending);
    });
    const stoppedCard=t.page.locator('.card').filter({has:t.page.locator('.name',{hasText:'Dev'})});
    await stoppedCard.getByRole('button',{name:'Delete reply',exact:true}).click();
    assert.equal(await t.page.evaluate(()=>cur().log.some(entry=>entry.from==='Dev')),false);
    assert.ok(await t.page.evaluate(()=>cur().log.some(entry=>entry.pending)),'other participant replies keep running');
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('a writing participant shows and clears the worktree lock wait notice',options,async()=>{
  const t=await setup();try{
    await t.page.locator('#input').fill('lock-wait buffered delayed');await t.page.locator('#send').click();
    await t.page.locator('#composerlock').waitFor();
    const waitingConversation=await t.page.evaluate(()=>cur().id);
    const runId=await t.page.evaluate(()=>cur().log.find(entry=>entry.pending)?.run_id);
    assert.equal(await t.page.locator('.card .locknotice, .card .lockbadge').count(),0);
    assert.equal(await t.page.locator('.conv.active .lockworking').innerText(),'waiting for Writer');
    assert.match(await t.page.locator('#composerlock').innerText(),/Dev in “Lock-wait buffered delayed” is waiting for Writer.*only one conversation can write to a worktree at a time/s);
    await t.page.evaluate(()=>{
      const c={id:crypto.randomUUID(),title:'Foreground chat',participants:[],log:[],round:0,updated:Date.now(),sendTo:null,checkout:null};
      state.convs.unshift(c);switchTo(c.id);
    });
    assert.equal(await t.page.locator('#composerlock').isHidden(),true,'a background wait stays scoped to its conversation');
    assert.equal(await t.page.locator('.conv .lockworking').count(),1,'the sidebar still identifies the waiting conversation');
    await t.page.evaluate(id=>switchTo(id),waitingConversation);
    assert.equal(await t.page.locator('#composerlock').isVisible(),true);
    assert.match(await t.page.locator('#composerlock').innerText(),/Lock-wait buffered delayed/);
    assert.ok(t.f.requests.filter(request=>request.method==='GET'&&request.path==='/api/chat/runs/'+runId).length>=2,'startup polling retries after the run initially returns 404');
    await t.page.locator('#composerlockforce').click();
    assert.equal(await t.page.locator('#composerlock').isHidden(),true);
    assert.equal(await t.page.locator('.conv .lockworking').count(),0);
    assert.ok(t.f.requests.some(request=>request.method==='POST'&&request.path.endsWith('/force')));
    await t.page.waitForFunction(()=>state.convs.some(c=>c.log.some(entry=>!entry.pending&&entry.text==='Lock acquired')));
    assert.equal(await t.page.locator('.conv .lockworking').count(),0);
    assert.equal(await t.page.locator('#composerlock').isHidden(),true);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('Loop Engineering selects a reviewer by role, persists progress and stops explicitly',options,async()=>{
  const t=await setup({team:true});try{
    await t.page.locator('#input').fill('/loop Implement the responsive header');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>state.convs.some(c=>c.engineeringLoop?.state==='paused'&&c.engineeringLoop.turns===2));
    const loop=await t.page.evaluate(()=>structuredClone(cur().engineeringLoop));
    assert.equal(loop.implementer,'Dev');assert.equal(loop.reviewer,'Reviewer');assert.equal(loop.round,1);
    assert.match(await t.page.locator('#engineeringloopstatus').innerText(),/Round 1\/3 · Paused · Reviewer/);
    const turns=t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat').map(request=>request.body);
    assert.equal(turns.length,2);assert.equal(turns[0].participant,'Dev');assert.equal(turns[1].participant,'Reviewer');
    assert.equal(turns[0].engineering_loop.id,loop.id);assert.equal(turns[0].engineering_loop.reviewer,'Reviewer');assert.match(turns[1].system,/machine-readable directive/);
    await t.page.locator('#engineeringloopstop').click();
    assert.equal(await t.page.locator('#engineeringloop').isHidden(),true);
    for(let i=0;i<100&&!t.f.requests.some(request=>request.method==='POST'&&request.path.endsWith('/stop'));i++)await t.page.waitForTimeout(10);
    assert.ok(t.f.requests.some(request=>request.method==='POST'&&request.path.endsWith('/stop')&&request.path.includes('/api/chat/loops/')));
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('/review starts a review-first Engineering Loop with one participant',options,async()=>{
  const t=await setup();try{
    await t.page.locator('#input').fill('/review Approve existing work');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='approved');
    const loop=await t.page.evaluate(()=>structuredClone(cur().engineeringLoop));
    assert.equal(loop.mode,'review_first');assert.equal(loop.phase,'reviewing');
    assert.equal(loop.reviewer,'Dev');assert.equal(loop.implementer,null);
    const turns=t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat').map(request=>request.body);
    assert.equal(turns.length,1);assert.equal(turns[0].participant,'Dev');assert.equal(turns[0].engineering_loop.mode,'review_first');
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('review-first waits for a writable implementer when changes are requested',options,async()=>{
  const t=await setup();try{
    await t.page.locator('#input').fill('/loop review Needs an implementer');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='awaiting_implementer');
    assert.equal(await t.page.locator('#engineeringloopassignment').isVisible(),true);
    assert.equal(await t.page.locator('#engineeringloopassign').isDisabled(),true);
    await t.page.evaluate(()=>{
      cur().participants.push({name:'Fixer',agent:'codex',role:'Developer',write:true,model:'',effort:'',session:'',seen:0});
      save();render();
    });
    await t.page.locator('#engineeringloopimplementer').selectOption('Fixer');
    await t.page.locator('#engineeringloopassign').click();
    await t.page.waitForFunction(()=>cur().log.some(entry=>entry.from==='Fixer'));
    const posts=t.f.requests.filter(request=>request.method==='POST');
    const statePost=posts.findIndex(request=>request.path==='/api/chat/state'&&request.body.state.convs
      .some(conversation=>conversation.participants.some(participant=>participant.name==='Fixer')));
    const implementerPost=posts.findIndex(request=>request.path.endsWith('/implementer')&&request.body.participant==='Fixer');
    assert.ok(statePost>=0);assert.ok(implementerPost>statePost);
    for(let i=0;i<100&&!t.f.requests.some(request=>request.path==='/api/chat'&&request.body.participant==='Fixer');i++)await t.page.waitForTimeout(10);
    assert.ok(t.f.requests.some(request=>request.path==='/api/chat'&&request.body.participant==='Fixer'));
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('review-first preserves implementer selection state when its durable loop is missing',options,async()=>{
  const t=await setup();try{
    await t.page.locator('#input').fill('/review Needs an implementer');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='awaiting_implementer');
    await t.page.evaluate(()=>{
      cur().participants.push({name:'Fixer',agent:'codex',role:'Developer',write:true,model:'',effort:'',session:'',seen:0});
      save();render();
    });
    t.f.missingLoop=true;
    await t.page.locator('#engineeringloopimplementer').selectOption('Fixer');
    await t.page.locator('#engineeringloopassign').click();
    await t.page.waitForFunction(()=>document.querySelector('#engineeringloopassign').textContent==='Continue');
    const loop=await t.page.evaluate(()=>structuredClone(cur().engineeringLoop));
    assert.equal(loop.state,'awaiting_implementer');assert.equal(loop.implementer,null);
    assert.equal(t.f.requests.filter(request=>request.path==='/api/chat'&&request.body.participant==='Fixer').length,0);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('Loop Engineering starts mandatory review when the implementer omits a handoff',options,async()=>{
  const t=await setup({team:true});try{
    await t.page.locator('#input').fill('/loop Missing handoff');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='paused');
    const loop=await t.page.evaluate(()=>structuredClone(cur().engineeringLoop));
    assert.equal(loop.turns,2);assert.equal(loop.phase,'reviewing');assert.equal(loop.reviewer,'Reviewer');
    assert.match(loop.reason,/structured verdict/);
    const turns=t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat');
    assert.deepEqual(turns.map(turn=>turn.body.participant),['Dev','Reviewer']);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('Loop Engineering never selects a read-only recipient as implementer',options,async()=>{
  const t=await setup({team:true});try{
    await t.page.evaluate(()=>{
      const c=cur(),reviewer=c.participants.find(participant=>participant.name==='Reviewer');
      reviewer.write=false;c.sendTo=['Reviewer'];save();render();
    });
    await t.page.locator('#input').fill('/loop Keep the writer as implementer');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='paused');
    const loop=await t.page.evaluate(()=>structuredClone(cur().engineeringLoop));
    assert.equal(loop.implementer,'Dev');assert.equal(loop.reviewer,'Reviewer');
    const turns=t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat');
    assert.deepEqual(turns.map(turn=>turn.body.participant),['Dev','Reviewer']);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('Loop Engineering ignores implementer attempts to replace the selected reviewer',options,async()=>{
  const t=await setup({team:true});try{
    await t.page.locator('#input').fill('/loop @Dev @Reviewer Change reviewer');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='paused');
    const loop=await t.page.evaluate(()=>structuredClone(cur().engineeringLoop));
    assert.equal(loop.turns,2);assert.equal(loop.phase,'reviewing');assert.equal(loop.reviewer,'Reviewer');
    assert.match(loop.reason,/structured verdict/);
    const turns=t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat');
    assert.deepEqual(turns.map(turn=>turn.body.participant),['Dev','Reviewer']);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

for(const [objective,label] of [['Review without target','missing'],['Review wrong target','incorrect']]){
  test(`Loop Engineering routes changes_requested with a ${label} target to its stored implementer`,options,async()=>{
    const t=await setup({team:true});try{
      await t.page.evaluate(()=>{loopMaxRounds=1});
      await t.page.locator('#input').fill(`/loop @Dev @Reviewer ${objective}`);await t.page.locator('#send').click();
      await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='awaiting_round_approval');
      const loop=await t.page.evaluate(()=>structuredClone(cur().engineeringLoop));
      assert.equal(loop.turns,2);assert.equal(loop.round,1);assert.equal(loop.phase,'reviewing');
      const turns=t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat');
      assert.deepEqual(turns.map(request=>request.body.participant),['Dev','Reviewer']);
      assert.deepEqual(t.errors,[]);
    }finally{await t.close();}
  });
}

test('Loop Engineering asks before each round beyond the configured maximum',options,async()=>{
  const t=await setup({team:true});try{
    await t.page.evaluate(()=>{loopMaxRounds=1});
    await t.page.locator('#input').fill('/loop @Dev @Reviewer Review at limit');await t.page.locator('#send').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='awaiting_round_approval');
    assert.match(await t.page.locator('#engineeringloopstatus').innerText(),/Waiting for your decision/);
    assert.equal(await t.page.locator('#engineeringloopapprove').isVisible(),true);
    assert.equal(await t.page.locator('#engineeringloopdecline').isVisible(),true);
    await t.page.locator('#engineeringloopapprove').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='awaiting_round_approval'&&cur().engineeringLoop.round===2);
    let decisions=t.f.requests.filter(request=>request.method==='POST'&&request.path.endsWith('/round'));
    assert.equal(decisions.length,1);assert.equal(decisions[0].body.decision,'approve');
    assert.deepEqual(t.f.requests.filter(request=>request.method==='POST'&&request.path==='/api/chat').map(request=>request.body.participant),['Dev','Reviewer','Dev','Reviewer']);
    await t.page.locator('#engineeringloopdecline').click();
    await t.page.waitForFunction(()=>cur().engineeringLoop?.state==='declined');
    decisions=t.f.requests.filter(request=>request.method==='POST'&&request.path.endsWith('/round'));
    assert.equal(decisions.length,2);assert.equal(decisions[1].body.decision,'decline');
    assert.equal(await t.page.locator('#engineeringloop').isHidden(),true);
    assert.deepEqual(t.errors,[]);
  }finally{await t.close();}
});

test('live Rust API: create a plan, discuss it, fork it and return to a free conversation',
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
      await reply(page,'Discuss the linked plan');
      const binding=await (await f.request('/api/conversations/live-source/plan')).json();
      assert.equal(binding.plan_slug,'live-frontend-plan');assert.deepEqual(binding.selections,{});
      assert.equal(await page.locator('#plantoggle, #planmode, #plantargets, #taskcompletedialog').count(),0);
      if(process.env.AIC_PLAN_DESKTOP_SCREENSHOT)await page.screenshot({path:process.env.AIC_PLAN_DESKTOP_SCREENSHOT});
      await page.locator('#input').fill('/fork');await page.locator('#send').click();await page.waitForFunction(()=>cur().id!=='live-source');
      const fork=await current(page);assert.equal(fork.planBinding.plan_id,'live-frontend-plan');assert.equal(fork.participants[0].session,'');
      await chooser(page,'free');await page.locator('#planapply').click();await page.waitForSelector('#plandialog',{state:'hidden'});assert.equal((await current(page)).planBinding.plan_id,null);
      assert.equal((await (await f.request('/api/conversations/live-source/plan')).json()).plan_slug,'live-frontend-plan');
      assert.deepEqual(errors,[]);
    }finally{await browser?.close();await f.close();}
  });
