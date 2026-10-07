const { test } = require('node:test');
const assert = require('node:assert/strict');
const policy = require('../src/chat/web/chat-onboarding.js');
const report = { agents: [{agent:'codex',readiness:'ready'}, {agent:'claude',readiness:'blocked'}] };
test('new conversations never inherit another default or select a sole agent silently', () => {
  assert.equal(policy.initial(report,''),'');
  assert.equal(policy.initial(report,'claude'),'');
  assert.equal(policy.initial(report,'missing'),'');
  assert.equal(policy.initial(report,'codex'),'codex');
  assert.equal(policy.selection(report,'claude').reason,'preferred_unavailable');
  assert.equal(policy.selection(report,'').reason,'confirmation_required');
  assert.deepEqual(policy.selection({agents:[]},'').ready,[]);
  assert.equal(policy.selection({agents:[]},'').reason,'none_ready');
});
test('only ready agents are selectable even with several configured integrations', () => {
  const many={agents:[...report.agents,{agent:'gemini',readiness:'ready'}]};
  assert.deepEqual(policy.selection(many,'').ready,['codex','gemini']);
});
test('branch changes require acknowledgement of each transition including returning to origin', () => {
  const origin={checkout:'/repo',branch:'main',sha:'aaa'}, next={checkout:'/repo',branch:'feature',sha:'aaa'};
  assert.equal(policy.branchChanged(origin,next),true);
  assert.equal(policy.branchChanged(origin,next,policy.gitKey(next)),false);
  assert.equal(policy.branchChanged(origin,origin,policy.gitKey(next)),true);
  const detached={checkout:'/repo',branch:null,sha:'bbb'};
  assert.equal(policy.branchChanged(origin,detached,policy.gitKey(next)),true);
  assert.notEqual(policy.gitKey(origin),policy.gitKey({...origin,checkout:'/another-worktree'}));
});
