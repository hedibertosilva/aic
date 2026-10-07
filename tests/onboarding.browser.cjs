const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path');
const {execFileSync}=require('node:child_process');
const {fixture}=require('./chat-quotas.integration.cjs');
let chromium;try{({chromium}=require(process.env.AIC_PLAYWRIGHT_MODULE||'playwright'));}catch{}
test('readiness selection, keyboard recovery, draft preservation and branch acknowledgement', {skip:!process.env.AIC_TEST_BINARY||!chromium},async()=>{
 const f=await fixture();let browser;try{
  const config=path.join(f.root,'config/config.toml');fs.writeFileSync(config,fs.readFileSync(config,'utf8').replace('default_agent="codex"','default_agent=""'));
  browser=await chromium.launch({headless:true});const context=await browser.newContext({permissions:['clipboard-read','clipboard-write']});const page=await context.newPage();
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto(f.base);await page.waitForSelector('#readydialog[open]');await page.waitForFunction(()=>document.querySelector('#readyagent').options.length===4);
  assert.equal(await page.locator('#readyagent').inputValue(),'','no implicit agent choice');
  assert.equal(await page.locator('#readyagent option').count(),4);
  assert.match(await page.locator('#readyrows').innerText(),/Authentication/i);
  await page.locator('#readyagent').selectOption('codex');await page.locator('#readycontinue').click();await page.waitForSelector('#chips .chip');
  await page.locator('#input').fill('preserved draft');const draft=await page.locator('#input').inputValue();
  fs.rmSync(path.join(f.root,'.aic/diagnostics/tests.json'));
  await page.locator('#send').click();await page.waitForSelector('#readydialog[open]');
  assert.equal(await page.locator('#input').inputValue(),draft);
  await page.waitForFunction(()=>document.querySelector('#readyrows').textContent.includes('Last test: never'));
  assert.match(await page.locator('#readyrows').innerText(),/Last test: never/);
  await page.locator('#readyrows button').filter({hasText:'Copy command'}).first().click();
  assert.match(await page.evaluate(()=>navigator.clipboard.readText()),/aic/);
  execFileSync(path.resolve(process.env.AIC_TEST_BINARY),['agent','test','codex','--yes'],{cwd:f.root,env:{...process.env,AIC_CONFIG_DIR:path.join(f.root,'config'),AIC_IN_SANDBOX:'1'}});
  await page.locator('#readyrefresh').click();await page.waitForFunction(()=>document.querySelector('#readyrows').textContent.includes('ready_host'));
  await page.locator('#readyclose').focus();await page.keyboard.press('Enter');
  assert.equal(await page.locator('#input').inputValue(),draft);
  execFileSync('git',['-c','user.name=Fixture','-c','user.email=fixture@example.test','commit','--allow-empty','-qm','initial'],{cwd:f.root});
  execFileSync('git',['checkout','-qb','changed-branch'],{cwd:f.root});
  await page.locator('#send').click();await page.waitForSelector('#branchdialog[open]');
  assert.match(await page.locator('#branchdialog').innerText(),/keeps its history and participant sessions/);
  assert.equal(await page.locator('#input').inputValue(),draft);
  await page.locator('#branchcontinue').click();await page.waitForFunction(()=>JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e=>e.text==='Fixture reply'));
  assert.match(await page.locator('#repobranch').innerText(),/changed-branch/);
  assert.deepEqual(errors,[]);
 }finally{await browser?.close();await f.close();}
});
test('empty readiness prevents confirmation, preserves existing sessions and reports unavailable preference', {skip:!process.env.AIC_TEST_BINARY||!chromium},async()=>{
 const f=await fixture();let browser;try{
  fs.rmSync(path.join(f.root,'.aic/diagnostics/tests.json'));
  browser=await chromium.launch({headless:true});const page=await browser.newPage();await page.goto(f.base);await page.waitForSelector('#readydialog[open]');
  await page.waitForFunction(()=>document.querySelector('#readyreason').textContent.includes('unavailable'));
  assert.match(await page.locator('#readyreason').innerText(),/preferred agent \(codex\) is unavailable/);
  assert.equal(await page.locator('#readycontinue').isDisabled(),true);
  await page.locator('#readyclose').click();assert.equal(await page.locator('#send').isDisabled(),true);
  await page.setViewportSize({width:390,height:844});await page.locator('#readybtn').click();
  const box=await page.locator('#readydialog').boundingBox();assert.ok(box.x>=0&&box.x+box.width<=390);
 }finally{await browser?.close();await f.close();}
});

test('team shortcut, hidden selectors, skills and explicit checkout switch preserve history', {skip:!process.env.AIC_TEST_BINARY||!chromium},async()=>{
 const f=await fixture();let browser;try{
  execFileSync('git',['-c','user.name=Fixture','-c','user.email=fixture@example.test','commit','--allow-empty','-qm','initial'],{cwd:f.root});
  const worktree=path.join(f.root,'linked');execFileSync('git',['worktree','add','-qb','linked',worktree],{cwd:f.root});
  fs.mkdirSync(path.join(f.root,'.claude/skills/write-it'),{recursive:true});fs.writeFileSync(path.join(f.root,'.claude/skills/write-it/SKILL.md'),'---\nname: write-it\ndescription: Fixture editing skill\nargument-hint: <file>\nallowed-tools: [Edit]\n---\n');
  browser=await chromium.launch({headless:true});const page=await browser.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));await page.goto(f.base);await page.waitForSelector('#chips .chip');
  await page.locator('#input').fill('first message');await page.locator('#send').click();await page.waitForFunction(()=>JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e=>e.text==='Fixture reply'));
  await page.locator('#worktreebtn').click();await page.locator('#worktreeselect').selectOption(worktree);await page.locator('#worktreeconfirm').click();
  await page.waitForFunction(target=>document.querySelector('#contextcheckout').textContent===target,worktree);
  await page.waitForFunction(target=>{const s=JSON.parse(localStorage.getItem('aic-chat-v2'));return s.convs.find(c=>c.id===s.current)?.checkout===target;},worktree);
  const current=await page.evaluate(()=>{const s=JSON.parse(localStorage.getItem('aic-chat-v2'));return s.convs.find(c=>c.id===s.current);});assert.equal(current.checkout,worktree);assert.ok(current.log.length);assert.equal(current.participants[0].session,'');
  await page.locator('#readybtn').click();await page.waitForFunction(()=>document.querySelector('#readyrows').textContent.includes('Last test: never'));
  await page.locator('#agentvisibilityrows label').filter({hasText:'Claude'}).locator('input').uncheck();await page.locator('#readyclose').click();
  // Return to the ready original checkout, explicitly restarting native sessions.
  await page.locator('#worktreebtn').click();await page.locator('#worktreeselect').selectOption(f.root);await page.locator('#worktreeconfirm').click();await page.waitForFunction(target=>document.querySelector('#contextcheckout').textContent===target,f.root);
  await page.locator('#newoptions').click();await page.waitForSelector('#readydialog[open]');await page.waitForFunction(()=>document.querySelector('#readyagent').options.length===3);
  await page.locator('#readymode').selectOption('team');await page.locator('#readyagent').selectOption('codex');await page.locator('#readysecond').selectOption('codex');await page.locator('#readycontinue').click();await page.waitForFunction(()=>document.querySelectorAll('#chips .chip').length===2);assert.equal(await page.locator('#chips .chip').count(),2);
  await page.locator('#input').fill('/write');await page.waitForSelector('#ac.open');assert.match(await page.locator('#ac').innerText(),/exactly one participant/);
  assert.deepEqual(errors,[]);
 }finally{await browser?.close();await f.close();}
});
