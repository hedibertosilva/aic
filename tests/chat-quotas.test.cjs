const { test } = require('node:test');
const assert = require('node:assert/strict');
const Q = require('../src/chat/web/chat-quotas.js');
const quota = (...windows) => ({ state: 'available', windows });
test('alerts use inclusive thresholds while summaries prefer the shortest window', () => {
  assert.equal(Q.level(quota({ used_pct: 79.9 })), 'normal');
  assert.equal(Q.level(quota({ used_pct: 80 })), 'warn');
  assert.equal(Q.level(quota({ used_pct: 100 })), 'out');
  const q = quota({ used_pct: 41, window_minutes: 300 }, { used_pct: 86, window_minutes: 10080 });
  assert.equal(Q.critical(q).window_minutes, 10080);
  assert.equal(Q.primary(q).window_minutes, 300);
  assert.equal(Q.summary(q), '41% · 5h');
});
test('unknown quotas and provider warnings never fabricate a percentage', () => {
  assert.equal(Q.summary(quota()), 'Quota not available');
  assert.equal(Q.percent(null), '');
  assert.equal(Q.percent(-1), '');
  const warning = quota({ provider_status: 'allowed_warning' });
  assert.equal(Q.level(warning), 'warn');
  assert.ok(!Q.summary(warning).includes('%'));
  assert.equal(Q.summary(quota({ provider_status: 'rejected' })), 'Exhausted');
  assert.equal(Q.summary({ reason: 'No quota reading yet' }), 'No quota reading yet');
});
test('missing usage fields stay n/a and partial totals are marked as lower bounds', () => {
  const partial = Q.consumptionText({ turns: 2, missing_usage: 1, usage: { input_tokens: 100 }, reports: { input_tokens: 1 } });
  assert.ok(partial.includes('≥ 100 in'));
  assert.ok(partial.includes('out n/a'));
  assert.ok(partial.includes('cost n/a'));
  assert.ok(!partial.includes('$0'));
  assert.ok(partial.includes("1 reply didn't report usage"));
  assert.equal(Q.consumptionText(null), 'No usage yet in this conversation');
  const knownZero = Q.consumptionText({ turns: 1, usage: { cost_usd: 0 }, reports: { cost_usd: 1 } });
  assert.ok(knownZero.includes('$0.0000'));
});
test('expired windows preserve percentages and reading errors preserve age', () => {
  const now = Date.parse('2026-10-03T15:00:00Z');
  const q = { windows: [{ used_pct: 100, resets_at: '2026-10-03T14:00:00Z' }], fetched_at: '2026-10-03T14:30:00Z' };
  assert.equal(Q.expired(q, now), true);
  assert.equal(Q.stale(q, now), true);
  assert.equal(Q.summary(q), 'Exhausted');
  assert.ok(Q.resetText(q.windows[0], now).startsWith('Reset at'));
  q.state = 'read_error'; q.attempted_at = '2026-10-03T14:59:00Z';
  assert.equal(Q.updated(q, now), 'Updated 30 min ago');
  assert.equal(Q.critical(q).used_pct, 100);
  assert.equal(Q.resetText({}), 'reset time unknown');
});
test('a failed refresh does not grey out a recent successful reading', () => {
  const now = Date.parse('2026-10-03T15:00:00Z');
  const recent = { state: 'read_error', windows: [{ used_pct: 34, resets_at: '2026-10-03T18:00:00Z' }], fetched_at: '2026-10-03T14:59:00Z' };
  assert.deepEqual(Q.visualStatus(recent, false, now), { kind: 'normal', label: 'Quota normal; latest refresh failed' });
  recent.fetched_at = '2026-10-03T14:30:00Z';
  assert.deepEqual(Q.visualStatus(recent, false, now), { kind: 'stale', label: 'Quota status is outdated' });
});
test('expired windows stop after two automatic retries and reset with a new window', () => {
  const attempts = new Map(), now = Date.parse('2026-10-03T15:00:00Z');
  const status = { windows: [{ id: 'codex/primary', resets_at: '2026-10-03T14:00:00Z' }] };
  assert.equal(Q.shouldRetryExpired(status, attempts, now), true);
  assert.equal(Q.shouldRetryExpired(status, attempts, now + 59999), false);
  assert.equal(Q.shouldRetryExpired(status, attempts, now + 60000), true);
  assert.equal(Q.shouldRetryExpired(status, attempts, now + 120000), false);
  status.windows[0].resets_at = '2026-10-03T14:30:00Z';
  assert.equal(Q.shouldRetryExpired(status, attempts, now + 120000), true);
  status.windows[0].resets_at = '2026-10-03T18:00:00Z';
  assert.equal(Q.shouldRetryExpired(status, attempts, now + 120000), false);
  assert.equal(attempts.size, 0);
});
// agy shares one window per model group; this is its real /usage shape.
const agy = quota(
  { id: 'gemini-weekly', label: 'Weekly', bucket: 'Gemini Models', models: ['Gemini Flash', 'Gemini Pro'], used_pct: 100, window_minutes: 10080 },
  { id: '3p-weekly', label: 'Weekly', bucket: 'Claude and GPT models', models: ['Claude Opus', 'Claude Sonnet', 'GPT-OSS'], used_pct: 0, window_minutes: 10080 });
test('an exhausted model group only limits the models in it', () => {
  assert.equal(Q.level(Q.scoped(agy, 'gemini-3-pro-preview')), 'out');
  assert.equal(Q.level(Q.scoped(agy, 'gemini-3-flash')), 'out');
  assert.equal(Q.level(Q.scoped(agy, 'claude-opus-4-5-thinking')), 'normal');
  assert.equal(Q.level(Q.scoped(agy, 'gpt-oss-120b')), 'normal');
  assert.equal(Q.scoped(agy, 'claude-opus-4-5').windows.length, 1);
});
test('a model no group claims keeps every window, and the group is named when exhausted', () => {
  assert.equal(Q.scoped(agy, 'mystery-model'), agy);
  assert.equal(Q.scoped(agy, ''), agy);
  assert.equal(Q.level(Q.scoped(agy, 'mystery-model')), 'out');
  assert.equal(Q.summary(agy), 'Exhausted · Gemini Models');
  const codex = quota({ used_pct: 50 });
  assert.equal(Q.scoped(codex, 'gpt-5'), codex);
});
