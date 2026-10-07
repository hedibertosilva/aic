const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path');
const { execFileSync } = require('node:child_process');
const { fixture } = require('./chat-quotas.integration.cjs');
let chromium; try { ({ chromium } = require(process.env.AIC_PLAYWRIGHT_MODULE || 'playwright')); } catch {}
const options = { skip: !process.env.AIC_TEST_BINARY || !chromium };
const savedConversation = page => page.evaluate(() => {
  const state = JSON.parse(localStorage.getItem('aic-chat-v2'));
  return state.convs.find(c => c.id === state.current);
});
function testAgent(f, agent = 'codex') {
  execFileSync(path.resolve(process.env.AIC_TEST_BINARY), ['agent', 'test', agent, '--yes'], {
    cwd: f.root, env: { ...process.env, AIC_CONFIG_DIR: path.join(f.root, 'config'), AIC_IN_SANDBOX: '1' },
  });
}
function preference(f, name) {
  const file = path.join(f.root, 'config/config.toml');
  fs.writeFileSync(file, fs.readFileSync(file, 'utf8').replace('default_agent="codex"', 'default_agent=' + JSON.stringify(name)));
}
async function paddedDialog(page, id) {
  await page.locator(id).waitFor({ state: 'visible' });
  const metrics = await page.locator(id).evaluate(d => {
    d.scrollTop = 0;
    const box = d.getBoundingClientRect(), title = d.querySelector('h2').getBoundingClientRect(), style = getComputedStyle(d);
    return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, width: innerWidth, height: innerHeight,
      paddingX: parseFloat(style.paddingLeft), paddingY: parseFloat(style.paddingTop),
      titleInset: title.left - box.left, titleTop: title.top - box.top,
      scroll: d.scrollHeight > d.clientHeight, overflow: style.overflowY };
  });
  assert.ok(metrics.left >= 0 && metrics.right <= metrics.width, 'fits horizontally');
  assert.ok(metrics.top >= 0 && metrics.bottom <= metrics.height, 'fits vertically');
  assert.ok(metrics.paddingX >= 20 && metrics.paddingY >= 18, 'internal padding');
  assert.ok(metrics.titleInset >= 20 && metrics.titleTop >= 18, 'title stays away from border: ' + JSON.stringify(metrics));
  assert.equal(metrics.overflow, 'auto');
  return metrics;
}

test('explicit participant choice, targeted sends, keyboard recovery, draft and branch acknowledgement', options, async () => {
  const f = await fixture(); let browser;
  try {
    preference(f, '');
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ permissions: ['clipboard-read', 'clipboard-write'] });
    const page = await context.newPage(), errors = [], requests = [];
    page.on('pageerror', e => errors.push(e.message)); page.on('request', r => requests.push(new URL(r.url()).pathname));
    await page.goto(f.base); await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#pagent').inputValue(), '', 'no implicit agent choice');
    assert.equal(await page.locator('#psave').isDisabled(), true);
    assert.equal(await page.locator('#readydialog').getAttribute('open'), null);
    assert.equal(await page.locator('#newoptions').count(), 0);
    await page.locator('#pagent').selectOption('codex');
    assert.equal(await page.locator('#psave').isDisabled(), false);
    await page.locator('#psave').click(); await page.waitForSelector('#chips .chip');
    await page.locator('#input').fill('first message'); requests.length = 0;
    await page.locator('#send').click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e => e.text === 'Fixture reply' && !e.pending));
    assert.ok(requests.includes('/api/chat/preflight'));
    assert.equal(requests.filter(p => ['/api/diagnostics', '/api/agents', '/api/skills'].includes(p)).length, 0, 'send avoids full diagnostics/catalog reads');
    await page.locator('#input').fill('preserved draft'); const draft = await page.locator('#input').inputValue();
    fs.rmSync(path.join(f.root, '.aic/diagnostics/tests.json'));
    await page.locator('#send').click(); await page.waitForSelector('#readydialog[open]');
    assert.equal(await page.locator('#input').inputValue(), draft);
    await page.waitForFunction(() => document.querySelector('#readyrows').textContent.includes('Last test: never'));
    await page.locator('#readyrows button').filter({ hasText: 'Copy command' }).first().click();
    assert.match(await page.evaluate(() => navigator.clipboard.readText()), /aic/);
    testAgent(f); await page.locator('#readyrefresh').click();
    await page.waitForFunction(() => document.querySelector('#readyrows').textContent.includes('ready_host'));
    await page.locator('#readyclose').focus(); await page.keyboard.press('Enter');
    assert.equal(await page.locator('#input').inputValue(), draft);
    execFileSync('git', ['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '--allow-empty', '-qm', 'initial'], { cwd: f.root });
    execFileSync('git', ['checkout', '-qb', 'changed-branch'], { cwd: f.root });
    await page.locator('#send').click(); await page.waitForSelector('#branchdialog[open]');
    assert.match(await page.locator('#branchdialog').innerText(), /keeps its history and participant sessions/);
    assert.equal(await page.locator('#input').inputValue(), draft);
    await paddedDialog(page, '#branchdialog');
    await page.locator('#branchcontinue').click();
    await page.waitForFunction(text => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e => e.from === 'you' && e.text === text), draft);
    assert.match(await page.locator('#repobranch').innerText(), /changed-branch/);
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});

test('no ready agents: Environment is consultative, padded and scrollable on a short mobile screen', options, async () => {
  const f = await fixture(); let browser;
  try {
    fs.rmSync(path.join(f.root, '.aic/diagnostics/tests.json'));
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage({ viewport: { width: 390, height: 360 } });
    await page.goto(f.base); await page.waitForSelector('#readydialog[open]');
    await page.waitForFunction(() => document.querySelector('#readyreason').textContent.includes('unavailable'));
    assert.match(await page.locator('#readyreason').innerText(), /preferred agent \(codex\) is unavailable/);
    const before = await savedConversation(page);
    assert.equal(await page.locator('#readymode, #readyagent, #readysecond, #readycontinue, #readydialog #agentvisibility').count(), 0);
    const metrics = await paddedDialog(page, '#readydialog'); assert.ok(metrics.scroll, 'long panel scrolls internally');
    await page.locator('#readyrefresh').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    assert.deepEqual(await savedConversation(page), before);
    await page.locator('#readyclose').click(); assert.equal(await page.locator('#send').isDisabled(), true);
    await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#psave').isDisabled(), true);
    assert.equal((await savedConversation(page)).participants.length, 0);
  } finally { await browser?.close(); await f.close(); }
});

test('opening and refreshing Environment never changes existing participants, roles, sessions or drafts', options, async () => {
  const f = await fixture(); let browser;
  try {
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage();
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.goto(f.base); await page.waitForSelector('#chips .chip');
    const empty = await savedConversation(page);
    await page.locator('#readybtn').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    await page.locator('#readyclose').click();
    assert.deepEqual(await savedConversation(page), empty, 'consulting an empty conversation preserves its participant');
    await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    await page.locator('#pagent').selectOption('codex'); await page.locator('#pname').fill('Reviewer'); await page.locator('#prole').fill('Review changes'); await page.locator('#psave').click();
    assert.equal(await page.locator('#chips .chip').count(), 2, 'teams use the participant flow');
    await page.locator('#input').fill('with existing sessions'); await page.locator('#send').click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].participants.every(p => p.session));
    await page.locator('#input').fill('Unsent draft');
    const before = await savedConversation(page);
    await page.locator('#readybtn').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    await page.locator('#readyrefresh').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    await page.locator('#readyclose').click();
    assert.deepEqual(await savedConversation(page), before);
    assert.equal(await page.locator('#input').inputValue(), 'Unsent draft');
    await page.locator('#input').fill('/write'); // no automatic team role is assigned
    assert.equal((await savedConversation(page)).participants[1].role, 'Review changes');
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});

test('sole ready agent still needs explicit confirmation; unavailable preference is never replaced', options, async () => {
  const f = await fixture(); let browser;
  try {
    preference(f, '');
    const cache = path.join(f.root, '.aic/diagnostics/tests.json'), records = JSON.parse(fs.readFileSync(cache, 'utf8'));
    delete records.tests.claude; delete records.tests.gemini; fs.writeFileSync(cache, JSON.stringify(records));
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage();
    await page.goto(f.base); await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#pagent').inputValue(), ''); assert.equal(await page.locator('#psave').isDisabled(), true);
    await page.locator('#pagent').selectOption('codex'); await page.locator('#psave').click();
    assert.equal((await savedConversation(page)).participants[0].agent, 'codex');
    const file = path.join(f.root, 'config/config.toml'); fs.writeFileSync(file, fs.readFileSync(file, 'utf8').replace('default_agent=""', 'default_agent="claude"'));
    await page.locator('#new').click(); await page.waitForSelector('#readydialog[open]');
    await page.waitForFunction(() => document.querySelector('#readyreason').textContent.includes('preferred agent (claude)'));
    assert.equal((await savedConversation(page)).participants.length, 0);
    await page.locator('#readyclose').click(); await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#pagent').inputValue(), '');
    await page.locator('#pagent').selectOption('codex'); await page.locator('#psave').click();
    assert.equal((await savedConversation(page)).participants[0].agent, 'codex');
    assert.ok(fs.readFileSync(file, 'utf8').includes('default_agent="claude"'));
  } finally { await browser?.close(); await f.close(); }
});

test('default visibility is protected and checkout switching is only in Advanced configuration', options, async () => {
  const f = await fixture(); let browser;
  try {
    execFileSync('git', ['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '--allow-empty', '-qm', 'initial'], { cwd: f.root });
    const worktree = path.join(f.root, 'linked'); execFileSync('git', ['worktree', 'add', '-qb', 'linked', worktree], { cwd: f.root });
    await f.request('/api/chat/state', { state: { convs: [], current: null, hiddenAgents: ['codex'] }, drafts: {} });
    fs.mkdirSync(path.join(f.root, '.claude/skills/write-it'), { recursive: true });
    fs.writeFileSync(path.join(f.root, '.claude/skills/write-it/SKILL.md'), '---\nname: write-it\ndescription: Fixture editing skill\nargument-hint: <file>\nallowed-tools: [Edit]\n---\n');
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage(); const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.goto(f.base); await page.waitForSelector('#chips .chip');
    assert.equal((await savedConversation(page)).participants[0].agent, 'codex', 'legacy hidden default remains effective');
    assert.equal(await page.locator('#repobar #worktreebtn').count(), 0);
    await page.locator('#input').fill('first message'); await page.locator('#send').click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e => e.text === 'Fixture reply' && !e.pending));
    await page.locator('#configbtn').click(); await page.waitForSelector('#cdialog[open]');
    const def = page.locator('#agentvisibilityrows label').filter({ hasText: 'Codex' }).locator('input');
    assert.equal(await def.isDisabled(), true); assert.equal(await def.isChecked(), true);
    await page.locator('#agentvisibilityrows label').filter({ hasText: 'Claude' }).locator('input').uncheck();
    assert.equal(await page.locator('#worktreebtn').isVisible(), false);
    await page.locator('#cadvanced summary').click(); await page.locator('#worktreebtn').click();
    await page.setViewportSize({ width: 390, height: 360 }); await paddedDialog(page, '#worktreedialog');
    await page.locator('#worktreeselect').selectOption(worktree); await page.locator('#worktreeconfirm').click();
    await page.waitForFunction(target => document.querySelector('#contextcheckout').textContent === target, worktree);
    await page.waitForFunction(target => { const s = JSON.parse(localStorage.getItem('aic-chat-v2')); return s.convs.find(c => c.id === s.current)?.checkout === target; }, worktree);
    const current = await savedConversation(page); assert.equal(current.checkout, worktree); assert.ok(current.log.length); assert.equal(current.participants[0].session, '');
    // Return through Advanced, preserving explicit session reset and local readiness.
    await page.locator('#configbtn').click(); await page.waitForSelector('#cdialog[open]');
    await page.locator('#cadvanced summary').click(); await page.locator('#worktreebtn').click(); await page.locator('#worktreeselect').selectOption(f.root); await page.locator('#worktreeconfirm').click();
    await page.waitForFunction(target => document.querySelector('#contextcheckout').textContent === target, f.root);
    await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#pagent option[value="claude"]').count(), 0);
    assert.equal(await page.locator('#pagent').inputValue(), 'codex', 'effective ready preference is shown');
    await page.locator('#psave').click(); assert.equal(await page.locator('#chips .chip').count(), 2);
    await page.locator('#input').fill('/write'); await page.waitForSelector('#ac.open'); assert.match(await page.locator('#ac').innerText(), /exactly one participant/);
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});
