// Shared pure policy, usable from Node tests and the browser.
(function (root) {
  const policy = {
    ready: (report, name) => !!report?.agents?.some(a => a.agent === name && a.readiness === 'ready'),
    initial: (report, preference) => policy.ready(report, preference) ? preference : '',
    selection: (report, preference) => {
      const ready = (report?.agents || []).filter(a => a.readiness === 'ready').map(a => a.agent);
      return { ready, selected: policy.initial(report, preference), reason: preference && !policy.ready(report, preference) ? 'preferred_unavailable' : ready.length ? 'confirmation_required' : 'none_ready' };
    },
    environment: (report, preference, participants = []) => {
      if (!report) return { state: 'unknown', pending: 0 };
      const infrastructure = [report.repository, report.runtime, report.image];
      const infrastructurePending = infrastructure.filter(check => check && !['ready', 'not_required'].includes(check.state)).length;
      const relevant = new Set([preference, ...participants.map(p => p.agent)].filter(Boolean));
      const agentPending = [...relevant].filter(name => !policy.ready(report, name)).length;
      const anyReady = (report.agents || []).some(a => a.readiness === 'ready');
      const pending = Math.max(infrastructurePending + agentPending, anyReady ? 0 : 1);
      return { state: infrastructurePending || !anyReady ? 'blocked' : pending ? 'pending' : 'ready', pending };
    },
    gitKey: git => git ? `${git.checkout}\n${git.branch || '@' + git.sha}` : '',
    branchChanged: (origin, current, acknowledged) => !!origin && (acknowledged || policy.gitKey(origin)) !== policy.gitKey(current),
  };
  if (typeof module === 'object' && module.exports) module.exports = policy;
  else root.ChatOnboarding = policy;
})(typeof window === 'object' ? window : globalThis);
