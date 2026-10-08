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
test('Environment counts infrastructure, default and participant agents, ignoring optional integrations and quota', () => {
  const current = { repository: {state:'ready'}, runtime: {state:'ready'}, image: {state:'not_required'},
    agents: [...report.agents, {agent:'gemini',readiness:'unknown'}] };
  assert.deepEqual(policy.environment(null,'codex'), {state:'unknown',pending:0});
  assert.deepEqual(policy.environment(current,'codex',[{agent:'codex'}]), {state:'ready',pending:0});
  assert.deepEqual(policy.environment(current,'claude',[{agent:'codex'}]), {state:'pending',pending:1});
  assert.deepEqual(policy.environment(current,'codex',[{agent:'claude'},{agent:'claude'}]), {state:'pending',pending:1});
  assert.deepEqual(policy.environment(current,'codex',[{agent:'removed'}]), {state:'pending',pending:1});
  assert.deepEqual(policy.environment({...current,runtime:{state:'missing'}},'codex'), {state:'blocked',pending:1});
  assert.deepEqual(policy.environment({...current,agents:[]},''), {state:'blocked',pending:1});
  assert.deepEqual(policy.environment({...current,agents:[{agent:'codex',readiness:'ready',quota:'exhausted'}]},'codex'), {state:'ready',pending:0});
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
