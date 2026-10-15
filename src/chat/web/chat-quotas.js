"use strict";
// Pure presentation rules shared by the chat and its tests. Unknown is not zero.
const ChatQuotas = (() => {
  const numeric = (n) => typeof n === "number" && Number.isFinite(n) && n >= 0;
  const time = (s) => s ? Date.parse(s) : NaN;
  const percent = (n) => numeric(n) ? n.toFixed(1).replace(/\.0$/, "") + "%" : "";
  const fmt = (n) => n >= 1000 ? (n / 1000).toFixed(1).replace(/\.0$/, "") + "k" : String(n);
  const windowLabel = (w) => (w.window_minutes === 300 ? "5h" : w.window_minutes === 10080 ? "wk" : w.label || "Window") + (w.bucket ? " · " + w.bucket : "");
  function windowLevel(w) {
    if (w.provider_status === "rejected" || numeric(w.used_pct) && w.used_pct >= 100) return "out";
    if (w.provider_status === "allowed_warning" || numeric(w.used_pct) && w.used_pct >= 80) return "warn";
    return "normal";
  }
  function critical(q) {
    const rank = { out: 3, warn: 2, normal: 1 };
    return [...(q?.windows || [])].sort((a,b) => rank[windowLevel(b)] - rank[windowLevel(a)] || (b.used_pct ?? -1) - (a.used_pct ?? -1))[0];
  }
  function primary(q) {
    const rank = { out: 3, warn: 2, normal: 1 };
    const duration = w => numeric(w.window_minutes) ? w.window_minutes : Number.POSITIVE_INFINITY;
    return [...(q?.windows || [])].sort((a,b) => duration(a) - duration(b)
      || rank[windowLevel(b)] - rank[windowLevel(a)] || (b.used_pct ?? -1) - (a.used_pct ?? -1))[0];
  }
  const level = (q) => critical(q) ? windowLevel(critical(q)) : "normal";
  // Some providers (agy) share a window per model group. A group entry such as
  // "Gemini Pro" applies to a model id when all its words appear in the id.
  const words = (s) => String(s || "").toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
  function appliesTo(w, model) {
    const have = new Set(words(model));
    return !w.models?.length || w.models.some(m => { const t = words(m); return t.length && t.every(x => have.has(x)); });
  }
  // Windows that govern one model. A model no group claims can't be told apart,
  // so every window stays: never hide a limit because a name didn't match.
  function scoped(q, model) {
    const claimed = (q?.windows || []).some(w => w.models?.length && appliesTo(w, model));
    return model && claimed ? { ...q, windows: q.windows.filter(w => appliesTo(w, model)) } : q;
  }
  function resetText(w, now = Date.now()) {
    const reset = time(w?.resets_at);
    if (!Number.isFinite(reset)) return "reset time unknown";
    const date = new Date(reset);
    const clock = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    const diff = reset - now;
    if (diff <= 0) return `Reset at ${clock}`;
    if (diff < 86400000) {
      const minutes = Math.ceil(diff / 60000), hours = Math.floor(minutes / 60);
      return `resets in ${hours ? hours + "h " : ""}${minutes % 60}m (${clock})`;
    }
    return "resets " + date.toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit", month: "short", day: "numeric" });
  }
  function updated(q, now = Date.now()) {
    const elapsed = now - time(q?.fetched_at);
    if (!Number.isFinite(elapsed)) return "No successful reading yet";
    const minutes = Math.max(0, Math.floor(elapsed / 60000));
    return minutes < 1 ? "Updated just now" : `Updated ${minutes} min ago`;
  }
  function summary(q) {
    const w = primary(q), l = w ? windowLevel(w) : "normal";
    if (q?.state === "read_error") return "Couldn't read quota";
    if (!w) return q?.reading ? "Checking…" : q?.reason === "No quota reading yet" ? "No quota reading yet" : "Quota not available";
    if (l === "out") return "Exhausted" + (w.bucket ? " · " + w.bucket : "");
    if (l === "warn" && !numeric(w.used_pct)) return "⚠ Near limit (reported by provider)";
    if (!numeric(w.used_pct)) return "Quota not available";
    return `${l === "warn" ? "⚠ Near limit " : ""}${percent(w.used_pct)} · ${windowLabel(w)}${w.model ? " · " + w.model : ""}`;
  }
  function consumptionText(total) {
    if (!total?.turns) return "No usage yet in this conversation";
    const u = total.usage || {}, reports = total.reports || {};
    const values = [u.input_tokens, u.cache_read_tokens, u.cache_write_tokens].filter(numeric);
    const input = values.length ? values.reduce((a,b) => a+b,0) : null;
    const cached = (numeric(u.cache_read_tokens) ? u.cache_read_tokens : 0)
      + (numeric(u.cache_write_tokens) ? u.cache_write_tokens : 0);
    const lower = (field) => (reports[field] ?? 0) < total.turns ? "≥ " : "";
    const parts = [input == null ? "in n/a" : lower("input_tokens") + fmt(input) + " in" + (cached ? ` (${fmt(cached)} cached)` : ""),
      numeric(u.output_tokens) ? lower("output_tokens") + fmt(u.output_tokens) + " out" : "out n/a",
      numeric(u.cost_usd) ? lower("cost_usd") + "$" + u.cost_usd.toFixed(u.cost_usd < 1 ? 4 : 2) : "cost n/a"];
    if (total.missing_usage) parts.push(`${total.missing_usage} ${total.missing_usage === 1 ? "reply didn't" : "replies didn't"} report usage`);
    return parts.join(" · ");
  }
  const stale = (q, now = Date.now()) => Number.isFinite(time(q?.fetched_at)) && now - time(q.fetched_at) > 15 * 60000;
  const expired = (q, now = Date.now()) => (q?.windows || []).some(w => Number.isFinite(time(w.resets_at)) && time(w.resets_at) <= now);
  function visualStatus(q, inactive = false, now = Date.now()) {
    if (inactive) return { kind: "inactive", label: "Participant inactive" };
    if (!q) return { kind: "unknown", label: "Quota status unavailable" };
    const l = level(q);
    if (l === "out") return { kind: "out", label: "Quota exhausted" };
    if (l === "warn") return { kind: "warn", label: "Near usage limit" };
    // A failed refresh keeps the last successful reading. Do not turn every
    // participant/reply grey while that reading is still current; the quota
    // panel separately exposes the failed attempt and its error.
    if (stale(q, now) || expired(q, now)) return { kind: "stale", label: "Quota status is outdated" };
    const w = primary(q);
    if (!w || !numeric(w.used_pct)) {
      return q.reading ? { kind: "unknown", label: "Checking quota" }
        : { kind: "unavailable", label: "Quota unavailable" };
    }
    if (!q.fetched_at) return { kind: "unknown", label: "Quota status unavailable" };
    return { kind: "normal", label: q.state === "read_error" ? "Quota normal; latest refresh failed" : "Quota normal" };
  }
  function shouldRetryExpired(q, attempts, now = Date.now(), max = 2, interval = 60000) {
    const keys = (q?.windows || []).filter(w => Number.isFinite(time(w.resets_at)) && time(w.resets_at) <= now)
      .map(w => `${w.id || w.label || "window"}\n${w.resets_at}`);
    const current = new Set(keys);
    for (const key of attempts.keys()) if (!current.has(key)) attempts.delete(key);
    const due = keys.filter(key => {
      const attempt = attempts.get(key);
      return !attempt || attempt.count < max && now - attempt.last >= interval;
    });
    if (!due.length) return false;
    due.forEach(key => {
      const attempt = attempts.get(key) || { count: 0, last: 0 };
      attempts.set(key, { count: attempt.count + 1, last: now });
    });
    return true;
  }
  return { numeric, percent, windowLabel, windowLevel, critical, primary, level, scoped, resetText, updated, summary, consumptionText, stale, expired, visualStatus, shouldRetryExpired };
})();
if (typeof module !== "undefined") module.exports = ChatQuotas;
