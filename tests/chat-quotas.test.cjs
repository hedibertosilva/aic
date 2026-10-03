const { test } = require('node:test');
const assert = require('node:assert/strict');
const Q = require('../src/web/chat-quotas.js');
const quota = (...windows) => ({ state: 'available', windows });
test('alerts use inclusive thresholds and select the most critical window', () => {
  assert.equal(Q.level(quota({ used_pct: 79.9 })), 'normal');
  assert.equal(Q.level(quota({ used_pct: 80 })), 'warn');
  assert.equal(Q.level(quota({ used_pct: 100 })), 'out');
  const q = quota({ used_pct: 41, window_minutes: 300 }, { used_pct: 86, window_minutes: 10080 });
  assert.equal(Q.critical(q).window_minutes, 10080);
  assert.equal(Q.summary(q), '⚠ Near limit 86% · wk');
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
