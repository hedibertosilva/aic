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
async function emptyChat(page) {
  await page.waitForSelector('#empty');
  await page.waitForFunction(() => {
    const state = JSON.parse(localStorage.getItem('aic-chat-v2'));
    return state?.convs.find(c => c.id === state.current)?.participants.length === 0;
  });
  assert.equal(await page.locator('dialog[open]').count(), 0, 'new chat waits for an explicit card action');
  assert.equal(await page.locator('#empty button').filter({ hasText: 'Add participant' }).count(), 1);
  assert.equal(await page.locator('#empty button').filter({ hasText: 'Import conversation' }).count(), 1);
  assert.equal((await savedConversation(page)).participants.length, 0);
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

for (const [label, cacheRepo] of [['legacy unscoped', null], ['another repository', '/different/repository']]) {
  test(`${label} browser cache is not promoted into an empty database`, options, async () => {
    const f = await fixture(); let browser;
    try {
      browser = await chromium.launch({ headless: true });
      const page = await browser.newPage();
      await page.addInitScript(repo => {
        localStorage.setItem('aic-chat-v2', JSON.stringify({
          convs: [{ id: 'foreign', title: 'Foreign repository chat', participants: [],
            log: [{ from: 'you', text: 'foreign message', round: 1 }], round: 1, updated: Date.now() }],
          current: 'foreign', layout: 'auto',
        }));
        localStorage.setItem('aic-chat-drafts', JSON.stringify({ foreign: 'foreign draft' }));
        if (repo) for (const key of ['aic-chat-v2', 'aic-chat-drafts', 'aic-chat-draft-pastes']) {
          localStorage.setItem('aic-chat-cache-repo-v1:' + key, repo);
        }
      }, cacheRepo);
      await page.goto(f.base);
      await page.waitForSelector('#empty');
      await page.waitForFunction(() => {
        const state = JSON.parse(localStorage.getItem('aic-chat-v2'));
        return state?.convs.some(conversation => conversation.id === state.current)
          && !state.convs.some(conversation => conversation.id === 'foreign');
      });
      const cached = await page.evaluate(() => JSON.parse(localStorage.getItem('aic-chat-v2')));
      assert.equal(cached.convs.some(conversation => conversation.id === 'foreign'), false);
      assert.equal(await page.locator('#list').getByText('Foreign repository chat').count(), 0);
    } finally { await browser?.close(); await f.close(); }
  });
}

test('cache owned by the current repository is promoted into an empty database', options, async () => {
  const f = await fixture(); let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    await page.addInitScript(repo => {
      localStorage.setItem('aic-chat-v2', JSON.stringify({
        convs: [{ id: 'local', title: 'Current repository chat', participants: [],
          log: [{ from: 'you', text: 'local message', round: 1 }], round: 1, updated: Date.now() }],
        current: 'local', layout: 'auto',
      }));
      localStorage.setItem('aic-chat-cache-repo-v1:aic-chat-v2', repo);
    }, f.root);
    await page.goto(f.base);
    await page.waitForFunction(() => document.querySelector('#list')?.textContent.includes('Current repository chat'));
    await page.waitForFunction(async () => {
      const response = await fetch('/api/chat/state');
      const saved = await response.json();
      return saved.state?.convs?.some(conversation => conversation.id === 'local');
    });
  } finally { await browser?.close(); await f.close(); }
});

test('explicit participant choice, targeted sends, keyboard recovery, draft and branch acknowledgement', options, async () => {
  const f = await fixture(); let browser;
  try {
    preference(f, '');
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ permissions: ['clipboard-read', 'clipboard-write'] });
    const page = await context.newPage(), errors = [], requests = [];
    page.on('pageerror', e => errors.push(e.message)); page.on('request', r => requests.push(new URL(r.url()).pathname));
    await page.goto(f.base); await emptyChat(page);
    await page.locator('#empty button').filter({ hasText: 'Add participant' }).click();
    await page.waitForSelector('#pdialog[open]');
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
    const codexRecovery = page.locator('#readyrows .readycard').filter({ hasText: 'Codex' });
    assert.deepEqual(await codexRecovery.locator('.readycommand button').allTextContents(), ['Copy', 'Run']);
    assert.equal(await codexRecovery.locator('button.readyrun').isEnabled(), true);
    await codexRecovery.locator('button').filter({ hasText: 'Copy' }).click();
    assert.match(await page.evaluate(() => navigator.clipboard.readText()), /aic/);
    await codexRecovery.locator('button.readyrun').click();
    await page.waitForFunction(() => [...document.querySelectorAll('#readyrows .readycard')]
      .some(card => card.textContent.includes('Codex') && card.textContent.includes('Ready_host')));
    assert.ok(requests.includes('/api/diagnostics/run'));
    await page.locator('#readyclose').focus(); await page.keyboard.press('Enter');
    assert.equal(await page.locator('#input').inputValue(), draft);
    execFileSync('git', ['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '--allow-empty', '-qm', 'initial'], { cwd: f.root });
    execFileSync('git', ['checkout', '-qb', 'changed-branch'], { cwd: f.root });
    await page.waitForFunction(async () => (await (await fetch('/api/git')).json()).branch === 'changed-branch');
    await page.locator('#send').click(); await page.waitForSelector('#branchdialog[open]');
    assert.match(await page.locator('#branchdialog').innerText(), /keeps its history and participant sessions/);
    assert.equal(await page.locator('#input').inputValue(), draft);
    await paddedDialog(page, '#branchdialog');
    await page.locator('#branchcontinue').click();
    await page.waitForFunction(text => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e => e.from === 'you' && e.text === text), draft);
    assert.match(await page.locator('#contextbranch').innerText(), /changed-branch/);
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});

test('no ready agents: Environment is consultative, padded and scrollable on a short mobile screen', options, async () => {
  const f = await fixture(); let browser;
  try {
    fs.rmSync(path.join(f.root, '.aic/diagnostics/tests.json'));
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage({ viewport: { width: 390, height: 360 } });
    await page.goto(f.base); await emptyChat(page);
    await page.locator('#readybtn').click(); await page.waitForSelector('#readydialog[open]');
    await page.waitForFunction(() => document.querySelector('#readyreason').textContent.includes('unavailable'));
    assert.match(await page.locator('#readyreason').innerText(), /preferred agent \(codex\) is unavailable/);
    const before = await savedConversation(page);
    assert.equal(await page.locator('#readymode, #readyagent, #readysecond, #readycontinue, #readydialog #agentvisibility').count(), 0);
    const metrics = await paddedDialog(page, '#readydialog'); assert.ok(metrics.scroll, 'long panel scrolls internally');
    await page.locator('#readyrefresh').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    assert.deepEqual(await savedConversation(page), before);
    await page.locator('#readyclose').click(); assert.equal(await page.locator('#send').isDisabled(), true);
    await page.locator('#empty button').filter({ hasText: 'Add participant' }).click(); await page.waitForSelector('#pdialog[open]');
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
    const role = 'Review changes.\n\nFocus on correctness and safety.\nExplain the expected behavior and constraints.';
    assert.equal(await page.locator('#prole').evaluate(e => e.tagName), 'TEXTAREA');
    assert.equal(await page.locator('#prole').getAttribute('rows'), '5');
    await page.locator('#pagent').selectOption('codex'); await page.locator('#pname').fill('Reviewer'); await page.locator('#prole').fill(role); await page.locator('#psave').click();
    assert.equal(await page.locator('#chips .chip').count(), 2, 'teams use the participant flow');
    await page.locator('#chips .chip').filter({ hasText: 'Reviewer' }).click();
    await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#prole').inputValue(), role, 'editing retains line breaks');
    await page.locator('#pcancel').click();
    const prompts = [];
    page.on('request', request => {
      if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/chat') prompts.push(request.postDataJSON());
    });
    await page.locator('#input').fill('with existing sessions'); await page.locator('#send').click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].participants.every(p => p.session));
    assert.ok(prompts.some(prompt => prompt.system.includes(role)), 'instructions are sent with their line breaks');
    await page.locator('#to .pill').filter({ hasText: 'Reviewer' }).click();
    await page.locator('#input').fill('/fresh @Reviewer'); await page.locator('#send').click();
    const reset = await savedConversation(page), primary = reset.participants.find(p => p.name !== 'Reviewer'), reviewer = reset.participants.find(p => p.name === 'Reviewer');
    assert.ok(primary.session, 'an unmentioned participant keeps its session');
    assert.equal(reviewer.session, ''); assert.equal(reviewer.freshContext, true);
    assert.equal(await page.locator('#to .pill').filter({ hasText: 'Reviewer' }).getAttribute('aria-pressed'), 'false', 'an explicit /fresh target does not change To:');
    await page.locator('#input').fill('Unsent draft');
    const before = await savedConversation(page);
    await page.locator('#readybtn').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    await page.locator('#readyrefresh').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    await page.locator('#readyclose').click();
    assert.deepEqual(await savedConversation(page), before);
    assert.equal(await page.locator('#input').inputValue(), 'Unsent draft');
    await page.locator('#input').fill('/write'); // no automatic team role is assigned
    assert.equal((await savedConversation(page)).participants[1].role, role);
    await page.reload(); await page.waitForSelector('#chips .chip');
    assert.equal((await savedConversation(page)).participants[1].role, role, 'instructions survive reload');
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});

test('a failed participant can be retried while another reply is still running', options, async () => {
  const f = await fixture(); let browser;
  try {
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage();
    await page.goto(f.base); await page.waitForSelector('#chips .chip');
    const first = (await savedConversation(page)).participants[0].name;
    await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    await page.locator('#pagent').selectOption('codex'); await page.locator('#pname').fill('Slow'); await page.locator('#psave').click();
    let firstAttempts = 0;
    await page.route('**/api/chat', async route => {
      const request = route.request();
      if (request.method() !== 'POST') return route.continue();
      const body = request.postDataJSON();
      if (body.participant === 'Slow') await new Promise(resolve => setTimeout(resolve, 1500));
      const failed = body.participant === first && firstAttempts++ === 0;
      const events = failed
        ? [{type:'done', ok:false, error:'Temporary failure'}]
        : [{type:'session', id:'retry-session'}, {type:'delta', text:body.participant === 'Slow' ? 'Slow reply' : 'Recovered reply'}, {type:'done', ok:true}];
      await route.fulfill({status:200, contentType:'application/x-ndjson', body:events.map(JSON.stringify).join('\n') + '\n'});
    });
    await page.locator('#input').fill('retry this message'); await page.locator('#send').click();
    const retry = page.getByRole('button', { name: 'Try again', exact: true });
    await retry.waitFor();
    assert.ok(await page.locator('.card .status:not([hidden])').count(), 'the other participant is still replying');
    await retry.click();
    await page.waitForFunction(() => document.querySelector('#msgs').textContent.includes('Recovered reply'));
    assert.equal(await page.locator('.user').count(), 1, 'retry does not require deleting or resending the user message');
  } finally { await browser?.close(); await f.close(); }
});

test('sole ready agent still needs explicit confirmation; unavailable preference is never replaced', options, async () => {
  const f = await fixture(); let browser;
  try {
    preference(f, '');
    const cache = path.join(f.root, '.aic/diagnostics/tests.json'), records = JSON.parse(fs.readFileSync(cache, 'utf8'));
    delete records.tests.claude; delete records.tests.gemini; fs.writeFileSync(cache, JSON.stringify(records));
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage();
    await page.goto(f.base); await emptyChat(page);
    await page.locator('#empty button').filter({ hasText: 'Add participant' }).click();
    await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#pagent').inputValue(), ''); assert.equal(await page.locator('#psave').isDisabled(), true);
    await page.keyboard.press('Escape'); await page.waitForSelector('#pdialog', { state: 'hidden' });
    await page.locator('#empty button').filter({ hasText: 'Add participant' }).click();
    await page.waitForSelector('#pdialog[open]'); await page.mouse.click(4, 4);
    await page.waitForSelector('#pdialog', { state: 'hidden' });
    await page.locator('#empty button').filter({ hasText: 'Add participant' }).click();
    await page.waitForSelector('#pdialog[open]');
    await page.locator('#pagent').selectOption('codex'); await page.locator('#psave').click();
    assert.equal((await savedConversation(page)).participants[0].agent, 'codex');
    const file = path.join(f.root, 'config/config.toml'); fs.writeFileSync(file, fs.readFileSync(file, 'utf8').replace('default_agent=""', 'default_agent="claude"'));
    await page.locator('#new').click(); await emptyChat(page);
    await page.locator('#empty button').filter({ hasText: 'Add participant' }).click();
    await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#ppreference').count(), 0);
    assert.match(await page.locator('.participantenvironment').innerText(), /Environment/);
    assert.equal(await page.locator('#readydialog').getAttribute('open'), null, 'ready alternative goes directly to selection');
    assert.equal((await savedConversation(page)).participants.length, 0);
    assert.equal(await page.locator('#pagent').inputValue(), '');
    await page.locator('#pagent').selectOption('codex'); await page.locator('#psave').click();
    assert.equal((await savedConversation(page)).participants[0].agent, 'codex');
    assert.ok(fs.readFileSync(file, 'utf8').includes('default_agent="claude"'));
  } finally { await browser?.close(); await f.close(); }
});

test('new chat cards allow importing before participant setup, including after the keyboard shortcut', options, async () => {
  const f = await fixture(); let browser;
  try {
    preference(f, '');
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage();
    await page.goto(f.base); await emptyChat(page);
    const previous = (await savedConversation(page)).id;
    await page.keyboard.press('Control+Shift+O');
    await page.waitForFunction(id => JSON.parse(localStorage.getItem('aic-chat-v2')).current !== id, previous);
    await emptyChat(page);
    const fileChooser = page.waitForEvent('filechooser');
    await page.locator('#empty button').filter({ hasText: 'Import conversation' }).click();
    await (await fileChooser).setFiles({ name: 'conversation.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify({
      version: 1, title: 'Imported from card', participants: [{ name: 'Reviewer', agent: 'codex', role: 'Review changes\nExplain risks' }],
      log: [{ from: 'you', text: 'Existing question' }, { from: 'Reviewer', text: 'Existing answer' }],
    })) });
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs.some(c => c.title === 'Imported from card'));
    const imported = await savedConversation(page);
    assert.equal(imported.title, 'Imported from card');
    assert.equal(imported.participants.length, 1);
    assert.equal(imported.participants[0].role, 'Review changes\nExplain risks');
    assert.equal(imported.participants[0].session, '');
    assert.equal(imported.log.length, 2);
    assert.equal(await page.locator('dialog[open]').count(), 0);
    assert.ok(fs.readFileSync(path.join(f.root, 'config/config.toml'), 'utf8').includes('default_agent=""'));
  } finally { await browser?.close(); await f.close(); }
});

test('sidebar pins favorite conversations above the chronological groups and persists the choice', options, async () => {
  const f = await fixture(); let browser;
  try {
    const now = Date.now();
    const conversation = (id, title, updated) => ({ id, title, participants: [],
      log: [{ from: 'you', text: title, round: 1 }], round: 1, updated, sendTo: null });
    await f.request('/api/chat/state', { state: {
      convs: [conversation('recent', 'Recent conversation', now), conversation('favorite', 'Favorite conversation', now - 864e5)],
      current: 'recent', layout: 'auto',
    }, drafts: {} });
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage();
    await page.goto(f.base); await page.waitForSelector('.conv');
    assert.deepEqual(await page.locator('.conv .title').allTextContents(), ['Recent conversation', 'Favorite conversation']);

    const favorite = page.locator('.conv').filter({ hasText: 'Favorite conversation' });
    await favorite.getByRole('button', { name: /More actions/ }).click();
    await page.getByRole('menuitem', { name: 'Pin', exact: true }).click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs.find(c => c.id === 'favorite')?.pinned === true);
    assert.equal(await page.locator('#list h3').first().textContent(), 'Pinned');
    assert.deepEqual(await page.locator('.conv .title').allTextContents(), ['Favorite conversation', 'Recent conversation']);
    assert.equal(await page.locator('.conv').first().locator('.pinmark').getAttribute('title'), 'Pinned');
    await page.waitForFunction(async () => (await (await fetch('/api/chat/state')).json()).state.convs.find(c => c.id === 'favorite')?.pinned === true);

    await page.reload(); await page.waitForSelector('.conv');
    assert.deepEqual(await page.locator('.conv .title').allTextContents(), ['Favorite conversation', 'Recent conversation']);
    await page.locator('.conv').first().getByRole('button', { name: /More actions/ }).click();
    assert.equal(await page.getByRole('menuitem', { name: 'Unpin', exact: true }).count(), 1);
  } finally { await browser?.close(); await f.close(); }
});

test('default visibility is protected and checkout switching is scoped to this conversation', options, async () => {
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
    assert.equal(await page.locator('#agentvisibility legend').innerText(), 'Visible');
    const def = page.locator('#agentvisibilityrows label').filter({ hasText: 'Codex' }).locator('input');
    assert.equal(await def.isDisabled(), true); assert.equal(await def.isChecked(), true);
    await page.locator('#agentvisibilityrows label').filter({ hasText: 'Claude' }).locator('input').uncheck();
    assert.equal(await page.locator('#worktreebtn').isVisible(), false);
    assert.equal(await page.locator('#cform #worktreebtn').count(), 0);
    await page.locator('#csave').click(); await page.waitForSelector('#cdialog', { state: 'hidden' });
    await page.locator('#readybtn').click(); await page.waitForSelector('#readydialog[open]');
    assert.equal(await page.locator('#readyrows .readyagents').getByText('Claude', { exact: true }).count(), 0);
    await page.locator('#readyclose').click();
    await page.locator('#quotabtn').click();
    await page.waitForFunction(() => document.querySelectorAll('#quotarows > .quota').length > 0);
    assert.equal(await page.locator('#quotarows > .quota').filter({ hasText: 'Claude' }).count(), 0);
    await page.locator('#quotaclose').click();
    await page.locator('#conversationadvanced summary').click();
    assert.match(await page.locator('#conversationadvanced').innerText(), /this conversation only/);
    await page.locator('#worktreebtn').click();
    await page.setViewportSize({ width: 390, height: 360 }); await paddedDialog(page, '#worktreedialog');
    await page.locator('#worktreeselect').selectOption(worktree); await page.locator('#worktreeconfirm').click();
    await page.waitForFunction(target => document.querySelector('#contextbar').dataset.checkout === target, worktree);
    await page.waitForFunction(target => { const s = JSON.parse(localStorage.getItem('aic-chat-v2')); return s.convs.find(c => c.id === s.current)?.checkout === target; }, worktree);
    const current = await savedConversation(page); assert.equal(current.checkout, worktree); assert.ok(current.log.length); assert.equal(current.participants[0].session, '');
    // Return through Advanced, preserving explicit session reset and local readiness.
    await page.locator('#conversationadvanced summary').click(); await page.locator('#worktreebtn').click(); await page.locator('#worktreeselect').selectOption(f.root); await page.locator('#worktreeconfirm').click();
    await page.waitForFunction(target => document.querySelector('#contextbar').dataset.checkout === target, f.root);
    await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#pagent option[value="claude"]').count(), 0);
    assert.equal(await page.locator('#pagent').inputValue(), 'codex', 'effective ready preference is shown');
    await page.locator('#psave').click(); assert.equal(await page.locator('#chips .chip').count(), 2);
    await page.locator('#input').fill('/write'); await page.waitForSelector('#ac.open'); assert.match(await page.locator('#ac').innerText(), /exactly one participant/);
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});

test('Configuration stages visibility: Cancel/Esc and failed Save discard or retain the draft without persistence', options, async () => {
  const f = await fixture(); let browser;
  try {
    browser = await chromium.launch({ headless: true }); const page = await browser.newPage();
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.goto(f.base); await page.waitForSelector('#chips .chip');
    const configFile = path.join(f.root, 'config/config.toml'), original = fs.readFileSync(configFile, 'utf8');
    const hidden = () => page.evaluate(() => JSON.parse(localStorage.getItem('aic-chat-v2')).hiddenAgents || []);
    const box = name => page.locator('#agentvisibilityrows label').filter({hasText:name}).locator('input');
    const open = async () => { await page.locator('#configbtn').click(); await page.waitForSelector('#cdialog[open]'); };
    await open(); await box('Claude').uncheck(); await page.locator('#cpermission').selectOption('plan');
    await page.locator('#csandboximage').fill('unsaved-image');
    assert.deepEqual(await hidden(), [], 'checkbox must not mutate stored preferences');
    assert.equal(await page.locator('#cform #worktreebtn').count(), 0, 'conversation actions are outside global form');
    assert.equal(await page.locator('#worktreebtn').isVisible(), false, 'disclosure is closed and inert behind Configuration');
    await page.locator('#ccancel').click();
    assert.equal(fs.readFileSync(configFile, 'utf8'), original);
    assert.deepEqual(await hidden(), []);
    await open(); assert.equal(await box('Claude').isChecked(), true);
    assert.equal(await page.locator('#cpermission').inputValue(), 'acceptEdits');
    assert.equal(await page.locator('#cserializewriters').isChecked(), true);
    assert.equal(await page.locator('#clooprounds').inputValue(), '3');
    assert.notEqual(await page.locator('#csandboximage').inputValue(), 'unsaved-image');
    await box('Claude').uncheck(); await page.keyboard.press('Escape');
    assert.deepEqual(await hidden(), []);
    await open(); assert.equal(await box('Claude').isChecked(), true);
    await box('Claude').uncheck(); await page.locator('#cdefault').selectOption('claude');
    assert.equal(await box('Claude').isChecked(), true); assert.equal(await box('Claude').isDisabled(), true, 'pending new default stays visible');
    await page.locator('#cdefault').selectOption('codex'); await box('Claude').uncheck();
    await page.route('**/api/config', async route => {
      if (route.request().method() === 'POST') await route.fulfill({status:409,body:'Fixture save rejected'});
      else await route.continue();
    });
    await page.locator('#csave').click(); await page.waitForSelector('#cerror:not([hidden])');
    assert.match(await page.locator('#cerror').innerText(), /Fixture save rejected/);
    assert.deepEqual(await hidden(), []); assert.equal(await box('Claude').isChecked(), false, 'failed save retains only draft');
    assert.equal(fs.readFileSync(configFile, 'utf8'), original);
    await page.locator('#ccancel').click(); await page.unroute('**/api/config');
    await open(); assert.equal(await box('Claude').isChecked(), true);
    await box('Claude').uncheck(); await page.locator('#csave').click(); await page.waitForSelector('#cdialog', {state:'hidden'});
    assert.deepEqual(await hidden(), ['claude']);
    await page.reload(); await page.waitForSelector('#chips .chip');
    assert.deepEqual(await hidden(), ['claude'], 'successful Save survives reload');
    await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    assert.equal(await page.locator('#pagent option[value="claude"]').count(), 0);
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});

test('Environment indicator ignores unused agents, follows participants and keeps quota in its own panel', options, async () => {
  const f = await fixture(); let browser;
  try {
    const cache = path.join(f.root, '.aic/diagnostics/tests.json');
    const records = JSON.parse(fs.readFileSync(cache, 'utf8')); delete records.tests.claude; delete records.tests.gemini; fs.writeFileSync(cache, JSON.stringify(records));
    browser = await chromium.launch({headless:true}); const page = await browser.newPage();
    await page.goto(f.base); await page.waitForSelector('#chips .chip');
    await page.waitForFunction(() => document.querySelector('#readybtn').getAttribute('aria-label') === 'Environment ready');
    await page.locator('#readybtn').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled);
    assert.deepEqual(await page.locator('#readyrows .readysection > h3').allTextContents(), ['Workspace','Agents']);
    assert.equal(await page.locator('#readyrows .readycard').count(), 6);
    assert.match(await page.locator('#readyrows').innerText(), /Claude/);
    assert.match(await page.locator('#readyrows').innerText(), /Gemini/);
    assert.deepEqual(await page.locator('#readyrows dt').allTextContents(), ['Installation','Authentication','Execution','Installation','Authentication','Execution','Installation','Authentication','Execution']);
    await page.locator('#readyclose').click();
    await page.locator('#input').fill('Team history'); await page.locator('#send').click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e => e.text === 'Fixture reply' && !e.pending));
    testAgent(f, 'claude');
    await page.locator('#addbtn').click(); await page.waitForSelector('#pdialog[open]');
    await page.locator('#pagent').selectOption('claude'); await page.locator('#psave').click();
    const withClaude = await savedConversation(page);
    await page.locator('#new').click(); await page.waitForFunction(() => document.querySelectorAll('#chips .chip').length === 1);
    await page.locator('#input').fill('Solo history'); await page.locator('#send').click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e => e.text === 'Fixture reply' && !e.pending));
    const withoutClaude = await savedConversation(page);
    const invalid = JSON.parse(fs.readFileSync(cache,'utf8')); delete invalid.tests.claude; fs.writeFileSync(cache,JSON.stringify(invalid));
    await page.locator('#readybtn').click(); await page.waitForFunction(() => !document.querySelector('#readyrefresh').disabled); await page.locator('#readyclose').click();
    assert.equal(await page.locator('#readybtn').getAttribute('aria-label'), 'Environment ready');
    await page.locator('#list .conv .open').filter({hasText:withClaude.title}).click();
    await page.waitForFunction(() => document.querySelector('#readybtn').getAttribute('aria-label') === 'Environment: 1 pending item');
    await page.locator('#list .conv .open').filter({hasText:withoutClaude.title}).click();
    await page.waitForFunction(() => document.querySelector('#readybtn').getAttribute('aria-label') === 'Environment ready');
    await page.locator('#quotabtn').click(); assert.equal(await page.locator('#quotas').isVisible(), true);
  } finally { await browser?.close(); await f.close(); }
});

test('pasted text stays movable, rejects media and expands from the sent message', options, async () => {
  const f = await fixture(); let browser;
  try {
    browser = await chromium.launch({headless:true}); const page = await browser.newPage();
    const errors = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto(f.base); await page.waitForSelector('#chips .chip');
    await page.locator('#input').evaluate(node => {
      const oneLine = new DataTransfer(); oneLine.setData('text/plain', 'one line');
      node.dispatchEvent(new ClipboardEvent('paste', {clipboardData:oneLine, bubbles:true, cancelable:true}));
      const twoLines = new DataTransfer(); twoLines.setData('text/plain', '\nand two lines');
      node.dispatchEvent(new ClipboardEvent('paste', {clipboardData:twoLines, bubbles:true, cancelable:true}));
    });
    assert.equal(await page.locator('#input').inputValue(), 'One line\nand two lines');
    await page.locator('#input').fill('Existing text');
    await page.locator('#input').evaluate(node => {
      node.setSelectionRange(4, 4);
      const data = new DataTransfer(); data.setData('text/plain', 'alpha\nbeta\ngamma');
      node.dispatchEvent(new ClipboardEvent('paste', {clipboardData:data, bubbles:true, cancelable:true}));
    });
    const marker = '[Pasted Text · +3 lines]';
    assert.equal(await page.locator('#input').inputValue(), marker + '\nExisting text');
    await page.locator('#input').evaluate((node, end) => node.setSelectionRange(end, end), marker.length);
    await page.locator('#input').press('Backspace');
    assert.equal(await page.locator('#input').inputValue(), 'Existing text', 'deleting the closing bracket removes the whole pasted block');
    await page.locator('#input').evaluate(node => {
      node.setSelectionRange(0, 0);
      const data = new DataTransfer(); data.setData('text/plain', 'alpha\nbeta\ngamma');
      node.dispatchEvent(new ClipboardEvent('paste', {clipboardData:data, bubbles:true, cancelable:true}));
    });
    await page.locator('#input').fill('Before\n' + marker + '\nAfter');
    await page.waitForTimeout(500); await page.reload(); await page.waitForSelector('#chips .chip');
    assert.equal(await page.locator('#input').inputValue(), 'Before\n' + marker + '\nAfter', 'paste metadata survives reload');
    await page.locator('#input').evaluate(node => {
      const data = new DataTransfer(); data.items.add(new File(['image'], 'image.png', {type:'image/png'}));
      node.dispatchEvent(new ClipboardEvent('paste', {clipboardData:data, bubbles:true, cancelable:true}));
    });
    assert.equal(await page.locator('#input').inputValue(), 'Before\n' + marker + '\nAfter');
    assert.match(await page.locator('#toast').innerText(), /Media can't be pasted/);
    await page.locator('#send').click();
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('aic-chat-v2')).convs[0].log.some(e => e.text === 'Before\nalpha\nbeta\ngamma\nAfter'));
    assert.equal(await page.locator('.user .pastedtext').count(), 1);
    assert.equal(await page.locator('.pastedtextpreview').innerText(), 'alpha');
    assert.equal(await page.locator('.pastedtextcontent').isHidden(), true);
    await page.locator('.pastedtexttoggle').click();
    assert.equal(await page.locator('.pastedtextcontent').innerText(), 'beta\ngamma');
    assert.deepEqual(errors, []);
  } finally { await browser?.close(); await f.close(); }
});
