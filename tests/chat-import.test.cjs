const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const importer = require('../src/chat/web/chat-import.js');
const source = {
  id: 'old', title: 'A conversation', round: 2, sendTo: ['Dev'],
  participants: [{ name: 'Dev', agent: 'codex', model: 'model', role: 'Developer\nwith context', effort: 'high',
    hue: 210, write: true, session: 'private-local-session', seen: 9 }],
  log: [
    { from: 'you', text: 'hello\n', to: ['Dev'], round: 1, t0: 123, pasted: [{ start: 0, end: 5, lines: 1 }] },
    { from: 'Dev', text: '## you\n\n```md\n## Dev (Codex, model)\n```\n\n', round: 1, actions: ['Read file'], usage: { cost_usd: 0.1 }, dur: 25 },
    { from: 'you', text: '', round: 2 },
    { from: 'Dev', text: 'error\nsecond line', round: 2, error: true },
  ],
};
for (const format of ['json', 'md']) test(`${format} round trip preserves history and renews sessions safely`, () => {
  const exported = importer.exportText(source, format);
  assert.ok(!exported.includes('private-local-session'));
  const c = importer.parse(exported, format, ['codex']).conversation;
  assert.equal(c.title, source.title);
  assert.deepEqual(c.log, source.log);
  assert.equal(c.participants[0].role, source.participants[0].role);
  assert.equal(c.participants[0].effort, 'high');
  assert.equal(c.participants[0].write, false);
  assert.equal(c.participants[0].session, '');
  assert.equal(c.participants[0].seen, source.log.length);
  assert.deepEqual(c.sendTo, ['Dev']);
});
test('legacy JSON and unavailable agents', () => {
  const c = importer.parse(JSON.stringify(source), 'json', ['claude']).conversation;
  assert.equal(c.participants[0].inactive, true);
  assert.equal(c.participants[0].agent, 'codex');
});
test('legacy Markdown preserves unused participants and ignores fenced headings', () => {
  const text = '# Old\n\nParticipants: Dev (Codex, Model), Vera (Claude)\n\n## you → Dev\n\nQuestion\n\n## Dev (Codex, Model)\n\n```md\n## you\n```\nAnswer\n';
  const c = importer.parse(text, 'md', ['codex', 'claude']).conversation;
  assert.equal(c.participants.length, 2);
  assert.equal(c.log.length, 2);
  assert.deepEqual(c.log[0].to, ['Dev']);
  assert.equal(c.log[1].text, '```md\n## you\n```\nAnswer');
});
test('ordinary Markdown is an unsent draft', () => {
  const text = '# Notes\n\n## you\nSome notes';
  assert.deepEqual(importer.parse(text, 'md', []), { draft: text });
});
test('malformed versions, references, metadata and structures are rejected', () => {
  for (const bad of [null, {}, { ...source, version: 2 }, { ...source, participants: [...source.participants, ...source.participants] },
    { ...source, log: [{ from: 'stranger', text: 'no' }] }, { ...source, log: [{ from: 'you', text: 'no', to: ['stranger'] }] }]) {
    assert.throws(() => importer.parse(JSON.stringify(bad), 'json', ['codex']));
  }
  assert.throws(() => importer.parse('{', 'json', []));
  assert.throws(() => importer.parse('---\naic: {}\n---\n', 'md', []));
  assert.throws(() => importer.parse(importer.exportText(source, 'md').slice(0, -5), 'md', ['codex']));
});
test('pending entries are imported as interrupted rather than running', () => {
  const c = importer.parse(JSON.stringify({ ...source, log: [{ from: 'Dev', text: '', pending: true }] }), 'json', ['codex']).conversation;
  assert.equal(c.log[0].text, 'interrupted');
  assert.equal(c.log[0].pending, undefined);
  assert.equal(c.log[0].error, true);
});
test('empty agent setups round trip in Markdown', () => {
  const c = { title: 'Setup', participants: source.participants, log: [] };
  assert.equal(importer.parse(importer.exportText(c, 'md'), 'md', ['codex']).conversation.participants.length, 1);
});

function harness(failKey) {
  const elements = new Map(); const cache = new Map(); const initial = { convs: [], current: null };
  let undo;
  const context = vm.createContext({
    ChatImport: importer, state: initial, drafts: {}, meta: { agents: ['codex'] }, MAX_PARTICIPANTS: 5,
    KEY: 'state', DRAFTS_KEY: 'drafts', CACHE_REPO_KEY: 'repo', cacheRepoKey: key => `repo:${key}`,
    serverCheckout: '/repo', crypto: { randomUUID: () => 'new-id' }, hist: {},
    document: { body: { classList: { remove() {} } } },
    $: (id) => { if (!elements.has(id)) elements.set(id, { value: '', focus() {} }); return elements.get(id); },
    localStorage: { getItem: (k) => cache.get(k) ?? null, removeItem: (k) => cache.delete(k), setItem(k,v) { if (k === failKey) throw Error('quota'); cache.set(k,v); } },
    cur: () => null, defaultAgent: () => '', autosize() {}, saveRemoteSoon() {}, render() {}, stopAll() {}, save() {}, saveDrafts() {},
    toast: (message, callback) => { context.message = message; undo = callback; },
  });
  const code = fs.readFileSync(require.resolve('../src/chat/web/chat.js'), 'utf8');
  vm.runInContext(code.slice(code.indexOf('async function importFile'), code.indexOf('/* ---------- rendering: top bar')), context);
  return { context, cache, initial, undo: () => undo() };
}
test('file import creates a complete conversation and undo removes it', async () => {
  const h = harness();
  await h.context.importFile({ size: 1, name: 'chat.json', text: async () => JSON.stringify(source) });
  assert.equal(h.context.state.convs.length, 1);
  assert.equal(h.context.state.current, 'new-id');
  assert.equal(h.context.state.convs[0].log.length, 4);
  assert.equal(h.cache.get('repo:state'), '/repo');
  assert.equal(h.cache.get('repo:drafts'), '/repo');
  h.undo(); assert.equal(h.context.state.convs.length, 0);
});
for (const key of ['state', 'drafts', 'repo:state', 'repo:drafts']) test(`failure writing ${key} leaves no partial import`, async () => {
  const h = harness(key);
  await h.context.importFile({ size: 1, name: 'chat.json', text: async () => JSON.stringify(source) });
  assert.equal(h.context.state, h.initial);
  assert.equal(h.context.state.convs.length, 0);
  assert.equal(h.cache.size, 0);
  assert.match(h.context.message, /storage/);
});
test('oversized files are rejected before reading', async () => {
  const h = harness();
  await h.context.importFile({ size: importer.MAX_BYTES + 1, name: 'chat.json', text: () => { throw Error('should not read'); } });
  assert.match(h.context.message, /too large/);
  assert.equal(h.context.state.convs.length, 0);
});
test('ordinary Markdown file creates only a draft and undo removes it', async () => {
  const h = harness();
  await h.context.importFile({ size: 1, name: 'notes.md', text: async () => '# Notes\nUnsent' });
  assert.equal(h.context.state.convs[0].log.length, 0);
  assert.equal(h.context.drafts['new-id'], '# Notes\nUnsent');
  h.undo(); assert.equal(h.context.drafts['new-id'], undefined);
});
