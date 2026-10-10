"use strict";
/* aic chat — the page script for chat.html. Model text only ever reaches the
   DOM through textContent / createTextNode, never innerHTML. */

const $ = (id) => document.getElementById(id);
const KEY = "aic-chat-v2";
const DRAFTS_KEY = "aic-chat-drafts";
const DRAFT_PASTES_KEY = "aic-chat-draft-pastes";
const THEME_KEY = "aic-chat-theme";
let state = { convs: [], current: null, layout: "auto" };
let drafts = {}; // conversation id -> unsent text, saved apart from the (big) state
let draftPastes = {}; // conversation id -> text hidden behind movable paste markers
let meta = { agents: [], details: {}, default: "", repo: "", sandboxed: false };
let serverCheckout = "";
let readiness = null, gitContext = null;
let environmentFocus = null;
let skills = []; // [{ name, description, argument_hint, requires_write }]
const running = new Map(); // `${conv id}\n${participant}` -> AbortController
const cardParts = new WeakMap(); // log entry -> DOM parts of its card
const MAX_PARTICIPANTS = 5;
let quotaData = { agents: [], consumption: {} }, quotaConversation = null, quotaRequest = null;
const quotaExpanded = new Set(), quotaLevels = new Map(), quotaExpiryAttempts = new Map();
const quotaManualReads = new Set();
let quotaRefreshPending = false;

/* ---------- small helpers ---------- */
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
const I = (d) => `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${d}</svg>`;
const ICONS = {
  copy: I('<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>'),
  check: I('<path d="M5 12.5l4.5 4.5L19 7"/>'),
  retry: I('<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 4v7h-7"/>'),
  stop: I('<rect x="7" y="7" width="10" height="10" rx="1.5" fill="currentColor"/>'),
  edit: I('<path d="M13 5H6a2 2 0 0 0-2 2v11a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2v-7"/><path d="M10 16l1-4 6.8-6.8a2.1 2.1 0 0 1 3 3L14 15z"/>'),
  close: I('<path d="M6 6l12 12M18 6L6 18"/>'),
  menu: I('<path d="M4 7h16M4 12h16M4 17h16"/>'),
  plus: I('<path d="M12 5v14M5 12h14"/>'),
  down: I('<path d="M12 5v14M6 13l6 6 6-6"/>'),
  send: I('<path d="M21 3 10.7 13.3M21 3l-6.5 18-3.8-7.7L3 9.5z"/>'),
  trash: I('<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>'),
  expand: I('<path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"/>'),
  collapse: I('<path d="M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7"/>'),
  pause: I('<path d="M8 5v14M16 5v14"/>'),
  play: I('<path d="M7 4.5v15l12-7.5z"/>'),
  spark: I('<path d="M11 3l1.9 5.6L18.5 10.5l-5.6 1.9L11 18l-1.9-5.6L3.5 10.5l5.6-1.9z"/><path d="M19 15v4M17 17h4"/>'),
  more: I('<circle cx="5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="19" cy="12" r="1.2"/>'),
  columns: I('<rect x="3" y="5" width="7.5" height="14" rx="1.5"/><rect x="13.5" y="5" width="7.5" height="14" rx="1.5"/>'),
  stack: I('<rect x="4" y="3.5" width="16" height="7.5" rx="1.5"/><rect x="4" y="13" width="16" height="7.5" rx="1.5"/>'),
  comment: I('<path d="M4 5h11a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H9l-4 3v-3H4z"/><path d="M17 9h3v8h-1v3l-3-3h-4"/>'),
  download: I('<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>'),
  alert: I('<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17v.5"/>'),
  rename: I('<path d="M4 20h16"/><path d="M6 16l9.5-9.5 3 3L9 19H6z"/>'),
  settings: I('<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.12 2.12-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.04 1.56V20h-3v-.08A1.7 1.7 0 0 0 10.66 18.36a1.7 1.7 0 0 0-1.88.34l-.06.06-2.12-2.12.06-.06A1.7 1.7 0 0 0 7 14.7 1.7 1.7 0 0 0 5.44 13.66H5v-3h.08A1.7 1.7 0 0 0 6.64 9.62 1.7 1.7 0 0 0 6.3 7.74l-.06-.06 2.12-2.12.06.06A1.7 1.7 0 0 0 10.3 6 1.7 1.7 0 0 0 11.34 4.44V4h3v.08A1.7 1.7 0 0 0 15.38 5.64a1.7 1.7 0 0 0 1.88-.34l.06-.06 2.12 2.12-.06.06A1.7 1.7 0 0 0 19 9.3a1.7 1.7 0 0 0 1.56 1.04H21v3h-.08A1.7 1.7 0 0 0 19.4 15z"/>'),
  meter: I('<path d="M4 18a9 9 0 1 1 16 0"/><path d="M12 13l5-5"/><circle cx="12" cy="13" r="1.5"/>'),
  branch: I('<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M8 15h2a8 8 0 0 0 8-5"/>'),
  plan: I('<rect x="5" y="4" width="14" height="16" rx="1.5"/><path d="M9 9h6M9 13h6M9 17h4"/>'),
  history: I('<path d="M4 7v5h5"/><path d="M5.2 16.5A8 8 0 1 0 4.7 8"/><path d="M12 8v5l3 2"/>'),
  sun: I('<circle cx="12" cy="12" r="3.5"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>'),
  moon: I('<path d="M20.5 15.2A8.5 8.5 0 0 1 8.8 3.5 8.5 8.5 0 1 0 20.5 15.2z"/>'),
};
function icon(kind) {
  const t = document.createElement("template");
  t.innerHTML = ICONS[kind]; // static markup above, never model text
  return t.content.firstChild;
}
function iconBtn(kind, label, onclick, text) {
  const b = el("button", "icon"); b.type = "button";
  b.appendChild(icon(kind));
  if (text) b.appendChild(document.createTextNode(text));
  b.title = label; b.setAttribute("aria-label", label); b.onclick = onclick;
  return b;
}
function setIcon(btn, kind) { btn.replaceChildren(icon(kind)); }

async function copyText(text, btn) {
  try { await navigator.clipboard.writeText(text); }
  catch {
    const t = el("textarea"); t.value = text; t.style.position = "fixed"; t.style.opacity = "0";
    document.body.appendChild(t); t.select(); try { document.execCommand("copy"); } catch {} t.remove();
  }
  if (btn) { setIcon(btn, "check"); setTimeout(() => setIcon(btn, "copy"), 1400); }
}

function toast(text, undo, actionLabel = 'Undo') {
  $("toasttext").textContent = text;
  $("undo").hidden = !undo;
  $("undo").textContent = actionLabel;
  $("undo").onclick = () => { $("toast").classList.remove("show"); undo && undo(); };
  $("toast").classList.add("show");
  clearTimeout(toast.t); toast.t = setTimeout(() => $("toast").classList.remove("show"), 8000);
}
const announce = (text) => { $("announce").textContent = text; };
const uniq = (xs) => [...new Set(xs)];
const fmtK = (n) => (n >= 1000 ? (n / 1000).toFixed(n >= 10000 ? 0 : 1) + "k" : String(n));

/* ---------- persistence ---------- */
// localStorage is retained only as an offline/startup cache. The local aic
// server owns the durable copy in .aic/chat/chat.sqlite3.
let remoteReady = false, remoteSaving = false, remoteDirty = false;
let remoteRevision = null, remoteConflict = false;
function loadLocal() {
  try { state = Object.assign(state, JSON.parse(localStorage.getItem(KEY)) || {}); } catch {}
  try { drafts = JSON.parse(localStorage.getItem(DRAFTS_KEY)) || {}; } catch {}
  const cachedPastes = state.pastedDrafts && typeof state.pastedDrafts === "object" ? state.pastedDrafts : {};
  try { draftPastes = { ...cachedPastes, ...(JSON.parse(localStorage.getItem(DRAFT_PASTES_KEY)) || {}) }; } catch { draftPastes = cachedPastes; }
  if (!draftPastes || typeof draftPastes !== "object" || Array.isArray(draftPastes)) draftPastes = {};
  state.pastedDrafts = draftPastes;
  normalizeState();
}
// Run this after loading either cache or SQLite. Reconnectable runs stay
// pending across a reload; legacy pending entries have no server-side event
// log and are migrated to the old interrupted state.
function normalizeState() {
  if (!Array.isArray(state.convs)) state.convs = [];
  if (state.stack) state.layout = "stack";
  delete state.stack;
  state.convs.forEach((c) => {
    if (c.plan_binding) hydratePlanBinding(c, c.plan_binding);
    if (c.draft && !drafts[c.id]) drafts[c.id] = c.draft;
    delete c.draft;
    c.log.forEach((e) => {
      if (e.pending && !e.run_id) { e.pending = false; if (!e.text) { e.error = true; e.soft = true; e.text = "interrupted"; } }
      if (e.pending && e.run_id) e.reconnecting = true;
    });
  });
  // Older sessions may already contain placeholder conversations created by
  // opening the chat and leaving it immediately. They have no chat entries,
  // so remove them during migration too.
  pruneEmptyConversations();
}
function storeItem(key, value) {
  try { localStorage.setItem(key, value); return true; }
  catch (err) {
    const b = $("banner");
    b.replaceChildren(icon("alert"), el("span", "message",
      "Couldn't save your conversations (" + (err && err.name === "QuotaExceededError" ? "browser storage is full" : String(err && err.message || err)) +
      "). New messages will be lost when you close this tab: export and delete old conversations to free space."));
    b.hidden = false;
    return false;
  }
}
function save() {
  clearTimeout(save.t); save.t = 0;
  if (storeItem(KEY, JSON.stringify(state))) $("banner").hidden = true;
  saveRemoteSoon();
}
function saveSoon() { clearTimeout(save.t); save.t = setTimeout(save, 500); }
function saveDrafts() {
  clearTimeout(saveDrafts.t); saveDrafts.t = 0;
  state.pastedDrafts = draftPastes;
  storeItem(DRAFTS_KEY, JSON.stringify(drafts));
  storeItem(DRAFT_PASTES_KEY, JSON.stringify(draftPastes));
  saveRemoteSoon();
}
function saveDraftsSoon() { clearTimeout(saveDrafts.t); saveDrafts.t = setTimeout(saveDrafts, 400); }
function remoteError(err) {
  const b = $("banner");
  b.replaceChildren(icon("alert"), el("span", "message", "Couldn't save your conversations to .aic/chat: " + (err.message || err) + ". This browser still has a temporary copy."));
  b.hidden = false;
}
// Only one request may be in flight. A later edit marks the state dirty and
// is sent after it, preventing an older request from winning a write race.
async function saveRemote() {
  if (!remoteReady || remoteSaving || !remoteDirty || remoteConflict) return;
  remoteSaving = true; remoteDirty = false;
  let succeeded = false;
  try {
    const res = await fetch("/api/chat/state", {
      method: "POST", headers: { "Content-Type": "application/json" },
      // `keepalive` limits request bodies to roughly 64 KiB in browsers. A
      // conversation snapshot can be much larger, so normal saves must use a
      // regular fetch; localStorage remains the abrupt-close fallback.
      body: JSON.stringify({ state, drafts, ...(Number.isSafeInteger(remoteRevision) ? {revision: remoteRevision} : {}) }),
    });
    if (!res.ok) { if (res.status === 409) remoteConflict = true; throw new Error(await res.text()); }
    const result = await res.json();
    if (Number.isSafeInteger(result.revision)) remoteRevision = result.revision;
    $("banner").hidden = true;
    succeeded = true;
  } catch (err) {
    remoteError(err);
  } finally {
    remoteSaving = false;
    if (remoteDirty) saveRemote();
  }
  return succeeded;
}
function saveRemoteSoon() {
  if (!remoteReady) return;
  remoteDirty = true;
  clearTimeout(saveRemoteSoon.t);
  saveRemoteSoon.t = setTimeout(saveRemote, 250);
}
window.addEventListener("pagehide", () => {
  if (save.t) save(); if (saveDrafts.t) saveDrafts();
  // The local cache still covers an abrupt browser close.
  saveRemote();
});

const cur = () => state.convs.find((c) => c.id === state.current);
const hasInteraction = (c) => !!c.keepEmpty || !!c.planBinding?.plan_id || (Array.isArray(c.log) && c.log.length > 0);
function pruneEmptyConversations() {
  const empty = new Set(state.convs.filter((c) => !hasInteraction(c)).map((c) => c.id));
  if (!empty.size) return false;
  state.convs = state.convs.filter((c) => !empty.has(c.id));
  empty.forEach((id) => { delete drafts[id]; });
  if (empty.has(state.current)) state.current = null;
  saveDraftsSoon();
  return true;
}
function discardEmptyConversation(c) {
  if (!c || hasInteraction(c)) return false;
  state.convs = state.convs.filter((x) => x !== c);
  delete drafts[c.id];
  if (state.current === c.id) state.current = null;
  saveDraftsSoon();
  return true;
}
const runKey = (c, p) => c.id + "\n" + p.name;
const isRunning = (c, p) => running.has(runKey(c, p));
const busy = (c) => !!c && [...running.keys()].some((k) => k.startsWith(c.id + "\n"));
const kindOf = (p) => meta.details?.[p.agent]?.kind || "claude";

/* ---------- colours ---------- */
// One persistent hue identifies a participant. Each option has a deliberately
// darker light-theme colour and a brighter dark-theme counterpart so names,
// borders and avatars retain contrast when the OS theme changes.
const PARTICIPANT_PALETTE = [
  { hue: 210, name: "Blue",    light: "#1d4ed8", dark: "#60a5fa" },
  { hue: 25,  name: "Orange",  light: "#c2410c", dark: "#fb923c" },
  { hue: 135, name: "Green",   light: "#15803d", dark: "#4ade80" },
  { hue: 290, name: "Purple",  light: "#7e22ce", dark: "#c084fc" },
  { hue: 350, name: "Rose",    light: "#be123c", dark: "#fb7185" },
  { hue: 180, name: "Teal",    light: "#0f766e", dark: "#2dd4bf" },
  { hue: 55,  name: "Amber",   light: "#a16207", dark: "#facc15" },
  { hue: 250, name: "Indigo",  light: "#4338ca", dark: "#818cf8" },
  { hue: 195, name: "Cyan",    light: "#0e7490", dark: "#22d3ee" },
  { hue: 320, name: "Fuchsia", light: "#a21caf", dark: "#e879f9" },
  { hue: 90,  name: "Lime",    light: "#4d7c0f", dark: "#a3e635" },
  { hue: 220, name: "Slate",   light: "#475569", dark: "#94a3b8" },
];
const PALETTE = PARTICIPANT_PALETTE.map((colour) => colour.hue);
function hashHue(name) { let h = 0; for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) % 360; return h; }
const hueOf = (p) => (typeof p.hue === "number" ? p.hue : hashHue(p.name)); // old conversations: hash
function hueFor(c, name) { const p = c.participants.find((x) => x.name === name); return p ? hueOf(p) : hashHue(name); }
const paletteColour = (hue) => PARTICIPANT_PALETTE.find((colour) => colour.hue === hue);
function applyParticipantColour(node, hue) {
  node.style.setProperty("--h", hue);
  const colour = paletteColour(hue);
  if (colour) {
    node.style.setProperty("--participant-light", colour.light);
    node.style.setProperty("--participant-dark", colour.dark);
  }
  return node;
}
const hueGap = (a, b) => { const d = Math.abs(a - b) % 360; return Math.min(d, 360 - d); };
function pickHue(c, except) {
  const used = c.participants.filter((p) => p !== except).map(hueOf);
  const free = PALETTE.find((h) => used.every((u) => hueGap(u, h) >= 15));
  if (free !== undefined) return free;
  let best = 0, bestGap = -1;
  for (let h = 0; h < 360; h += 5) {
    const gap = Math.min(...used.map((u) => hueGap(u, h)));
    if (gap > bestGap) { best = h; bestGap = gap; }
  }
  return best;
}
function avatar(c, name, big) {
  const a = el("span", "avatar" + (big ? " big" : ""), name.charAt(0));
  applyParticipantColour(a, hueFor(c, name)); a.setAttribute("aria-hidden", "true");
  return a;
}
function writeBadge() {
  const b = el("span", "badge writes"); b.title = "can edit files"; b.setAttribute("aria-label", "can edit files");
  b.appendChild(icon("edit"));
  return b;
}

/* ---------- quota cache and presentation ---------- */
// With a model, only the windows that govern it (agy groups quota by model).
const quotaOf = (agent, model) => { const q = quotaData.agents.find(q => q.agent === agent); return q && model ? ChatQuotas.scoped(q, model) : q; };
const quotaModel = (agent, model) => model || meta.details?.[agent]?.model || "";
function quotaVisualStatus(agent, model, inactive = false) {
  if (inactive) return {kind: "inactive", label: "Participant inactive"};
  const q = quotaOf(agent, quotaModel(agent, model)), level = ChatQuotas.level(q);
  if (!q) return {kind: "unknown", label: "Quota status unavailable"};
  if (level === "out") return {kind: "out", label: "Quota exhausted"};
  if (level === "warn") return {kind: "warn", label: "Near usage limit"};
  if (ChatQuotas.stale(q) || ChatQuotas.expired(q) || q.state === "read_error") return {kind: "stale", label: "Quota status is outdated"};
  const primary = ChatQuotas.primary(q);
  if (!primary || !ChatQuotas.numeric(primary.used_pct)) {
    return q.reading ? {kind: "unknown", label: "Checking quota"} : {kind: "unavailable", label: "Quota unavailable"};
  }
  if (!q.fetched_at) return {kind: "unknown", label: "Quota status unavailable"};
  return {kind: "normal", label: "Quota normal"};
}
const quotaUsagePercent = (agent, model) => ChatQuotas.percent(ChatQuotas.primary(quotaOf(agent, quotaModel(agent, model)))?.used_pct);
function quotaStatusDot(status, className) {
  const dot = el("span", className + " status-" + status.kind);
  dot.title = status.label; dot.setAttribute("aria-label", status.label); return dot;
}
function repaintQuotaIndicators() {
  renderBar(); renderComposer();
  const c = cur(); if (c) c.log.forEach(entry => { if (entry.from !== "you") paintCard(c, entry); });
}
function quotaAlert(agent, model) {
  const q = quotaOf(agent, model), level = ChatQuotas.level(q);
  if (level === "normal") return "";
  const w = ChatQuotas.critical(q);
  return `${cap(agent)} ${level === "out" ? "quota exhausted" : "near usage limit"} · ${ChatQuotas.resetText(w)} · ${ChatQuotas.updated(q)}`;
}
function quotaBadge(agent, model) {
  const q = quotaOf(agent, model), level = ChatQuotas.level(q);
  if (level === "normal") return null;
  const w = ChatQuotas.critical(q);
  const b = el("span", "badge quota-" + level, level === "out" ? "Exhausted" : ChatQuotas.percent(w.used_pct) || "Near limit");
  b.title = quotaAlert(agent, model); b.setAttribute("aria-label", b.title);
  return b;
}
function quotaMeter(w, name) {
  if (!ChatQuotas.numeric(w?.used_pct)) return null;
  const meter = el("div", "meter");
  meter.setAttribute("role", "meter"); meter.setAttribute("aria-label", `${name} ${w.label || "usage"}`);
  meter.setAttribute("aria-valuemin", "0"); meter.setAttribute("aria-valuemax", "100");
  meter.setAttribute("aria-valuenow", String(Math.min(100,w.used_pct)));
  meter.setAttribute("aria-valuetext", `${ChatQuotas.percent(w.used_pct)} of ${w.label || "usage limit"} used, ${ChatQuotas.resetText(w)}`);
  meter.style.setProperty("--pct", Math.min(100,w.used_pct) + "%");
  meter.appendChild(el("i")); return meter;
}
async function fetchQuotaState() {
  if (quotaRequest || !meta.agents.length) return quotaRequest;
  const conversation = cur()?.id || "";
  quotaRequest = (async () => {
    try {
      const res = await fetch("/api/quotas?conversation=" + encodeURIComponent(conversation));
      if (!res.ok) throw new Error("Couldn't read usage limits");
      const data = await res.json();
      const changed = JSON.stringify(data.agents) !== JSON.stringify(quotaData.agents);
      quotaData.agents = data.agents || [];
      if ((cur()?.id || "") === conversation) { quotaData.consumption = data.consumption || {}; quotaConversation = conversation; }
      const notices = [];
      quotaData.agents.forEach(q => {
        if (!q.reading) quotaManualReads.delete(q.agent);
        const level = ChatQuotas.level(q), before = quotaLevels.get(q.agent);
        if (before !== level && level !== "normal") notices.push(quotaAlert(q.agent));
        quotaLevels.set(q.agent, level);
      });
      if (notices.length) announce(notices.join(". "));
      if (changed) repaintQuotaIndicators();
      renderQuotas();
    } catch {
      quotaManualReads.clear();
      // A transport failure keeps old readings visible without claiming freshness.
      quotaData.agents = meta.agents.map(agent => ({ ...(quotaOf(agent) || { agent, windows: [] }),
        refreshable: meta.details?.[agent]?.kind === "codex", state: "read_error", reading: false, error: "Couldn't reach the quota reader", attempted_at: new Date().toISOString() }));
      repaintQuotaIndicators();
      renderQuotas();
    } finally {
      quotaRequest = null;
      if ((cur()?.id || "") !== conversation) fetchQuotaState();
    }
  })();
  return quotaRequest;
}
async function refreshQuotas(agent, silent = false) {
  if (quotaRefreshPending && !agent) return;
  quotaRefreshPending = true;
  if (!silent) (agent ? [agent] : quotaData.agents.filter(q => q.refreshable).map(q => q.agent)).forEach(a => quotaManualReads.add(a));
  renderQuotas();
  try {
    const res = await fetch("/api/quotas/refresh", { method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ agent: agent || null }) });
    if (!res.ok) throw new Error(await res.text());
    // A snapshot begun before the POST cannot establish the refresh result.
    if (quotaRequest) await quotaRequest;
    await fetchQuotaState();
  } catch (err) { quotaManualReads.clear(); if (!silent) toast("Couldn't refresh usage limits: " + err.message); }
  finally { quotaRefreshPending = false; renderQuotas(); }
}
function renderQuotas() {
  const c = cur(), sameConversation = quotaConversation === (c?.id || "");
  const rows = $("quotarows"), focused = document.activeElement?.dataset.quotaFocus;
  rows.replaceChildren();
  const levels = quotaData.agents.map(ChatQuotas.level);
  const level = levels.includes("out") ? "out" : levels.includes("warn") ? "warn" : "";
  if (level) $("quotabtn").dataset.level = level; else delete $("quotabtn").dataset.level;
  $("quotabtn").setAttribute("aria-label", "Usage limits" + (level === "out" ? ": an agent is exhausted" : level ? ": an agent is near its limit" : ""));
  const reading = quotaRefreshPending || quotaData.agents.some(q => q.reading);
  $("quotarefresh").disabled = reading; $("quotarefresh").classList.toggle("spinning", quotaManualReads.size > 0);
  $("quotas").setAttribute("aria-busy", String(reading));
  $("quotasubtitle").textContent = reading ? "Updating provider usage…"
    : level === "out" ? "An agent has exhausted its quota"
    : level === "warn" ? "An agent is nearing its limit"
    : "Provider limits and conversation usage";
  meta.agents.forEach(agent => {
    const q = quotaOf(agent) || { agent, windows: [], reason: "No quota reading yet" };
    const quotaLevel = ChatQuotas.level(q), visualStatus = quotaVisualStatus(agent, ""), w = ChatQuotas.primary(q), stale = ChatQuotas.stale(q) || ChatQuotas.expired(q) || q.state === "read_error";
    const row = el("div", "quota " + quotaLevel + (stale ? " stale" : "") + (visualStatus.kind === "unavailable" ? " unavailable" : ""));
    const head = el("div", "head"), details = el("div", "quota-details");
    const agentName = el("span", "quota-agent"), stateDot = el("span", "quota-state");
    stateDot.setAttribute("aria-hidden", "true"); agentName.append(stateDot, el("strong", "", cap(agent))); head.appendChild(agentName);
    if (q.reading && (!q.fetched_at || quotaManualReads.has(agent))) { const dots = el("span", "dots"); dots.setAttribute("aria-label", "Reading quota"); head.appendChild(dots); }
    const summaryText = ChatQuotas.summary(q), summary = el("span", "val");
    if (summaryText.startsWith("⚠")) summary.append(icon("alert"), document.createTextNode(summaryText.replace(/^⚠\s*/, "")));
    else summary.textContent = summaryText;
    head.appendChild(summary);
    const toggle = iconBtn("down", `Details for ${cap(agent)}`, () => {
      if (quotaExpanded.has(agent)) quotaExpanded.delete(agent); else quotaExpanded.add(agent);
      details.hidden = !quotaExpanded.has(agent); toggle.setAttribute("aria-expanded", String(!details.hidden));
    });
    toggle.dataset.quotaFocus = agent + ":details";
    toggle.setAttribute("aria-expanded", String(quotaExpanded.has(agent))); head.appendChild(toggle);
    row.appendChild(head);
    const quota = el("div", "sub quota-reading");
    const meter = quotaMeter(w, cap(agent)); if (meter) quota.appendChild(meter);
    quota.appendChild(el("span", "", w ? ChatQuotas.resetText(w) + (ChatQuotas.expired(q) && q.reading ? " · refreshing…" : "")
      : !q.fetched_at && meta.details?.[agent]?.kind === "claude" ? "Claude quota integration is planned for phase 2" : q.reason || "Quota not available"));
    if (q.fetched_at) {
      const age = el("span", "quota-age" + (ChatQuotas.stale(q) ? " old" : ""), " · " + ChatQuotas.updated(q));
      age.title = "Source: " + q.source; quota.appendChild(age);
    }
    if (q.state === "read_error") {
      quota.appendChild(el("span", "quota-error", " · " + (q.error || "Couldn't refresh")));
      if (q.refreshable) {
        const retry = el("button", "quota-retry", "Retry"); retry.type = "button";
        retry.disabled = !!q.reading; retry.dataset.quotaFocus = agent + ":retry"; retry.onclick = () => refreshQuotas(agent); quota.appendChild(retry);
      }
    }
    row.appendChild(quota);
    const consumption = el("div", "sub quota-consumption");
    consumption.append(el("span", "quota-consumption-label", "This conversation"), el("strong", "", sameConversation ? ChatQuotas.consumptionText(quotaData.consumption[agent]) : "Loading usage…"));
    consumption.title = "Spent by aic in this conversation, including regenerated and deleted replies"; row.appendChild(consumption);
    details.hidden = !quotaExpanded.has(agent);
    (q.windows || []).forEach(window => {
      const line = el("div", "quota-window " + ChatQuotas.windowLevel(window));
      line.appendChild(el("span", "", (window.bucket ? window.bucket + " · " : "") + (window.model ? window.model + " · " : "") + window.label + ": " + (ChatQuotas.percent(window.used_pct) || "n/a")));
      const meter = quotaMeter(window, cap(agent)); if (meter) line.appendChild(meter);
      line.appendChild(el("div", "sub", ChatQuotas.resetText(window))); details.appendChild(line);
    });
    const provenance = el("div", "sub quota-source", "Source: " + (q.source || "No quota source available") + " · " + ChatQuotas.updated(q));
    if (ChatQuotas.stale(q)) provenance.classList.add("old");
    if (q.fetched_at) provenance.appendChild(el("div", "", "Last read " + new Date(q.fetched_at).toLocaleString()));
    if (q.state === "read_error" && q.attempted_at) provenance.appendChild(el("div", "", "Couldn't refresh at " + new Date(q.attempted_at).toLocaleString()));
    details.appendChild(provenance); row.appendChild(details); rows.appendChild(row);
    for (const option of $("pagent").options) if (option.value === agent) {
      const qd = quotaOf(agent, quotaModel(agent, "")), wd = ChatQuotas.critical(qd), level = ChatQuotas.level(qd);
      option.textContent = cap(agent) + (level === "out" ? " — exhausted" : level === "warn" ? " — " + (ChatQuotas.percent(wd?.used_pct) || "near limit") : "");
    }
  });
  $("quotaunknown").hidden = !c?.log.some(e => e.from !== "you" && !e.agent);
  if (focused) [...rows.querySelectorAll("[data-quota-focus]")].find(e => e.dataset.quotaFocus === focused)?.focus();
  if (!sameConversation && !quotaRequest && meta.agents.length) fetchQuotaState();
}

/* ---------- participants / conversations ---------- */
const NAME_OK = /^[\w-]+$/;
const cleanName = (s) => s.trim().replace(/\s+/g, "-").replace(/[^\w-]/g, "");
const cap = (s) => s.charAt(0).toUpperCase() + s.slice(1); // "claude" -> "Claude", "opus" -> "Opus"
function uniqueName(c, base) {
  let name = base, n = 2;
  while (c.participants.some((p) => p.name === name)) name = base + "-" + n++;
  return name;
}
function addParticipant(c, f) {
  const p = { name: uniqueName(c, cleanName(f.name || cap(f.agent)) || "agent"), agent: f.agent, role: f.role || "", write: !!f.write,
    model: f.model || "", effort: f.effort || "", session: "", seen: 0 };
  p.hue = typeof f.hue === "number" ? f.hue : pickHue(c);
  c.participants.push(p);
  return p;
}
const defaultAgent = () => ChatOnboarding.initial(readiness, meta.default);
async function newConv() {
  try { await refreshReadiness(); } catch { readiness = {agents: []}; }
  const c = { id: crypto.randomUUID(), title: "New conversation", participants: [], log: [], round: 0, updated: Date.now(), sendTo: null, checkout: cur()?.checkout || null, gitOrigin: gitContext ? structuredClone(gitContext) : null };
  const agent = defaultAgent();
  if (agent) addParticipant(c, { agent });
  state.convs.unshift(c);
  $("search").value = "";
  switchTo(c.id);
}
function copyParticipants(source, seen) {
  // Native CLI sessions cannot be forked. Start fresh sessions in the new
  // conversation, while preserving every participant setting.
  return source.participants.map((p) => ({
    name: p.name, agent: p.agent, role: p.role || "", write: !!p.write,
    model: p.model || "", effort: p.effort || "", hue: hueOf(p), session: "", seen,
    ...(p.inactive ? {inactive: true} : {}),
  }));
}
function copyLog(log) {
  return log.map((e) => ({
    ...e,
    ...(e.to ? { to: [...e.to] } : {}),
    ...(e.actions ? { actions: [...e.actions] } : {}),
    ...(e.usage ? { usage: { ...e.usage } } : {}),
  }));
}
async function copyConversation(source, withHistory) {
  if (withHistory && busy(source)) {
    toast("Wait for replies to finish before forking this conversation.");
    return;
  }
  if (withHistory) {
    try { await refreshPlanBinding(source); }
    catch (error) { toast(error.message + ' The original conversation is preserved.'); return; }
  }
  const log = withHistory ? copyLog(source.log) : [];
  const agentNames = source.participants.map((p) => p.name).join(", ") || "Agents";
  const c = {
    id: crypto.randomUUID(),
    title: (withHistory ? `Fork: ${source.title}` : agentNames).slice(0, 80),
    checkout: source.checkout || null,
    gitOrigin: source.gitOrigin ? structuredClone(source.gitOrigin) : null,
    participants: copyParticipants(source, log.length),
    log,
    round: withHistory ? source.round || 0 : 0,
    updated: Date.now(),
    sendTo: source.sendTo ? [...source.sendTo] : null,
    // Unlike an untouched “New conversation”, this is deliberately useful
    // without messages: it keeps a reusable group of agents.
    ...(withHistory ? {} : { keepEmpty: true }),
  };
  if (withHistory) c.planOrigin = { conversation_id: source.id, previous_plan_id: source.planBinding?.plan_id || null, message_count: log.length };
  // Planned forks must preserve the server-owned link, never just copy a
  // browser preference. A reusable agent group starts as a free conversation.
  if (withHistory && source.planBinding?.plan_id) {
    try {
      await flushChatForPlanMutation();
      const result = await planRequest(conversationPlanRoute(source, 'fork'), {
        checkout: source.checkout || null, state_revision: remoteRevision, with_history: true,
      });
      const fork = verifiedPlanFork(result);
      adoptPlanMutation(result, fork, source);
      return;
    } catch (error) { toast(error.message); return; }
  }
  state.convs.unshift(c);
  $("search").value = "";
  switchTo(c.id);
  toast(withHistory
    ? "Fork created. Agents start fresh sessions with this conversation as context."
    : "Conversation created with the same agents.");
}
const cloneAgents = (c) => copyConversation(c, false);
const forkConversation = (c) => copyConversation(c, true);
// /clear: empties the conversation and puts every participant on a fresh native session. The participants stay.
function clearConversation(c) {
  if (!c.log.length) { toast("Nothing to clear."); return; }
  const saved = { log: c.log, round: c.round, title: c.title, updated: c.updated, people: c.participants.map((p) => [p, p.session, p.seen]) };
  c.log = []; c.round = 0; c.updated = Date.now();
  if (!c.renamed) c.title = "New conversation";
  c.participants.forEach((p) => { p.session = ""; p.seen = 0; });
  save(); render(true);
  toast("Conversation cleared", () => {
    Object.assign(c, { log: saved.log, round: saved.round, title: saved.title, updated: saved.updated });
    saved.people.forEach(([p, session, seen]) => { p.session = session; p.seen = seen; });
    save(); render(true);
  });
}

// Start new native sessions without deleting the visible conversation or
// replaying it into the new sessions. The next user message becomes their
// entire conversational context; repository instructions still apply.
function freshContext(c, targets) {
  if (busy(c)) { toast("Wait for the replies to finish before resetting context."); return; }
  if (!targets.length) { toast("Pick whose context to reset in “To:”."); return; }
  const saved = targets.map((p) => [p, p.session, p.seen, p.freshContext]);
  targets.forEach((p) => {
    p.session = "";
    p.seen = c.log.length;
    p.freshContext = true;
  });
  c.updated = Date.now(); save(); render();
  const names = targets.map((p) => p.name).join(", ");
  toast(`Fresh context prepared for ${names}. Past messages stay visible but won't be sent to the new session.`, () => {
    saved.forEach(([p, session, seen, fresh]) => {
      p.session = session; p.seen = seen;
      if (fresh) p.freshContext = true; else delete p.freshContext;
    });
    c.updated = Date.now(); save(); render();
  });
}
function switchTo(id) {
  hist.idx = -1;
  const old = cur();
  if (old && !discardEmptyConversation(old)) { drafts[old.id] = $("input").value; saveDraftsSoon(); }
  state.current = id;
  $("input").value = drafts[id] || ""; autosize();
  document.body.classList.remove("drawer");
  save(); render(true); $("input").focus();
  refreshReadiness(cur()).catch(() => {});
  refreshPlanBinding(cur()).catch(error => { if (cur()?.id === id) showPlanError(error.message); });
}
/* ---------- rename dialog ---------- */
const TITLE_HINT = 35; // roughly what fits in the sidebar next to the "more" button; guidance, not a limit
let renamingConv = null, titleCtl = null;

// Whoever answered last without failing is the default generator.
function lastResponder(c) {
  for (let i = c.log.length - 1; i >= 0; i--) {
    const e = c.log[i];
    if (e.from !== "you" && !e.error && !e.pending && e.text) return c.participants.find((p) => p.name === e.from) || null;
  }
  return c.participants[0] || null;
}
// A small, bounded cut of the conversation (about 1,500 characters) for the one-off title request.
function titleContext(c) {
  const clip = (t, n) => { const s = t.replace(/\s+/g, " ").trim(); return s.length > n ? s.slice(0, n) + "…" : s; };
  const users = c.log.filter((e) => e.from === "you" && e.text);
  const replies = c.log.filter((e) => e.from !== "you" && !e.error && !e.pending && e.text);
  const out = ["First user message: " + clip(users[0]?.text || "", 300), "", "Recent context:"];
  users.slice(1).slice(-3).forEach((e) => out.push("User: " + clip(e.text, 200)));
  replies.slice(-3).forEach((e) => out.push(`[${e.from}]: ${clip(e.text, 200)}`));
  return out.join("\n");
}
const cleanTitle = (t) => (t.split("\n").map((l) => l.trim()).find(Boolean) || "")
  .replace(/^[\s"'`*#]+|[\s"'`*]+$/g, "").replace(/[.。]+$/, "").trim();

// One standalone request to the agent: no session, no write access, nothing added to the conversation.
async function askTitle(c, p, extra, signal) {
  const prompt = `Write a short title for this conversation: at most ${TITLE_HINT} characters, in the same language as the messages, no quotes, no emoji, no trailing period. Reply with the title only.\n\n${titleContext(c)}${extra ? "\n\n" + extra : ""}`;
  const res = await fetch("/api/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      agent: p.agent, prompt, session_id: null, write: false, effort: null,
      system: "You write short titles for chat conversations. Reply with the title only.",
      model: kindOf(p) === "claude" ? "haiku" : null, // a cheap model; other agents use their configured default
    }),
    signal,
  });
  if (!res.ok) throw new Error(await res.text());
  const reader = res.body.getReader(), dec = new TextDecoder();
  let buf = "", text = "", final = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i); buf = buf.slice(i + 1);
      if (!line.trim()) continue;
      const ev = JSON.parse(line);
      if (ev.type === "delta") text += ev.text;
      else if (ev.type === "done") {
        if (!ev.ok) throw new Error(ev.error || "The agent failed.");
        final = ev.text;
      }
    }
  }
  return cleanTitle(text || final);
}

function renderRenameCount() {
  const n = $("rinput").value.trim().length, over = n > TITLE_HINT, count = $("rcount");
  count.replaceChildren(...(over ? [icon("alert")] : []), el("span", "", `${n} / ${TITLE_HINT}`));
  count.classList.toggle("over", over); // only an older, longer title can be over: typing stops at the limit
  count.title = over ? `Longer than the ${TITLE_HINT} characters that fit in the sidebar` : "";
}
const setGenLabel = (btn, label, spark = true) => btn.replaceChildren(...(spark ? [icon("spark")] : []), el("span", "", label));
// Cut at the last whole word that fits, or hard at the limit for one long word.
const fitTitle = (t) => {
  if (t.length <= TITLE_HINT) return t;
  const cut = t.slice(0, TITLE_HINT + 1), at = cut.lastIndexOf(" ");
  return (at > 0 ? cut.slice(0, at) : t.slice(0, TITLE_HINT)).trim();
};
function renameError(msg) {
  $("rerror").replaceChildren(icon("alert"), el("span", "", msg));
  $("rerror").hidden = false;
}
function openRename(c) {
  renamingConv = c;
  $("rinput").value = c.title;
  $("rerror").hidden = true;
  const who = $("rwho"); who.textContent = "";
  c.participants.forEach((p) => { const o = el("option", "", p.name); o.value = p.name; who.appendChild(o); });
  const def = lastResponder(c);
  if (def) who.value = def.name;
  const hasText = c.log.some((e) => e.text);
  $("rgen").disabled = !def || !hasText;
  $("rgen").title = hasText ? "" : "Send a message first";
  who.disabled = !def;
  setGenLabel($("rgen"), "Generate");
  renderRenameCount();
  $("rdialog").showModal();
  $("rinput").focus(); $("rinput").select();
}
$("rinput").oninput = renderRenameCount;
$("rcancel").onclick = () => $("rdialog").close();
$("rdialog").onclick = (event) => {
  if (event.target === event.currentTarget) $("rdialog").close();
};
$("rdialog").onclose = () => { titleCtl?.abort(); titleCtl = null; renamingConv = null; };
$("rgen").onclick = async () => {
  const c = renamingConv, p = c?.participants.find((q) => q.name === $("rwho").value);
  if (!c || !p || titleCtl) return;
  const ctl = titleCtl = new AbortController();
  const btn = $("rgen");
  btn.disabled = true; $("rwho").disabled = true; setGenLabel(btn, "Generating…", false); $("rerror").hidden = true; $("rinput").readOnly = true;
  try {
    let title = await askTitle(c, p, "", ctl.signal);
    // Too long to fit: ask once more for a shorter one; if it is still long, cut at a word.
    if (title.length > TITLE_HINT) title = await askTitle(c, p, `Your previous attempt, "${title}", was ${title.length} characters. Give a shorter title (at most ${TITLE_HINT}).`, ctl.signal);
    if (!title) throw new Error("The agent didn't suggest a title.");
    $("rinput").value = fitTitle(title); renderRenameCount();
    setGenLabel(btn, "Generate again");
    $("rinput").focus();
  } catch (err) {
    if (err.name === "AbortError") return;
    renameError("Couldn't generate a title: " + (err.message || err));
    setGenLabel(btn, "Generate");
  } finally {
    if (titleCtl === ctl) titleCtl = null;
    btn.disabled = false; $("rwho").disabled = false; $("rinput").readOnly = false;
  }
};
$("rform").onsubmit = (e) => {
  e.preventDefault();
  const c = renamingConv, t = $("rinput").value.trim();
  if (!c) return;
  if (!t) return renameError("The title can't be empty.");
  c.title = t.slice(0, 80); c.renamed = true; save(); renderSidebar(); setTitle();
  $("rdialog").close();
};

// Deleting a conversation asks first; the dialog's Cancel has the initial focus so a stray Enter keeps it.
let deleting = null;
function removeConv(c) {
  if (busy(c)) { toast("Stop its replies before deleting it."); return; }
  deleting = c;
  $("dtext").textContent = c.title;
  $("ddialog").showModal();
  $("dcancel").focus();
}
$("dcancel").onclick = () => $("ddialog").close();
$("ddialog").onclick = (event) => {
  if (event.target === event.currentTarget) $("ddialog").close();
};
$("ddialog").onclose = () => { deleting = null; };
$("ddelete").onclick = () => {
  const c = deleting;
  $("ddialog").close();
  if (!c) return;
  if (busy(c)) toast("Stop its replies before deleting it."); else deleteConv(c);
};
function deleteConv(c) {
  const idx = state.convs.indexOf(c);
  if (idx < 0) return;
  state.convs.splice(idx, 1);
  if (state.current === c.id) { state.current = state.convs[0]?.id || null; $("input").value = drafts[state.current] || ""; }
  save(); render(true);
  toast(`Deleted “${c.title}”`, () => { state.convs.splice(idx, 0, c); state.current = c.id; save(); render(true); });
}
// Drops one log entry, keeping everyone's read position on the same entries.
// Returns an undo.
function removeEntry(c, e) {
  const idx = c.log.indexOf(e);
  if (idx < 0) return () => {};
  const seens = new Map(c.participants.map((p) => [p, p.seen]));
  c.log.splice(idx, 1);
  c.participants.forEach((p) => { if (p.seen > idx) p.seen--; });
  return () => { c.log.splice(idx, 0, e); seens.forEach((s, p) => { p.seen = s; }); };
}

/* ---------- markdown (DOM only, never innerHTML on model text) ---------- */
const INLINE = /(`[^`\n]+`)|\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|\*\*([^*\n]+)\*\*|(?<![\w*])\*([^*\s][^*\n]*)\*(?![\w*])|(?<![\w])_([^_\s][^_\n]*)_(?![\w])/g;
function inline(node, text) {
  // matchAll walks its own copy of the regex, so the recursion below can't disturb it.
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    if (m.index > last) node.appendChild(document.createTextNode(text.slice(last, m.index)));
    if (m[1]) node.appendChild(el("code", "", m[1].slice(1, -1)));
    else if (m[2]) { const a = el("a"); a.href = m[3]; a.target = "_blank"; a.rel = "noopener noreferrer"; inline(a, m[2]); node.appendChild(a); }
    else if (m[4]) { const s = el("strong"); inline(s, m[4]); node.appendChild(s); }
    else { const s = el("em"); inline(s, m[5] || m[6]); node.appendChild(s); }
    last = m.index + m[0].length;
  }
  if (last < text.length) node.appendChild(document.createTextNode(text.slice(last)));
}
function codeBox(code, lang) {
  const box = el("div", "codebox"), head = el("div", "codehead"), pre = el("pre");
  head.appendChild(el("span", "", lang || "text"));
  const b = iconBtn("copy", "Copy code", () => copyText(code, b)); head.appendChild(b);
  pre.appendChild(el("code", "", code));
  box.append(head, pre);
  return box;
}
const FENCE = /^\s*```\s*([^\s`]*)/;
const ROW = /^\s*\|?.+\|.*$/;
const DELIM = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;
const LI = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
function cells(line) { return line.trim().replace(/^\||\|$/g, "").split("|").map((s) => s.trim()); }

function renderMd(root, text) {
  root.textContent = "";
  blocks(root, text.replace(/\r\n?/g, "\n").split("\n"));
}
function blocks(root, lines) {
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    let m;
    if ((m = line.match(FENCE))) {
      const body = []; i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) body.push(lines[i++]);
      i++; root.appendChild(codeBox(body.join("\n"), m[1])); continue;
    }
    if ((m = line.match(/^(#{1,4})\s+(.*)$/))) { const h = el("h" + m[1].length); inline(h, m[2]); root.appendChild(h); i++; continue; }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) { root.appendChild(el("hr")); i++; continue; }
    if (/^\s*>/.test(line)) {
      const q = []; while (i < lines.length && /^\s*>/.test(lines[i])) q.push(lines[i++].replace(/^\s*>\s?/, ""));
      const bq = el("blockquote"); blocks(bq, q); root.appendChild(bq); continue;
    }
    if (LI.test(line)) { i = list(root, lines, i); continue; }
    if (i + 1 < lines.length && line.includes("|") && DELIM.test(lines[i + 1])) {
      const head = cells(line); i += 2;
      const rows = []; while (i < lines.length && lines[i].trim() && ROW.test(lines[i])) rows.push(cells(lines[i++]));
      const t = el("table"), tr = el("tr");
      head.forEach((h) => { const th = el("th"); inline(th, h); tr.appendChild(th); });
      t.appendChild(el("thead")).appendChild(tr);
      const tb = el("tbody");
      rows.forEach((r) => { const row = el("tr"); head.forEach((_, k) => { const td = el("td"); inline(td, r[k] || ""); row.appendChild(td); }); tb.appendChild(row); });
      t.appendChild(tb); root.appendChild(t); continue;
    }
    const para = [];
    while (i < lines.length && lines[i].trim() && !/^\s*(```|#{1,4}\s|>)/.test(lines[i]) && !LI.test(lines[i])) para.push(lines[i++]);
    const p = el("p"); inline(p, para.join("\n")); root.appendChild(p);
  }
}
function list(root, lines, i) {
  const stack = []; // { indent, node }
  while (i < lines.length) {
    const m = lines[i].match(LI);
    if (!m) break;
    const indent = m[1].replace(/\t/g, "    ").length, ordered = /\d/.test(m[2]);
    while (stack.length && indent < stack[stack.length - 1].indent) stack.pop();
    let top = stack[stack.length - 1];
    if (!top || indent > top.indent) {
      const node = el(ordered ? "ol" : "ul");
      (top ? top.node.lastElementChild || top.node : root).appendChild(node);
      top = { indent, node }; stack.push(top);
    }
    const li = el("li"); inline(li, m[3]); top.node.appendChild(li);
    i++;
  }
  return i;
}

// While a reply streams in, it's split into chunks at blank lines outside
// code fences. Finished chunks keep their DOM (and any text selection); only
// the open last one is re-rendered, so each delta costs about one chunk.
// The final reply is rendered once more as a whole.
function chunksOf(text) {
  const out = []; let cur = [], fence = false;
  for (const line of text.replace(/\r\n?/g, "\n").split("\n")) {
    if (/^\s*```/.test(line)) fence = !fence;
    if (!fence && !line.trim()) { if (cur.length) { out.push(cur.join("\n")); cur = []; } continue; }
    cur.push(line);
  }
  if (cur.length) out.push(cur.join("\n"));
  return out;
}
function renderStreaming(root, text, cache) {
  const chunks = chunksOf(text);
  let keep = 0;
  while (keep < cache.chunks.length - 1 && keep < chunks.length - 1 && cache.chunks[keep] === chunks[keep]) keep++;
  for (let i = keep; i < cache.nodes.length; i++) cache.nodes[i].forEach((n) => n.remove());
  cache.chunks.length = keep; cache.nodes.length = keep;
  for (let i = keep; i < chunks.length; i++) {
    const frag = document.createDocumentFragment();
    blocks(frag, chunks[i].split("\n"));
    cache.nodes.push([...frag.childNodes]); cache.chunks.push(chunks[i]);
    root.appendChild(frag);
  }
}

/* ---------- skills ---------- */
// "/name args" at the start of a message. "/var/log is full" isn't one.
const skillOf = (text) => { const m = (text || "").match(/^\/([\w:.-]+)(?=\s|$)/); return m ? m[1] : ""; };
const knownSkill = (name) => skills.find((s) => s.name === name);
// Why a skill message can't go to `targets` as it is, or "".
function skillProblem(name, targets) {
  const s = knownSkill(name);
  if (!s) return "";
  if (targets.length !== 1) return `/${name} runs a skill: send it to exactly one participant (pick one in “To:” or start with @name).`;
  const p = targets[0];
  if (kindOf(p) === "agy") return `${p.name} runs agy, which can't run skills.`;
  if (s.requires_write && !p.write) return `/${name} writes files: give ${p.name} edit access first (click its name above).`;
  return "";
}

/* ---------- rendering: sidebar ---------- */
let openMenu = null;
function closeMenu() { if (openMenu) { openMenu.menu.remove(); openMenu.btn.setAttribute("aria-expanded", "false"); openMenu = null; } }
document.addEventListener("click", (e) => { if (openMenu && !openMenu.menu.contains(e.target) && e.target !== openMenu.btn) closeMenu(); });

function dateGroup(t) {
  const day = new Date(); day.setHours(0, 0, 0, 0);
  const d = day.getTime();
  if (t >= d) return "Today";
  if (t >= d - 864e5) return "Yesterday";
  if (t >= d - 7 * 864e5) return "Last 7 days";
  return "Older";
}
function renderSidebar() {
  const q = $("search").value.trim().toLowerCase();
  const list = $("list"); list.textContent = "";
  const shown = state.convs
    .filter((c) => !q || c.title.toLowerCase().includes(q) || c.log.some((e) => e.text && e.text.toLowerCase().includes(q)))
    .sort((a, b) => (b.updated || 0) - (a.updated || 0));
  if (!shown.length) list.appendChild(el("div", "none", q ? "No conversation matches." : "No conversations yet."));
  let group = null;
  shown.forEach((c) => {
    const g = dateGroup(c.updated || 0);
    if (g !== group) { list.appendChild(el("h3", "", g)); group = g; }
    list.appendChild(convRow(c));
  });
  const name = (meta.repo || "").split(/[\\/]/).filter(Boolean).pop() || meta.repo;
  $("reponame").textContent = name; $("reponame").title = name;
  $("repo").textContent = gitContext?.checkout || meta.repo; $("repo").title = gitContext?.checkout || meta.repo;
  $("contextbranch").textContent = gitContext?.label || "Git context unavailable";
  $("contextbar").dataset.checkout = gitContext?.checkout || meta.repo;
}
function convRow(c) {
  const row = el("div", "conv" + (c.id === state.current ? " active" : ""));
  const open = el("button", "open"); open.type = "button";
  open.appendChild(el("span", "title", c.title));
  const sub = el("span", "sub");
  c.participants.slice(0, 8).forEach((p) => { const d = applyParticipantColour(el("span", "dot"), hueOf(p)); d.title = p.name; sub.appendChild(d); });
  if (busy(c)) sub.appendChild(el("span", "working dots", "working"));
  open.appendChild(sub);
  if (c.id === state.current) open.setAttribute("aria-current", "true");
  open.onclick = () => { if (c.id !== state.current) switchTo(c.id); else document.body.classList.remove("drawer"); };
  row.appendChild(open);

  const more = iconBtn("more", "More actions for “" + c.title + "”", (e) => {
    e.stopPropagation();
    const wasOpen = openMenu && openMenu.btn === more;
    closeMenu();
    if (wasOpen) return;
    const menu = el("div", "menu"); menu.setAttribute("role", "menu");
    const item = (kind, label, fn, cls) => {
      const b = el("button", cls || ""); b.type = "button"; b.setAttribute("role", "menuitem");
      b.append(icon(kind), document.createTextNode(label));
      b.onclick = () => { closeMenu(); fn(); };
      menu.appendChild(b);
    };
    item("rename", "Rename", () => openRename(c));
    item("copy", "Clone agents", () => cloneAgents(c));
    item("retry", "Fork conversation", () => forkConversation(c));
    item("download", "Export as Markdown", () => exportConv(c, "md"));
    item("download", "Export as JSON", () => exportConv(c, "json"));
    item("trash", "Delete", () => removeConv(c), "danger");
    menu.onkeydown = (ev) => {
      const items = [...menu.querySelectorAll("button")], at = items.indexOf(document.activeElement);
      if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
        ev.preventDefault(); items[(at + (ev.key === "ArrowDown" ? 1 : items.length - 1)) % items.length].focus();
      } else if (ev.key === "Escape") { ev.stopPropagation(); closeMenu(); more.focus(); }
    };
    row.appendChild(menu);
    more.setAttribute("aria-expanded", "true");
    openMenu = { menu, btn: more };
    menu.querySelector("button").focus();
  });
  more.classList.add("more"); more.setAttribute("aria-haspopup", "menu"); more.setAttribute("aria-expanded", "false");
  row.appendChild(more);
  return row;
}

function exportConv(c, kind) {
  const text = ChatImport.exportText(c, kind);
  const type = kind === "json" ? "application/json" : "text/markdown";
  const a = el("a");
  a.href = URL.createObjectURL(new Blob([text], { type }));
  a.download = (c.title.replace(/[^\w-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 60) || "conversation") + "." + kind;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 2000);
}

/* ---------- file import ---------- */
function chooseImport() { $("importfile").click(); }
async function importFile(file) {
  if (!file) return;
  try {
    if (file.size > ChatImport.MAX_BYTES) throw new Error("The file is too large (maximum 16 MiB).");
    const extension = file.name.split(".").pop().toLowerCase();
    const result = ChatImport.parse(await file.text(), extension, meta.agents, MAX_PARTICIPANTS);
    const previous = state.current;
    const c = result.conversation
      ? { ...result.conversation, id: crypto.randomUUID(), updated: Date.now() }
      : { id: crypto.randomUUID(), title: file.name.replace(/\.md$/i, ""), participants: [], log: [], round: 0,
          updated: Date.now(), sendTo: null, keepEmpty: true };
    if (result.draft !== undefined) {
      const agent = defaultAgent();
      if (agent) addParticipant(c, { agent });
    }
    // Check the actual serialized snapshot against browser quota. Commit the
    // complete conversation only after both cache writes succeed.
    const next = { ...state, current: c.id, convs: [c, ...state.convs] };
    const nextDrafts = { ...drafts };
    const old = cur();
    if (old) nextDrafts[old.id] = $("input").value;
    if (result.draft !== undefined) nextDrafts[c.id] = result.draft;
    const oldCache = localStorage.getItem(KEY);
    const oldDrafts = localStorage.getItem(DRAFTS_KEY);
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
      localStorage.setItem(DRAFTS_KEY, JSON.stringify(nextDrafts));
    } catch (err) {
      try {
        if (oldCache === null) localStorage.removeItem(KEY); else localStorage.setItem(KEY, oldCache);
        if (oldDrafts === null) localStorage.removeItem(DRAFTS_KEY); else localStorage.setItem(DRAFTS_KEY, oldDrafts);
      } catch {}
      throw new Error("Browser storage is unavailable or full. Free space and try again.");
    }
    state = next; drafts = nextDrafts;
    $("search").value = "";
    $("input").value = drafts[c.id] || ""; autosize(); hist.idx = -1;
    document.body.classList.remove("drawer");
    saveRemoteSoon(); render(true); $("input").focus();
    toast(result.draft !== undefined ? "Markdown loaded as an unsent draft."
      : "Conversation imported. Agents start fresh sessions with this history as context. Writing is disabled.", () => {
      stopAll(c);
      state.convs = state.convs.filter((x) => x !== c); delete drafts[c.id];
      if (state.current === c.id) {
        state.current = state.convs.some((x) => x.id === previous) ? previous : state.convs[0]?.id || null;
        $("input").value = drafts[state.current] || ""; autosize();
      }
      save(); saveDrafts(); render(true);
    });
  } catch (err) { toast("Import failed: " + (err.message || err)); }
}

/* ---------- rendering: top bar and layout switch ---------- */
function renderBar() {
  const c = cur(), chips = $("chips"); chips.textContent = "";
  if (c) c.participants.forEach((p) => {
    const chip = el("button", "chip" + (p.inactive ? " off" : "")); chip.type = "button";
    const chipUsage = ChatQuotas.critical(quotaOf(p.agent, quotaModel(p.agent, p.model)))?.used_pct;
    if (ChatQuotas.numeric(chipUsage) && chipUsage > 80) chip.classList.add("high-usage");
    applyParticipantColour(chip, hueFor(c, p.name));
    chip.title = (p.role ? p.role + "\n\n" : "") + (p.inactive ? p.name + " — inactive. " : "") + "Edit " + p.name;
    if (p.inactive) chip.setAttribute("aria-label", p.name + ", inactive");
    const identity = el("span", "chipidentity");
    const nameLine = el("span", "chipnameline");
    nameLine.append(quotaStatusDot(quotaVisualStatus(p.agent, p.model, p.inactive), "agentstatus"), el("span", "chipname", p.name));
    identity.appendChild(nameLine);
    identity.appendChild(el("small", "", cap(p.agent) + (p.model ? " · " + cap(p.model) : "")));
    chip.appendChild(identity);
    if (p.inactive) { const ps = icon("pause"); ps.classList.add("pause"); chip.appendChild(ps); }
    if (p.write) chip.appendChild(writeBadge());
    const qb = quotaBadge(p.agent, quotaModel(p.agent, p.model));
    if (qb) { chip.appendChild(qb); chip.title += "\n" + quotaAlert(p.agent, quotaModel(p.agent, p.model)); }
    chip.onclick = () => openDialog(p);
    chips.appendChild(chip);
  });
  let notice = $("importnotice");
  if (!notice) { notice = el("div"); notice.id = "importnotice"; notice.setAttribute("role", "status"); $("bar").after(notice); }
  const unavailable = c ? c.participants.filter((p) => !meta.agents.includes(p.agent)) : [];
  notice.hidden = !unavailable.length;
  notice.textContent = unavailable.length ? "Unavailable agents: " + unavailable.map((p) => p.name).join(", ") + ". Edit their participant settings to choose an available agent and activate them." : "";
  const add = $("addbtn");
  add.hidden = !!c && c.participants.length >= MAX_PARTICIPANTS;
  add.disabled = !c || !meta.agents.length;

  const lay = $("layout"); lay.textContent = "";
  if (c && c.participants.length > 1) {
    const opt = (mode, kind, label) => {
      const b = iconBtn(kind, label + (state.layout === mode ? " (click again for automatic)" : ""), () => {
        state.layout = state.layout === mode ? "auto" : mode; save(); renderBar(); renderLog(true);
      });
      b.setAttribute("aria-pressed", String(state.layout === mode));
      lay.appendChild(b);
    };
    opt("columns", "columns", "Replies side by side");
    opt("stack", "stack", "Replies stacked");
  }
}

/* ---------- rendering: messages ---------- */
function stamp(e) {
  const t = new Date(e.t0 || Date.now()).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const time = e.dur != null ? `${t} · ${(e.dur / 1000).toFixed(1)}s` : t;
  return time + (e.task_id ? ' · ' + e.task_id : '');
}
function usageText(u) {
  if (!u) return ["", ""];
  const inTok = (u.input_tokens || 0) + (u.cache_read_tokens || 0) + (u.cache_write_tokens || 0);
  const parts = [];
  if (inTok || u.output_tokens) parts.push(`${fmtK(inTok)} in · ${fmtK(u.output_tokens || 0)} out`);
  if (u.cost_usd != null) parts.push("$" + u.cost_usd.toFixed(u.cost_usd < 1 ? 4 : 2));
  const detail = [
    inTok > 0 && `processed input: ${inTok}`,
    u.input_tokens != null && `uncached input: ${u.input_tokens}`,
    u.cache_read_tokens != null && `cache read: ${u.cache_read_tokens}`,
    u.cache_write_tokens != null && `cache write: ${u.cache_write_tokens}`,
    u.output_tokens != null && `output: ${u.output_tokens}`,
    u.num_turns != null && `agent turns: ${u.num_turns}`,
    inTok > 0 && "Input accumulates across internal model and tool calls.",
  ].filter(Boolean).join("\n");
  return [parts.join(" · "), detail];
}
const lastEntryOf = (c, name) => { for (let i = c.log.length - 1; i >= 0; i--) if (c.log[i].from === name) return c.log[i]; return null; };
const lastUserEntry = (c) => lastEntryOf(c, "you");

// One reply per round can be expanded to fill the row; the others fold into name strips. Kept in memory only.
const expanded = new Map(); // "conversation:round" -> participant name
const expKey = (c, round) => c.id + ":" + round;
const isExpanded = (c, e) => expanded.get(expKey(c, e.round)) === e.from;
function applyFocus(c, r) {
  const who = expanded.get(expKey(c, r.round)), on = !!who && r.cards.some((x) => x.name === who);
  r.el.classList.toggle("focus", on);
  r.cards.forEach((x) => {
    x.node.classList.toggle("expanded", on && x.name === who);
    x.node.classList.toggle("folded", on && x.name !== who);
    paintCard(c, x.entry); // the expand button reflects the state
  });
}
function toggleExpand(c, e) {
  const key = expKey(c, e.round), open = !isExpanded(c, e);
  if (open) expanded.set(key, e.from); else expanded.delete(key);
  view.rows.forEach((r) => { if (r.round === e.round) applyFocus(c, r); });
}
// Esc: fold every expanded reply of this conversation back. True if there was one.
function collapseAll(c) {
  const keys = [...expanded.keys()].filter((k) => k.startsWith(c.id + ":"));
  keys.forEach((k) => expanded.delete(k));
  if (keys.length) view.rows.forEach((r) => applyFocus(c, r));
  return keys.length > 0;
}
function card(c, e) {
  const d = el("article", "card");
  applyParticipantColour(d, hueFor(c, e.from));
  const head = el("header"), status = el("div", "status"), body = el("div", "body md"), acts = el("div"), foot = el("footer");
  d.append(head, status, body, acts, foot);
  d.onclick = () => { if (d.classList.contains("folded")) toggleExpand(c, e); };
  cardParts.set(e, { card: d, head, status, body, acts, foot, mode: "", rendered: null, stream: null, keys: {} });
  paintCard(c, e);
  return d;
}

// Repaints one card from its entry; used for streaming without rebuilding the
// page. Each part is rebuilt only when what it shows changed.
function paintCard(c, e) {
  const k = cardParts.get(e); if (!k) return;
  const p = c.participants.find((x) => x.name === e.from);
  const turnAgent = e.agent || p?.agent, turnModel = e.agent ? e.model : p?.model;
  const quotaStatus = quotaVisualStatus(turnAgent, turnModel, p?.inactive), quotaPercent = quotaUsagePercent(turnAgent, turnModel);
  const changed = (part, key) => { const s = JSON.stringify(key); if (k.keys[part] === s) return false; k.keys[part] = s; return true; };
  k.card.classList.toggle("error", !!e.error && !e.soft);
  k.card.classList.toggle("soft", !!e.error && !!e.soft);

  if (changed("head", [turnAgent, turnModel, p?.write, e.skill, e.pending, hueFor(c, e.from), isExpanded(c, e), quotaStatus.kind, quotaPercent])) {
    applyParticipantColour(k.card, hueFor(c, e.from));
    k.head.textContent = "";
    const identity = el("span", "cardidentity");
    identity.appendChild(quotaStatusDot(quotaStatus, "carddot"));
    identity.appendChild(el("span", "name", e.from));
    if (turnAgent) identity.appendChild(el("small", "cardmeta", cap(turnAgent) + (quotaPercent ? " · " + quotaPercent : "")));
    k.head.appendChild(identity);
    const context = el("span", "cardcontext");
    if (p?.write) context.appendChild(writeBadge());
    if (e.skill) { const s = el("span", "badge skill", "/" + e.skill); s.title = "ran the skill /" + e.skill; context.appendChild(s); }
    if (context.childElementCount) k.head.appendChild(context);
    const tools = el("span", "cardtools");
    if (e.pending) tools.appendChild(iconBtn("stop", "Stop " + e.from, () => stopEntry(c, e)));
    const on = isExpanded(c, e), ex = iconBtn(on ? "collapse" : "expand", on ? "Collapse reply" : "Expand reply", () => toggleExpand(c, e));
    ex.classList.add("expandbtn"); ex.setAttribute("aria-expanded", String(on));
    tools.appendChild(ex); k.head.appendChild(tools);
  }

  if (changed("status", [e.pending, e.activity || "", !!e.text])) {
    k.status.textContent = "";
    if (e.pending) {
      const act = el("span", "act dots", e.activity || (e.text ? "Writing" : "Thinking"));
      k.status.appendChild(act);
    }
    k.status.hidden = !e.pending;
  }

  if (e.error) {
    if (k.mode !== "error" || k.rendered !== e.text) {
      k.body.textContent = ""; k.mode = "error"; k.rendered = e.text;
      const long = e.text.length > 220 || e.text.includes("\n");
      if (long) {
        const det = el("details"); det.appendChild(el("summary", "", e.text.split("\n")[0].slice(0, 140)));
        det.appendChild(el("div", "err", e.text)); k.body.appendChild(det);
      } else k.body.appendChild(el("div", "err", e.text));
    }
  } else if (e.pending) {
    if (k.mode !== "stream") { k.body.textContent = ""; k.mode = "stream"; k.stream = { chunks: [], nodes: [] }; }
    renderStreaming(k.body, e.text, k.stream);
  } else if (k.mode !== "final" || k.rendered !== e.text) {
    renderMd(k.body, e.text); k.mode = "final"; k.rendered = e.text;
  }

  const actions = e.actions || [];
  if (changed("acts", [actions.length])) {
    const wasOpen = !!k.acts.querySelector("details[open]");
    k.acts.textContent = "";
    if (actions.length) {
      const det = el("details", "actions"); det.open = wasOpen;
      const counts = new Map();
      actions.forEach((a) => { const tool = a.split(":")[0]; counts.set(tool, (counts.get(tool) || 0) + 1); });
      const top = [...counts].sort((a, b) => b[1] - a[1]).slice(0, 4).map(([t, n]) => `${t} ${n}`).join(", ");
      det.appendChild(el("summary", "", `${actions.length} action${actions.length > 1 ? "s" : ""}: ${top}${counts.size > 4 ? ", …" : ""}`));
      const ol = el("ol");
      actions.forEach((a) => { const li = el("li", "", a); li.title = a; ol.appendChild(li); });
      det.appendChild(ol);
      k.acts.appendChild(det);
    }
  }

  const canRegen = !e.pending && !!p && !busy(c) && lastEntryOf(c, e.from) === e && (e.seen0 != null || e.error);
  const [usage, detail] = usageText(e.usage);
  if (changed("foot", [e.pending, e.dur, usage, canRegen, !!e.text, e.stopped, busy(c)])) {
    k.foot.textContent = "";
    if (e.pending) {
      // While it thinks or writes, the footer shows how long it has been at it, where the clock time goes afterwards.
      const t = el("span", "", Math.round((Date.now() - e.t0) / 1000) + "s"); t.dataset.tick = e.t0;
      k.foot.appendChild(el("span", "metrics")).appendChild(t);
    } else {
      const m = el("span", "metrics", stamp(e) + (usage ? " · " + usage : "") + (e.stopped ? " · stopped" : ""));
      if (detail) m.title = detail;
      const tools = el("span", "hovertools");
      if (e.text && !e.error) tools.appendChild(iconBtn("copy", "Copy reply", function () { copyText(e.text, this); }));
      if (canRegen) tools.appendChild(iconBtn("retry", e.error ? "Try again" : "Regenerate", () => regenerate(c, e, p)));
      if (!busy(c)) tools.appendChild(iconBtn("trash", "Delete reply", () => deleteEntry(c, e)));
      k.foot.append(m, tools);
    }
  }
}

// Elapsed time on pending cards.
setInterval(() => {
  document.querySelectorAll("[data-tick]").forEach((n) => { n.textContent = Math.round((Date.now() - Number(n.dataset.tick)) / 1000) + "s"; });
}, 1000);

function pastedTextBox(paste) {
  const box = el("div", "pastedtext");
  const label = el("div", "pastedtextlabel", `[Pasted Text · +${paste.lines} lines]`);
  const lines = paste.text.split("\n");
  const preview = el("pre", "pastedtextpreview", lines.shift() || "");
  const content = el("pre", "pastedtextcontent", lines.join("\n")); content.hidden = true;
  const toggle = el("button", "pastedtexttoggle", "Expand"); toggle.type = "button";
  toggle.setAttribute("aria-expanded", "false");
  toggle.hidden = !lines.length;
  toggle.onclick = () => {
    const expanded = content.hidden;
    content.hidden = !expanded;
    toggle.textContent = expanded ? "Collapse" : "Expand";
    toggle.setAttribute("aria-expanded", String(expanded));
  };
  box.append(label, preview, content, toggle);
  return box;
}
function renderUserMessage(node, e) {
  const pastes = Array.isArray(e.pasted) ? e.pasted
    .filter(paste => Number.isSafeInteger(paste.start) && Number.isSafeInteger(paste.end)
      && Number.isSafeInteger(paste.lines) && paste.lines > 0
      && paste.start >= 0 && paste.end >= paste.start && paste.end <= e.text.length)
    .sort((a, b) => a.start - b.start)
    .map(paste => ({ ...paste, text: e.text.slice(paste.start, paste.end) })) : [];
  if (!pastes.length) {
    const sk = skillOf(e.text);
    if (sk) {
      const badge = el("span", "badge skill", "/" + sk); badge.title = knownSkill(sk)?.description || "skill";
      node.append(badge, document.createTextNode(e.text.slice(sk.length + 1)));
    } else node.textContent = e.text;
    return;
  }
  let offset = 0;
  for (const paste of pastes) {
    if (paste.start < offset) continue;
    if (paste.start > offset) node.appendChild(document.createTextNode(e.text.slice(offset, paste.start)));
    node.appendChild(pastedTextBox(paste));
    offset = paste.end;
  }
  if (offset < e.text.length) node.appendChild(document.createTextNode(e.text.slice(offset)));
}
function userBubble(c, e) {
  const u = el("div", "user");
  const fill = () => {
    u.textContent = "";
    if (e.to) u.appendChild(el("span", "to", "TO · " + e.to.join(", ")));
    const text = el("div", "text");
    renderUserMessage(text, e);
    u.appendChild(text);
    if (e.edited) u.appendChild(el("span", "edited", "edited"));
    const tools = el("span", "hovertools tools");
    tools.appendChild(iconBtn("copy", "Copy message", function () { copyText(e.text, this); }));
    if (!busy(c) && lastUserEntry(c) === e) tools.appendChild(iconBtn("edit", "Edit and resend", () => editUser(c, e, u)));
    tools.appendChild(iconBtn("trash", "Delete message", () => deleteEntry(c, e)));
    u.appendChild(tools);
  };
  fill();
  u._fill = fill;
  return u;
}
function editUser(c, e, u) {
  u.textContent = "";
  const ta = el("textarea"); ta.value = e.text; ta.setAttribute("aria-label", "Edit message");
  const bar = el("div", "editbar");
  const cancel = el("button", "", "Cancel"); cancel.type = "button"; cancel.onclick = () => u._fill();
  const ok = el("button", "primary", "Save & resend"); ok.type = "button";
  ok.onclick = () => { const t = ta.value.trim(); if (t) resendEdited(c, e, t); };
  ta.onkeydown = (ev) => {
    if (ev.key === "Enter" && !ev.shiftKey && !ev.isComposing) { ev.preventDefault(); ok.click(); }
    else if (ev.key === "Escape") { ev.stopPropagation(); u._fill(); }
  };
  bar.append(cancel, ok); u.append(ta, bar);
  ta.focus(); ta.selectionStart = ta.selectionEnd = ta.value.length;
}
// Drops the message and the replies after it, rewinds everyone who saw it,
// and sends the new text to the same people.
function resendEdited(c, e, text) {
  if (busy(c)) return;
  const idx = c.log.indexOf(e);
  if (idx < 0) return;
  const later = c.log.slice(idx + 1);
  c.participants.forEach((p) => {
    const own = later.find((x) => x.from === p.name && x.seen0 != null);
    if (own) p.seen = Math.min(p.seen, own.seen0);
    p.seen = Math.min(p.seen, idx);
  });
  c.log.length = idx;
  const targets = e.to ? c.participants.filter((p) => e.to.includes(p.name)) : c.participants;
  const problem = skillOf(text) && skillProblem(skillOf(text), targets);
  if (problem) { toast(problem); renderLog(true); return; }
  launch(c, targets, text, e.to || null, { edited: true });
}
function deleteEntry(c, e) {
  if (busy(c)) { toast("Wait for the replies to finish."); return; }
  const undo = removeEntry(c, e);
  save(); renderLog(true); renderComposer();
  toast(e.from === "you" ? "Message deleted" : `Reply from ${e.from} deleted`, () => { undo(); save(); renderLog(true); renderComposer(); });
}

// The log is appended to, not rebuilt: rebuilding re-announces everything to
// screen readers and loses the reader's selection. It's rebuilt only when
// something earlier changed (switch, delete, edit, participants, layout).
const view = { id: null, sig: "", entries: [], wrap: null, after: null, rows: new Map(), lastUser: null };
const logSig = (c) => JSON.stringify([c.participants.map((p) => [p.name, p.agent, p.model, p.write, hueOf(p)]), state.layout]);
function renderLog(force) {
  const c = cur(), msgs = $("msgs");
  msgs.classList.toggle("stack", state.layout === "stack");
  msgs.classList.toggle("cols", state.layout === "columns");
  if (!c || !c.log.length) { view.id = null; msgs.replaceChildren(emptyState(c)); return; }
  const prefix = view.id === c.id && view.sig === logSig(c) && view.entries.length <= c.log.length && view.entries.every((e, i) => c.log[i] === e);
  if (force || !prefix) {
    view.id = c.id; view.sig = logSig(c); view.entries = []; view.rows = new Map(); view.lastUser = null;
    view.wrap = el("div", "wrap"); view.after = el("div", "after");
    view.wrap.appendChild(view.after);
    msgs.replaceChildren(view.wrap);
  }
  c.log.slice(view.entries.length).forEach((e) => appendEntry(c, e));
  renderAfter(c);
}
function appendEntry(c, e) {
  view.entries.push(e);
  if (e.from === "you") {
    const prev = view.lastUser;
    const u = userBubble(c, e);
    view.wrap.insertBefore(u, view.after);
    view.lastUser = u;
    if (prev) prev._fill(); // only the newest message can be edited
    return;
  }
  let r = view.rows.get(e.round);
  if (!r || r.cards.some((x) => x.name === e.from)) r = newRow(e.round);
  const node = card(c, e);
  // Replies sit side by side in participant order; whoever stayed silent takes no room,
  // so a regenerated reply lands back in its own place.
  const order = (name) => { const i = c.participants.findIndex((p) => p.name === name); return i < 0 ? Infinity : i; };
  const mine = order(e.from), next = r.cards.find((x) => order(x.name) > mine);
  r.el.insertBefore(node, next ? next.node : null);
  r.cards.splice(next ? r.cards.indexOf(next) : r.cards.length, 0, { name: e.from, node, entry: e });
  r.el.style.setProperty("--cols", r.cards.length);
  // Columns only pay off once two replies sit side by side; a lone reply gets the full reading width.
  r.el.classList.toggle("solo", r.cards.length < 2);
  r.el.classList.toggle("wide", r.cards.length >= 3);
  applyFocus(c, r);
}
function newRow(round) {
  const r = { round, cards: [], el: el("div", "row solo") };
  view.wrap.insertBefore(r.el, view.after);
  view.rows.set(round, r);
  return r;
}
// "Ask them to comment" under the last round, when there's something to comment on.
function renderAfter(c) {
  if (!view.after) return;
  view.after.textContent = "";
  if (!c || c.participants.length < 2 || busy(c) || !c.log.length || c.log[c.log.length - 1].from === "you") return;
  const ready = recipients(c).filter((p) => unseen(c, p).length);
  if (!ready.length) return;
  const label = ready.length === 1
    ? `Ask ${ready[0].name} to comment on the other replies`
    : ready.length === c.participants.length ? "Ask them to comment on each other's replies" : `Ask ${ready.map((p) => p.name).join(", ")} to comment on the other replies`;
  const b = iconBtn("comment", label, () => launch(c, ready), label);
  b.className = ""; b.type = "button";
  view.after.appendChild(b);
}

const SUGGESTIONS = [
  "Give me a quick tour of this repository.",
  "Review the uncommitted changes and point out bugs.",
  "What are the riskiest parts of this codebase?",
];
function emptyState(c) {
  const d = el("div"); d.id = "empty";
  const importButton = el("button", "emptyaction importaction");
  importButton.type = "button"; importButton.onclick = chooseImport;
  importButton.append(icon("download"));
  const importCopy = el("span", "emptyactioncopy");
  importCopy.append(el("strong", "", "Import conversation"), el("small", "", "Continue from an aic .json or .md file"));
  importButton.appendChild(importCopy);

  const hero = el("header", "emptyhero");
  const mark = el("span", "emptymark"); mark.appendChild(icon("spark"));
  hero.appendChild(mark);

  if (!meta.agents.length) {
    hero.append(el("span", "emptyeyebrow", "New conversation"), el("h1", "", "No agents configured"),
      el("p", "", "Add an agent with aic config, then reload this page — or import an existing conversation."));
    const actions = el("div", "emptyactions"); actions.appendChild(importButton);
    d.append(hero, actions);
    return d;
  }
  if (!c) {
    hero.append(el("span", "emptyeyebrow", "Conversation"), el("h1", "", "Nothing open"),
      el("p", "", "Choose a conversation from the sidebar or import one from a file."));
    const actions = el("div", "emptyactions"); actions.appendChild(importButton);
    d.append(hero, actions);
    return d;
  }

  const group = c.participants.length > 1;
  hero.append(el("span", "emptyeyebrow", group ? `${c.participants.length} participants ready` : "New conversation"),
    el("h1", "", group ? "Bring the right perspectives together" : "What do you want to work on?"),
    el("p", "", group
      ? "Send one prompt to the selected participants, compare their replies, then let them review each other."
      : c.participants.length
        ? `Start with a question for ${c.participants[0].name}, or add another participant for a second perspective.`
        : "Choose an agent for this conversation, or bring in a conversation you already started."));
  d.appendChild(hero);

  const add = el("button", "emptyaction primaryaction"); add.type = "button";
  add.appendChild(icon("plus"));
  const addCopy = el("span", "emptyactioncopy");
  addCopy.append(el("strong", "", "Add participant"), el("small", "", c.participants.length ? "Add another point of view" : "Choose an agent, model and access"));
  add.appendChild(addCopy);
  add.onclick = () => openDialog(null);

  const actions = el("div", "emptyactions"); actions.append(add, importButton);
  d.appendChild(actions);

  if (c.participants.length) {
    const participantSection = el("section", "emptyparticipants");
    const participantHeading = el("div", "emptysectiontitle");
    participantHeading.append(el("span", "", "In this conversation"), el("small", "", "Select a participant to edit"));
    const people = el("div", "people");
    c.participants.forEach((p) => {
      const b = el("button", "person"); b.type = "button"; b.title = "Edit " + p.name;
      applyParticipantColour(b, hueOf(p));
      const identity = el("span", "personidentity");
      identity.append(avatar(c, p.name, true), el("span", "name", p.name));
      b.appendChild(identity);
      const tags = el("span", "persontags"); tags.appendChild(el("span", "badge", cap(p.agent) + (p.model ? " · " + cap(p.model) : "")));
      if (p.write) tags.appendChild(writeBadge());
      b.appendChild(tags);
      if (p.role) b.appendChild(el("span", "role", p.role));
      b.onclick = () => openDialog(p);
      people.appendChild(b);
    });
    participantSection.append(participantHeading, people);
    d.appendChild(participantSection);
  }

  const promptSection = el("section", "emptyprompts");
  promptSection.appendChild(el("div", "emptysectiontitle", "Try a starting point"));
  const sug = el("div", "suggest");
  SUGGESTIONS.forEach((s) => {
    const b = el("button"); b.type = "button";
    b.append(icon("send"), el("span", "", s));
    b.onclick = () => { const i = $("input"); i.value = s; autosize(); i.focus(); onInput(); };
    sug.appendChild(b);
  });
  promptSection.appendChild(sug); d.appendChild(promptSection);
  return d;
}

/* ---------- rendering: composer ---------- */
// Leading "@a @b" in the message, or null.
function leadingMentions(text) {
  const m = (text + " ").match(/^((?:@[\w-]+\s+)+)/);
  return m ? m[1].trim().split(/\s+/).map((n) => n.slice(1)) : null;
}
// A disabled participant stays in the conversation (and its history) but gets nothing new.
const active = (c) => c.participants.filter((p) => !p.inactive && meta.agents.includes(p.agent));
// "@all" stands for every participant, unless someone is really named "all".
const expandMentions = (c, names) => names.flatMap((n) =>
  n === "all" && !c.participants.some((p) => p.name === n) ? active(c).map((p) => p.name) : [n]);
// Who a message typed now would go to: leading @mentions win, else the "To:" pills.
function recipients(c, text) {
  const typed = leadingMentions(text ?? $("input").value);
  const named = typed && expandMentions(c, typed).filter((n) => c.participants.some((p) => p.name === n));
  const names = named && named.length ? named : c.sendTo;
  return names ? active(c).filter((p) => names.includes(p.name)) : active(c);
}
function renderComposer() {
  const c = cur(), to = $("to"); to.textContent = "";
  if (c) {
    const sel = recipients(c);
    active(c).forEach((p) => {
      const on = sel.includes(p);
      const b = el("button", "pill"); b.type = "button";
      applyParticipantColour(b, hueOf(p));
      b.setAttribute("aria-pressed", String(on));
      b.appendChild(el("span", "dot"));
      b.appendChild(el("span", "", p.name));
      const pm = quotaModel(p.agent, p.model), alert = quotaAlert(p.agent, pm);
      if (alert) { const badge = icon("alert"); badge.classList.add("quota-" + ChatQuotas.level(quotaOf(p.agent, pm))); b.appendChild(badge); b.setAttribute("aria-label", p.name + ": " + alert); }
      const n = unseen(c, p).length;
      if (n && c.log.length) { const cnt = el("span", "count", String(n)); b.appendChild(cnt); }
      b.title = (on ? "Receives your message" : "Doesn't receive your message") + (n ? ` · ${n} new message${n > 1 ? "s" : ""} to read` : "");
      if (alert) b.title += "\n" + alert;
      if (p.freshContext) b.title += "\nFresh context starts with the next message.";
      b.onclick = () => togglePill(c, p);
      to.appendChild(b);
    });
  }
  const send = $("send"), stop = busy(c);
  setIcon(send, stop ? "stop" : "send");
  send.classList.toggle("stop", stop);
  send.setAttribute("aria-label", stop ? "Stop replies (Esc)" : "Send");
  send.title = stop ? "Stop replies (Esc)" : "Send (Enter)";
  send.disabled = !c || !c.participants.length;
  renderHint();
}
function renderHint() {
  const c = cur(), hint = $("hint"), text = $("input").value.trim();
  let msg = "Enter sends · Shift+Enter new line · @ mentions · /fork · /fresh · /clear" + (skills.length ? " · / skills" : ""), warn = false;
  if (c && busy(c)) msg = "Replies are coming in. Esc or ■ stops them; you can keep typing.";
  else if (c) {
    const body = text.replace(/^((?:@[\w-]+\s+)+)/, "");
    const sk = skillOf(body), problem = sk && skillProblem(sk, recipients(c));
    if (problem) { msg = problem; warn = true; }
    else if (c.participants.length && !recipients(c).length) { msg = "Pick who receives the message in “To:”."; warn = true; }
  }
  hint.textContent = msg; hint.classList.toggle("warn", warn);
}
function togglePill(c, p) {
  const all = active(c).map((q) => q.name);
  const sel = new Set(recipients(c).map((q) => q.name));
  if (sel.has(p.name)) sel.delete(p.name); else sel.add(p.name);
  c.sendTo = sel.size === all.length ? null : all.filter((n) => sel.has(n));
  // Keep leading @mentions in the text in step with the pills.
  const i = $("input"), m = (i.value + " ").match(/^((?:@[\w-]+\s+)+)/);
  if (m) {
    const rest = (i.value + " ").slice(m[1].length).replace(/ $/, "");
    i.value = (c.sendTo ? c.sendTo.map((n) => "@" + n + " ").join("") : "") + rest;
    autosize(); drafts[c.id] = i.value; saveDraftsSoon();
  }
  c.toFromText = false;
  saveSoon(); renderComposer(); renderAfter(c);
}
// Typing "@name" picks the pills; deleting the mention goes back to everyone.
function syncPillsFromText(c) {
  const typed = leadingMentions($("input").value);
  const names = typed && expandMentions(c, typed).filter((n) => active(c).some((p) => p.name === n));
  if (names && names.length) {
    const picked = uniq(names);
    c.sendTo = picked.length === active(c).length ? null : picked; // naming everyone is the same as no filter
    c.toFromText = true;
  }
  else if (c.toFromText) { c.sendTo = null; c.toFromText = false; }
}

function setTitle() { const c = cur(); document.title = c ? `${c.title} · aic chat` : "aic chat"; }
function render(scrollEnd) {
  renderSidebar(); renderBar(); renderLog(); renderComposer(); setTitle();
  renderQuotas(); renderEnvironmentStatus(); renderPlanContext();
  if (scrollEnd) toEnd();
}

/* ---------- scrolling that doesn't fight the reader ---------- */
const nearEnd = () => { const m = $("msgs"); return m.scrollHeight - m.scrollTop - m.clientHeight < 80; };
const toEnd = () => { const m = $("msgs"); m.scrollTop = m.scrollHeight; $("jump").classList.remove("show"); };
// The message list's scrollbar takes room the composer doesn't have; tell the CSS how much, so the two stay aligned.
if (window.ResizeObserver) {
  new ResizeObserver(() => {
    const m = $("msgs");
    document.documentElement.style.setProperty("--sb", (m.offsetWidth - m.clientWidth) + "px");
  }).observe($("msgs"));
  // How far down the footer the "To:" row starts: the footer's fade is solid from there. The input grows with its text.
  new ResizeObserver(() => {
    const bar = $("composer").querySelector(".toolbar");
    if (bar) document.documentElement.style.setProperty("--solid", Math.round(bar.getBoundingClientRect().top - $("composer").getBoundingClientRect().top) + "px");
  }).observe($("composer"));
}
$("msgs").addEventListener("scroll", () => { if (nearEnd()) $("jump").classList.remove("show"); });
$("jump").onclick = toEnd;
$("jump").append(icon("down"), document.createTextNode("New messages"));

/* ---------- turns ---------- */
// A user message addressed with @name is only for the named participants.
// `until` limits it to the log before that index.
const unseen = (c, p, until = c.log.length) => c.log.slice(p.seen, until).filter((e) =>
  e.from !== p.name && !e.error && e.text && !(e.to && !e.to.includes(p.name)));

function promptFor(c, p, opts = {}) {
  const msgs = unseen(c, p, opts.until);
  const group = c.participants.length > 1;
  const fmt = (e) => {
    const text = e.edited ? "(edited: this replaces my previous message; disregard that one and any reply to it)\n" + e.text : e.text;
    return group ? `[${e.from === "you" ? "user" : e.from}]: ${text}` : text;
  };
  const regen = opts.regen ? "(The user asked for another answer to this: answer it again from scratch rather than repeating your previous answer.)" : "";
  // A slash command only works at the very start of the prompt: lead with
  // the latest one, and pass whatever else is new as context after it.
  const last = [...msgs].reverse().find((e) => e.from === "you");
  const skill = last && kindOf(p) !== "agy" ? skillOf(last.text) : "";
  if (skill) {
    const rest = msgs.filter((e) => e !== last);
    const text = last.text
      + (rest.length ? "\n\n---\nOther messages since your last turn, for context:\n\n" + rest.map(fmt).join("\n\n") : "")
      + (regen ? "\n\n" + regen : "");
    return { text, skill };
  }
  return { text: (regen ? regen + "\n\n" : "") + msgs.map(fmt).join("\n\n"), skill: "" };
}
// A session belongs to the underlying CLI, not to the participant displayed in
// the page. When its agent changes, the old session cannot be resumed (for
// example, a Claude session is not meaningful to agy). Give the replacement
// agent everything this participant could already see before its next message.
function historyForNewSession(c, p, until) {
  return c.log.slice(0, until).filter((e) =>
    !e.pending && !e.error && e.text && !(e.to && !e.to.includes(p.name))
  ).map((e) => {
    const who = e.from === "you" ? "user" : e.from;
    const text = e.edited
      ? "(edited: this replaces my previous message; disregard that one and any reply to it)\n" + e.text
      : e.text;
    return `[${who}]: ${text}`;
  }).join("\n\n");
}

function bootstrapPrompt(c, job) {
  // The current prompt already contains every message for a brand-new
  // participant (whose `seen` is zero). A changed agent, however, has a
  // participant with prior messages marked as seen but no resumable session.
  const history = historyForNewSession(c, job.p, job.oldSeen);
  if (!history) return job.text;
  const context = "---\nConversation before this agent session. Use it as context; do not answer it separately.\n\n"
    + history + "\n\n---\n";
  // Claude skills only work when the slash command starts the prompt.
  return job.skill
    ? job.text + "\n\n" + context
    : context + "New messages to answer:\n\n" + job.text;
}
function systemFor(c, p) {
  const others = active(c).filter((q) => q !== p).map((q) => q.name);
  let s = others.length
    ? `You are "${p.name}" in a group chat with the user and: ${others.join(", ")}. Messages from others arrive prefixed with [name]. Reply only as yourself, without prefixing your own name.`
    : `You are "${p.name}".`;
  if (p.role) s += ` Your role: ${p.role}`;
  if (c.planBinding?.plan_id) {
    const plan = planFor(c);
    s += `\nCurrent shared plan: ${plan?.title || c.planBinding.plan_id}.`;
    if (c.planOrigin) s += '\nMessages inherited before this fork belong to the previous plan. Use them as historical context; the current plan governs new work.';
    s += '\nThis is a discussion turn. Discuss or review without editing files or updating plan records.';
  }
  if (kindOf(p) !== "agy" && skills.length) {
    s += " A user message that starts with /name (for example /aic-plan my-plan) asks you to run the skill `name` with the rest of that line as its arguments."
      + " If a skill asks you to interview the user, ask your questions as plain replies in this chat; there is no question tool here.";
  }
  if (kindOf(p) === "codex") {
    s += " Keep tool context compact: return names and selected fields instead of complete tool definitions, cap broad search output, and summarize large findings before continuing."
      + " Read each file or line range once unless it changed; after truncated output, continue at the first unread line without overlap.";
  }
  return s;
}

// Starts one turn for each target, concurrently. Prompts are built before any
// placeholder is logged so replies from this same round stay unseen for the next one.
async function launch(c, targets, userText, to, opts = {}) {
  if (!opts.preflightDone && !(await prepareSend(c, targets))) return;
  if (busy(c)) return;
  targets = targets.filter((p) => !p.inactive && meta.agents.includes(p.agent));
  if (!targets.length) { toast("Choose an available, active participant first."); return; }
  // A regenerated reply keeps its round, so it fills its own slot in that round's row.
  const round = opts.round ?? (c.round = (c.round || 0) + 1);
  c.updated = Date.now();
  if (userText) {
    const first = !c.log.some((e) => e.from === "you");
    c.log.push({ from: "you", round, text: userText, t0: Date.now(), ...(to ? { to } : {}),
      ...(opts.edited ? { edited: true } : {}), ...(opts.pasted?.length ? { pasted: opts.pasted } : {}) });
    if (first && !c.renamed) c.title = userText.slice(0, 48);
  }
  const jobs = targets
    .map((p) => ({ p, ...promptFor(c, p, opts), oldSeen: p.seen, freshContext: !!p.freshContext, planRun: planRunFor(c, p) }))
    .filter((j) => j.text.trim() && unseen(c, j.p, opts.until).length);
  jobs.forEach((j) => {
    if (!j.p.session && !j.freshContext) j.text = bootstrapPrompt(c, j);
  });
  jobs.forEach((j) => { j.p.seen = opts.until ?? c.log.length; });
  jobs.forEach((j) => {
    const runId = globalThis.crypto?.randomUUID?.() || (Date.now().toString(36) + "-" + Math.random().toString(36).slice(2));
    j.entry = { from: j.p.name, agent: j.p.agent, model: j.p.model || meta.details?.[j.p.agent]?.model || "", round, text: "", pending: true, run_id: runId, event_seq: 0, t0: Date.now(), seen0: j.oldSeen, actions: [], ...(j.skill ? { skill: j.skill } : {}), ...(j.planRun ? {plan_id: c.planBinding.plan_id} : {}) };
    c.log.push(j.entry);
    running.set(runKey(c, j.p), new AbortController());
  });
  if (!jobs.length && !userText) toast("Nothing new for them to answer yet.");
  save(); render(true);
  if (jobs.length) {
    try {
      // Commit the run id and placeholder before starting the provider. An
      // immediate reload can therefore discover and replay the durable run.
      await flushChatForPlanMutation();
    } catch (error) {
      jobs.forEach(({p, entry, oldSeen}) => {
        entry.pending = false; entry.error = true; entry.text = error.message || String(error);
        p.seen = oldSeen; running.delete(runKey(c, p));
      });
      if (userText) { drafts[c.id] = userText; if (cur() === c && !$('input').value) { $('input').value = userText; autosize(); } }
      save(); render(true); return;
    }
  }
  jobs.forEach((j) => turn(c, j));
}

// Another answer from the same participant to the same messages.
function regenerate(c, e, p) {
  if (busy(c) || lastEntryOf(c, p.name) !== e) return;
  const idx = c.log.indexOf(e);
  const seen = e.seen0 != null ? Math.min(p.seen, e.seen0) : p.seen;
  if (!unseen(c, { name: p.name, seen }, idx).length) { toast("The messages this answered are gone; nothing to answer again."); return; }
  p.seen = seen;
  removeEntry(c, e);
  // Only what the original reply answered; later messages stay unread.
  launch(c, [p], null, null, { regen: !e.error, until: idx, round: e.round });
}

async function stopEntry(c, entry) {
  const p = c.participants.find(participant => participant.name === entry.from);
  const ctl = p && running.get(runKey(c, p));
  if (!entry.run_id) { ctl?.abort(); return; }
  try {
    const response = await fetch('/api/chat/runs/' + encodeURIComponent(entry.run_id) + '/stop', {
      method: 'POST', headers: {'Content-Type':'application/json'}, body: '{}',
    });
    if (!response.ok && response.status !== 404) throw new Error(await response.text());
    ctl?.abort();
  } catch (error) { toast('Could not stop the execution: ' + (error.message || error)); }
}
function stopAll(c) {
  c.log.filter(entry => entry.pending).forEach(entry => { stopEntry(c, entry); });
}

function applyRunEvent(c, p, entry, ev, paint) {
  if (Number.isSafeInteger(ev.seq)) {
    if (ev.seq <= (entry.event_seq || 0)) return;
    entry.event_seq = ev.seq;
  }
  if (ev.type === "session") { p.session = ev.id; delete p.freshContext; }
  else if (ev.type === "execution") { entry.execution_id = ev.id; if (ev.task_id) entry.task_id = ev.task_id; }
  else if (ev.type === "activity") {
    entry.activity = ev.text.split(meta.repo + "/").join("");
    entry.stream_gap = true;
    if (entry.actions.length < 500) entry.actions.push(entry.activity);
    paint();
  } else if (ev.type === "delta") {
    if (entry.stream_gap && entry.text && !/\s$/.test(entry.text)) entry.text += "\n\n";
    entry.stream_gap = false; entry.text += ev.text; paint();
  } else if (ev.type === "done") {
    if (ev.usage) entry.usage = ev.usage;
    if (!ev.ok) {
      entry.error = true; entry.soft = !!ev.stopped; entry.text = ev.error || "agent failed";
      p.seen = entry.seen0;
      if (ev.stopped) entry.stopped = true;
    } else if (!entry.text) entry.text = ev.text;
    entry.run_done = true;
  }
  saveSoon();
}

const waitFor = (ms, signal) => new Promise((resolve, reject) => {
  const timer = setTimeout(resolve, ms);
  signal?.addEventListener('abort', () => { clearTimeout(timer); reject(new DOMException('Aborted', 'AbortError')); }, {once:true});
});

async function reconnectRun(c, p, entry, paint, signal) {
  let delay = 250;
  entry.reconnecting = true; paint();
  while (!entry.run_done) {
    try {
      const response = await fetch('/api/chat/runs/' + encodeURIComponent(entry.run_id) + '?after=' + (entry.event_seq || 0), {signal});
      if (response.status === 404) throw Object.assign(new Error('The execution record is no longer available.'), {terminal:true});
      if (!response.ok) throw new Error(await response.text());
      const snapshot = await response.json();
      for (const event of snapshot.events || []) applyRunEvent(c, p, entry, event, paint);
      if (['completed','failed','stopped','interrupted'].includes(snapshot.state)) {
        if (!entry.run_done) {
          entry.error = snapshot.state !== 'completed'; entry.soft = snapshot.state === 'stopped' || snapshot.state === 'interrupted';
          entry.text ||= snapshot.state === 'stopped' ? 'Stopped.' : 'The execution ended before a final response was saved.';
          p.seen = entry.seen0; entry.run_done = true;
        }
        break;
      }
      delay = 250;
    } catch (error) {
      if (error.name === 'AbortError' || error.terminal) throw error;
      entry.activity = 'Reconnecting'; paint();
      delay = Math.min(delay * 2, 2000);
    }
    await waitFor(delay, signal);
  }
  entry.reconnecting = false;
}

function finishRunUI(c, p, entry, key) {
  entry.pending = false; entry.activity = ""; entry.reconnecting = false; entry.dur = Date.now() - entry.t0;
  delete entry.stream_gap; delete entry.run_done;
  running.delete(key);
  c.updated = Date.now(); save();
  if (cur() === c) {
    paintCard(c, entry);
    if (!busy(c)) c.log.forEach((e) => paintCard(c, e));
    if (view.lastUser && !busy(c)) view.lastUser._fill();
  }
  render();
  fetchQuotaState();
  if (c.planBinding?.plan_id && !busy(c)) refreshPlanBinding(c).catch(error => { if (cur() === c) showPlanError(error.message); });
  if (!busy(c)) announce(cur() === c ? "Replies finished." : `Replies finished in “${c.title}”.`);
  else announce(`${p.name} finished.`);
}

function resumePendingRuns() {
  for (const c of state.convs) for (const entry of c.log.filter(e => e.pending && e.run_id)) {
    const p = c.participants.find(participant => participant.name === entry.from);
    if (!p) { entry.pending = false; entry.error = true; entry.soft = true; entry.text ||= 'The participant for this execution no longer exists.'; continue; }
    const key = runKey(c, p);
    if (running.has(key)) continue;
    const ctl = new AbortController(); running.set(key, ctl);
    let queued = false;
    const paint = () => {
      if (queued) return; queued = true;
      requestAnimationFrame(() => { queued = false; if (cur() === c) paintCard(c, entry); });
    };
    reconnectRun(c, p, entry, paint, ctl.signal)
      .catch(error => {
        if (error.name === 'AbortError') { if (!entry.text) { entry.text = 'Stopped.'; entry.error = true; entry.soft = true; } else entry.stopped = true; }
        else { entry.error = true; entry.soft = true; entry.text = error.message || String(error); p.seen = entry.seen0; }
      })
      .finally(() => finishRunUI(c, p, entry, key));
  }
  render(true);
}

async function turn(c, { p, text: prompt, entry, planRun }) {
  const key = runKey(c, p);
  let queued = false;
  const paint = () => {
    if (queued) return; queued = true;
    requestAnimationFrame(() => {
      queued = false;
      if (cur() !== c) return; // still streaming into a conversation that isn't on screen
      const stick = nearEnd();
      paintCard(c, entry);
      if (stick) toEnd(); else $("jump").classList.add("show");
    });
  };
  const fail = (msg, soft) => { entry.error = true; entry.soft = !!soft; entry.text = msg; p.seen = entry.seen0; };
  const ctl = running.get(key);
  try {
    const res = await fetch("/api/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        checkout: c.checkout || null, git_context: c.gitCurrent || c.gitOrigin || null, agent: entry.agent, conversation_id: c.id, prompt, session_id: p.session, system: systemFor(c, p), write: planRun ? planRun.intent === 'execute' && !!p.write : !!p.write,
        ...(planRun || {}), participant: p.name, run_id: entry.run_id,
        model: p.model || null, effort: p.effort || null,
      }),
      signal: ctl.signal,
    });
    if (!res.ok) {
      if (res.status === 409) { const last = [...c.log].reverse().find(e => e.from === "you"); if (last && !drafts[c.id]) { drafts[c.id] = last.text; if (cur() === c && !$("input").value) { $("input").value = last.text; autosize(); } saveDraftsSoon(); } }
      throw Object.assign(new Error(await res.text()), {noReconnect:true});
    }
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let i;
      while ((i = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, i); buf = buf.slice(i + 1);
        if (!line.trim()) continue; // heartbeat
        applyRunEvent(c, p, entry, JSON.parse(line), paint);
      }
    }
    if (!entry.run_done && entry.run_id) await reconnectRun(c, p, entry, paint, ctl.signal);
  } catch (err) {
    if (err.name === "AbortError") { if (!entry.text) fail("Stopped.", true); else entry.stopped = true; }
    else if (entry.run_id && !err.noReconnect && !err.terminal) {
      try { await reconnectRun(c, p, entry, paint, ctl.signal); }
      catch (reconnectError) { if (reconnectError.name === 'AbortError') { if (!entry.text) fail('Stopped.', true); else entry.stopped = true; } else fail(reconnectError.message || String(reconnectError), true); }
    } else fail(String(err.message || err), !!err.terminal);
  } finally {
    finishRunUI(c, p, entry, key);
  }
}

/* ---------- input ---------- */
const PASTE_MARKER_SOURCE = String.raw`\[Pasted Text · \+(\d+) lines\]`;
const pasteMarker = lines => `[Pasted Text · +${lines} lines]`;
function blocksFor(c) {
  if (!c) return [];
  if (!Array.isArray(draftPastes[c.id])) draftPastes[c.id] = [];
  return draftPastes[c.id];
}
function insertPasteMarker(c, marker, block) {
  const input = $("input"), value = input.value;
  const start = input.selectionStart, end = input.selectionEnd;
  const lineStart = value.lastIndexOf("\n", start - 1) + 1;
  let lineEnd = value.indexOf("\n", end); if (lineEnd < 0) lineEnd = value.length;
  const head = value.slice(0, lineStart);
  const sameLineText = value.slice(lineStart, start) + value.slice(end, lineEnd);
  const tail = value.slice(lineEnd);
  let replacement = marker, caret = head.length + marker.length + 1;
  if (sameLineText) replacement += "\n" + sameLineText + tail;
  else if (tail.startsWith("\n")) replacement += tail;
  else replacement += "\n" + tail;
  input.value = head + replacement;
  input.setSelectionRange(caret, caret);
  if (block) blocksFor(c).push(block);
  hist.idx = -1; onInput();
}
function expandPastedText(c, displayText) {
  const available = blocksFor(c), used = new Set();
  const pasted = [], re = new RegExp(PASTE_MARKER_SOURCE, "g");
  let output = "", offset = 0, match;
  while ((match = re.exec(displayText))) {
    output += displayText.slice(offset, match.index);
    let index = available.findIndex((block, i) => !used.has(i) && block.marker === match[0]);
    if (index < 0) index = available.findIndex(block => block.marker === match[0]);
    const block = index >= 0 ? available[index] : null;
    if (block) {
      used.add(index);
      const start = output.length;
      output += block.text;
      pasted.push({ start, end: output.length, lines: block.lines });
    } else output += match[0];
    offset = match.index + match[0].length;
  }
  output += displayText.slice(offset);
  return { text: output, pasted };
}
function transferHasMedia(transfer) {
  return !!transfer?.files?.length || [...(transfer?.items || [])].some(item => item.kind === "file");
}
function pasteFromTransfer(event, transfer) {
  const c = cur(); if (!c) return;
  event.preventDefault();
  if (transferHasMedia(transfer)) { toast("Media can't be pasted into chat."); return; }
  const text = transfer?.getData("text/plain") || "";
  if (!text) { toast("Only plain text can be pasted into chat."); return; }
  const knownMarker = new RegExp(`^${PASTE_MARKER_SOURCE}$`).exec(text);
  if (knownMarker && blocksFor(c).some(block => block.marker === text)) {
    insertPasteMarker(c, text);
    return;
  }
  const normalized = text.replace(/\r\n?/g, "\n");
  const lines = normalized.split("\n").length, marker = pasteMarker(lines);
  insertPasteMarker(c, marker, { id: crypto.randomUUID(), marker, text: normalized, lines });
}
function normalizeMarkerLine(input) {
  const start = input.selectionStart;
  const lineStart = input.value.lastIndexOf("\n", start - 1) + 1;
  let lineEnd = input.value.indexOf("\n", start); if (lineEnd < 0) lineEnd = input.value.length;
  const line = input.value.slice(lineStart, lineEnd), re = new RegExp(PASTE_MARKER_SOURCE, "g");
  const markers = [...line.matchAll(re)].map(match => match[0]);
  if (!markers.length || markers.length === 1 && line === markers[0]) return;
  const rest = line.replace(re, "").trim();
  const replacement = markers.join("\n") + (rest ? "\n" + rest : "");
  input.value = input.value.slice(0, lineStart) + replacement + input.value.slice(lineEnd);
  const caret = lineStart + replacement.length;
  input.setSelectionRange(caret, caret);
}
function clearInput(c) { hist.idx = -1; $("input").value = ""; drafts[c.id] = ""; delete draftPastes[c.id]; saveDraftsSoon(); c.toFromText = false; autosize(); acClose(); }
let sendPending = false;
$("form").onsubmit = async (e) => {
  e.preventDefault();
  if (sendPending) return;
  sendPending = true;
  try {
  const c = cur();
  if (!c) return;
  // While replies are coming in, the button is Stop; Enter does nothing, so
  // a reflex Enter on the next message can't throw the replies away.
  if (busy(c)) { if (e.submitter === $("send")) stopAll(c); else renderHint(); return; }
  const text = $("input").value.trim();
  if (text === "/fork") {
    clearInput(c);
    forkConversation(c);
    return;
  }
  if (text === "/clear") {
    clearInput(c);
    clearConversation(c);
    return;
  }
  const m = (text + " ").match(/^((?:@[\w-]+\s+)+)([\s\S]*)$/);
  let body = text;
  if (m) {
    const names = m[1].trim().split(/\s+/).map((n) => n.slice(1));
    const unknown = expandMentions(c, names).filter((n) => !c.participants.some((p) => p.name === n));
    if (unknown.length) { toast("Unknown participant: " + unknown.join(", ")); return; }
    const off = names.filter((n) => c.participants.find((p) => p.name === n)?.inactive);
    if (off.length) { toast(off.join(", ") + (off.length > 1 ? " are" : " is") + " disabled. Enable " + (off.length > 1 ? "them" : "it") + " in the participant's settings first."); return; }
    body = m[2].trim();
  }
  const targets = recipients(c, text);
  if (!targets.length) { toast("Pick who receives the message in “To:”."); return; }
  if (body === "/fresh") {
    clearInput(c);
    freshContext(c, targets);
    return;
  }
  const to = targets.length === active(c).length ? null : targets.map((p) => p.name);
  if (!body) { // nothing to add: let them answer what's new to them
    const ready = targets.filter((p) => unseen(c, p).length);
    if (!ready.length) { toast(text ? "Nothing new for them to answer yet." : "Write a message first."); return; }
    if (!(await prepareSend(c, ready))) return;
    clearInput(c); launch(c, ready, null, null, { preflightDone: true }); return;
  }
  const expanded = expandPastedText(c, body);
  const sk = skillOf(expanded.text), problem = sk && skillProblem(sk, targets);
  if (problem) { toast(problem); return; }
  if (!(await prepareSend(c, targets))) return;
  clearInput(c);
  launch(c, targets, expanded.text, to, { preflightDone: true, pasted: expanded.pasted });
  } finally { sendPending = false; }
};

// Autocomplete for the mention chain and a "/skill" right after it.
let acItems = [], acSel = 0;
const MENTION_Q = /^((?:@[\w-]+\s+)*)@([\w-]*)$/;
const SKILL_Q = /^((?:@[\w-]+\s+)*)\/([\w:.-]*)$/;
function acClose() { acItems = []; $("ac").classList.remove("open"); $("input").removeAttribute("aria-activedescendant"); }
function acUpdate() {
  const c = cur(), i = $("input"), before = i.value.slice(0, i.selectionStart);
  let m;
  acItems = [];
  if (c && (m = before.match(MENTION_Q))) {
    const used = m[1].trim().split(/\s+/).filter(Boolean).map((n) => n.slice(1));
    const q = m[2].toLowerCase();
    const everyone = active(c).length > 1 && !used.includes("all") && !c.participants.some((p) => p.name === "all") && "all".startsWith(q)
      ? [{ type: "@", value: "all", label: "@all", sub: "Everyone in this conversation" }] : [];
    acItems = [...everyone, ...active(c).filter((p) => !used.includes(p.name) && p.name.toLowerCase().startsWith(q))
      .map((p) => ({ type: "@", value: p.name, label: "@" + p.name, sub: cap(p.agent) + (p.role ? " · " + p.role : "") }))];
  } else if (c && (m = before.match(SKILL_Q))) {
    const q = m[2].toLowerCase();
    const allAgy = recipients(c).length > 0 && recipients(c).every((p) => kindOf(p) === "agy");
    const fork = { type: "/", value: "fork", label: "/fork", sub: "Create a conversation branch", off: busy(c) ? "wait for replies to finish" : "" };
    const fresh = { type: "/", value: "fresh", label: "/fresh", sub: "Restart selected agents without replaying visible history", off: busy(c) ? "wait for replies to finish" : "" };
    const clear = { type: "/", value: "clear", label: "/clear", sub: "Clear the conversation and reset the context" };
    acItems = [fork, fresh, clear, ...skills.filter((s) => s.name.toLowerCase().includes(q))
      .sort((a, b) => b.name.toLowerCase().startsWith(q) - a.name.toLowerCase().startsWith(q))
      .map((s) => ({ type: "/", value: s.name, label: "/" + s.name, arg: s.argument_hint, sub: s.description,
        off: skillProblem(s.name, recipients(c)),
        sub: s.description + " · Claude/Codex · one recipient" + (s.requires_write ? " · edit access required" : " · read access") }))]
      .filter((item) => item.value.toLowerCase().includes(q));
  }
  acSel = Math.min(acSel, Math.max(acItems.length - 1, 0));
  const box = $("ac"); box.textContent = "";
  box.classList.toggle("open", acItems.length > 0);
  acItems.forEach((it, idx) => {
    const row = el("div", "opt" + (idx === acSel ? " sel" : "") + (it.off ? " off" : ""));
    row.id = "ac-" + idx; row.setAttribute("role", "option"); row.setAttribute("aria-selected", String(idx === acSel));
    row.appendChild(el("b", "", it.label));
    if (it.arg) row.appendChild(el("small", "hintarg", it.arg));
    row.appendChild(el("small", "", it.off || it.sub || ""));
    if (it.off) row.title = it.off;
    row.onmousedown = (ev) => { ev.preventDefault(); acPick(idx); };
    box.appendChild(row);
  });
  if (acItems.length) i.setAttribute("aria-activedescendant", "ac-" + acSel); else i.removeAttribute("aria-activedescendant");
}
function acPick(idx) {
  const i = $("input"), it = acItems[idx];
  if (!it || it.off) return;
  const re = it.type === "@" ? /@[\w-]*$/ : /\/[\w:.-]*$/;
  const before = i.value.slice(0, i.selectionStart).replace(re, it.type + it.value + " ");
  i.value = before + i.value.slice(i.selectionStart);
  i.selectionStart = i.selectionEnd = before.length;
  acSel = 0; onInput();
}
// Shell-style history: with the caret at the very start, Up recalls earlier messages (oldest last) and Down
// comes back toward the newest, ending on whatever was being typed. Typing anything ends the browsing.
const hist = { idx: -1, draft: "" };
const userHistory = (c) => {
  const out = [];
  c.log.forEach((e) => {
    if (e.from !== "you" || !e.text) return;
    const t = (e.to?.length ? e.to.map((n) => "@" + n + " ").join("") : "") + e.text;
    if (out[out.length - 1] !== t) out.push(t);
  });
  return out;
};
function recall(text) {
  const t = $("input");
  t.value = text; t.setSelectionRange(0, 0);
  onInput();
}
function historyKey(e) {
  if ((e.key !== "ArrowUp" && e.key !== "ArrowDown") || e.shiftKey || e.altKey || e.ctrlKey || e.metaKey || e.isComposing) return false;
  const t = $("input"), c = cur();
  if (!c || t.selectionStart !== 0 || t.selectionEnd !== 0) return false;
  const h = userHistory(c);
  if (e.key === "ArrowUp") {
    if (!h.length) return false;
    if (hist.idx < 0) { hist.draft = t.value; hist.idx = h.length; }
    if (hist.idx > 0) { hist.idx--; recall(h[hist.idx]); }
    return true;
  }
  if (hist.idx < 0) return false;
  hist.idx++;
  if (hist.idx >= h.length) { hist.idx = -1; recall(hist.draft); } else recall(h[hist.idx]);
  return true;
}
$("input").onkeydown = (e) => {
  if (acItems.length) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      acSel = (acSel + (e.key === "ArrowDown" ? 1 : acItems.length - 1)) % acItems.length; acUpdate(); return;
    }
    if (e.key === "Tab" || e.key === "Enter") { e.preventDefault(); acPick(acSel); return; }
    if (e.key === "Escape") { e.stopPropagation(); acClose(); return; }
  }
  if (historyKey(e)) { e.preventDefault(); return; }
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); $("form").requestSubmit(); }
};
function autosize() {
  const t = $("input");
  t.style.height = "auto";
  // scrollHeight leaves out the border, which border-box sizing needs back (else a scrollbar shows).
  const max = window.innerHeight * 0.4, want = t.scrollHeight + (t.offsetHeight - t.clientHeight);
  t.style.height = Math.min(want, max) + "px";
  t.style.overflowY = want > max ? "auto" : "hidden";
}
function onInput() {
  const t = $("input");
  normalizeMarkerLine(t);
  // Capitalise the first letter typed; @mentions and /skills start with a symbol, so they are left alone.
  if (/^\p{Ll}/u.test(t.value)) {
    const { selectionStart: a, selectionEnd: b } = t;
    t.value = t.value.charAt(0).toUpperCase() + t.value.slice(1);
    t.setSelectionRange(a, b);
  }
  autosize(); acUpdate();
  const c = cur();
  if (c) { drafts[c.id] = $("input").value; saveDraftsSoon(); syncPillsFromText(c); renderComposer(); }
}
$("input").oninput = () => { hist.idx = -1; onInput(); };
$("input").onpaste = event => pasteFromTransfer(event, event.clipboardData);
$("input").ondragover = event => { event.preventDefault(); if (event.dataTransfer) event.dataTransfer.dropEffect = "copy"; };
$("input").ondrop = event => pasteFromTransfer(event, event.dataTransfer);
$("input").onblur = acClose;
$("input").onclick = acUpdate;

/* ---------- participant dialog ---------- */
let editing = null; // participant being edited, or null when adding
let dlgHue = 0;
// Suggestions for the datalists; any value can still be typed. Codex: `codex debug models` (listed ones); agy efforts: `agy --help`.
const MODELS = {
  claude: ["opus", "sonnet", "haiku"],
  agy: [],
  codex: ["gpt-6-astra", "gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.5"],
};
const EFFORTS = {
  claude: ["low", "medium", "high", "xhigh", "max"],
  agy: ["low", "medium", "high", "max"],
  codex: ["low", "medium", "high", "xhigh", "max", "ultra"],
};
// What the agent's own CLI reports (via the server); the static lists above are the fallback.
const liveModels = new Map();
function fetchModels(agent) {
  if (!liveModels.has(agent)) {
    const p = fetch("/api/models?agent=" + encodeURIComponent(agent))
      .then((r) => (r.ok ? r.json() : {}), () => ({}))
      .then((j) => { if (!j.models?.length) liveModels.delete(agent); return j; }); // retry later if it came back empty
    liveModels.set(agent, p);
  }
  return liveModels.get(agent);
}
const access = () => document.querySelector('input[name="paccess"]:checked').value;
// The chosen model/effort ("" = the agent's own default); kept here so a late list from the CLI can't reset the pick.
let dlgModel = "", dlgEffort = "";
// A plain select: "Agent default", then the suggestions. A saved value that isn't listed stays selectable.
function fillSelect(id, values, chosen, fallback) {
  const sel = $(id); sel.textContent = "";
  const def = el("option", "", "Agent default" + (fallback ? ` (${cap(fallback)})` : "")); def.value = "";
  sel.appendChild(def);
  uniq([...values.filter(Boolean), chosen].filter(Boolean)).forEach((v) => { const o = el("option", "", cap(v)); o.value = v; sel.appendChild(o); });
  sel.value = chosen;
}
async function openDialog(p) {
  const c = cur(); if (!c || !meta.agents.length) return;
  try { await refreshReadiness(c); } catch { toast('Environment status unavailable. Check Environment before adding an agent.'); }
  if (cur() !== c) return;
  editing = p || null;
  $("ptitle").textContent = p ? "Edit participant" : "Add participant";
  $("psave").textContent = "Save";
  $("pname").value = p ? p.name : "";
  updateAgentOptions(p ? p.agent : defaultAgent());
  updateParticipantReadiness();
  dlgModel = p?.model || ""; dlgEffort = p?.effort || "";
  $("prole").value = p?.role || "";
  document.querySelector(`input[name="paccess"][value="${p?.write ? "write" : "read"}"]`).checked = true;
  dlgHue = p ? hueOf(p) : pickHue(c);
  const replying = !!p && isRunning(c, p);
  $("pname").disabled = replying; $("pagent").disabled = replying;
  $("pname").title = $("pagent").title = replying ? `Wait for ${p.name} to finish replying` : "";
  $("premove").hidden = !p;
  $("ptoggle").hidden = !p;
  dlgInactive = !!p?.inactive;
  if (p) renderToggle();
  askRemove(false);
  $("perror").hidden = true;
  updateDialog();
  $("pdialog").showModal();
  (p ? $("prole") : $("pname")).focus();
}
function updateDialog() {
  const c = cur(), agent = $("pagent").value, d = meta.details?.[agent] || {}, kind = d.kind || "claude";
  const suggest = (live) => {
    fillSelect("pmodel", live?.models?.length ? live.models : MODELS[kind] || [], dlgModel, d.model);
    fillSelect("peffort", live?.efforts?.length ? live.efforts : EFFORTS[kind] || [], dlgEffort, d.effort);
  };
  suggest();
  fetchModels(agent).then((live) => { if ($("pagent").value === agent) suggest(live); });
  const w = $("pwritewarn");
  w.hidden = access() !== "write";
  w.replaceChildren(icon("alert"), el("span", "", `Can create, change and delete files in ${meta.repo || "this repository"}, following the permission mode in your config.`));
  const hues = $("phues"); hues.textContent = "";
  const options = PALETTE.includes(dlgHue) ? PALETTE : [...PALETTE, dlgHue];
  const taken = new Set(c ? c.participants.filter((q) => q !== editing).map(hueOf) : []);
  options.forEach((h) => {
    const b = el("button", "hue"); b.type = "button";
    applyParticipantColour(b, h);
    b.setAttribute("role", "radio"); b.setAttribute("aria-checked", String(h === dlgHue));
    const colour = paletteColour(h);
    b.setAttribute("aria-label", (colour?.name || `Hue ${h}`) + (taken.has(h) ? " (used by someone else)" : ""));
    b.title = taken.has(h) ? "Used by someone else" : "";
    b.tabIndex = h === dlgHue ? 0 : -1;
    b.onclick = () => { dlgHue = h; updateDialog(); hues.querySelector('[aria-checked="true"]').focus(); };
    b.onkeydown = (ev) => {
      if (ev.key !== "ArrowRight" && ev.key !== "ArrowLeft") return;
      ev.preventDefault();
      const at = options.indexOf(h), next = options[(at + (ev.key === "ArrowRight" ? 1 : options.length - 1)) % options.length];
      dlgHue = next; updateDialog(); hues.querySelector('[aria-checked="true"]').focus();
    };
    hues.appendChild(b);
  });
}
function dialogError(msg) { $("perror").textContent = msg; $("perror").hidden = false; }
$("pmodel").onchange = () => { dlgModel = $("pmodel").value; };
$("peffort").onchange = () => { dlgEffort = $("peffort").value; };
$("pagent").onchange = () => { dlgModel = ""; dlgEffort = ""; updateDialog(); }; // each agent has its own models
document.querySelectorAll('input[name="paccess"]').forEach((r) => { r.onchange = updateDialog; });
$("pname").oninput = () => { $("pname").value = $("pname").value.replace(/\s+/g, "-").replace(/[^\w-]/g, ""); };
$("pcancel").onclick = () => $("pdialog").close();
// Escape uses the dialog's native cancel behaviour. A click on its backdrop
// targets the dialog itself, rather than the form contained by it.
$("pdialog").addEventListener("click", (event) => {
  if (event.target === event.currentTarget) $("pdialog").close();
});
// The switch in the dialog header pauses a participant: it keeps its settings and history, and gets no new
// messages. Like the other fields it only takes effect on Save.
let dlgInactive = false;
function renderToggle() {
  const t = $("ptoggle");
  t.setAttribute("aria-checked", String(!dlgInactive));
  t.classList.toggle("off", dlgInactive);
  t.querySelector(".lbl").textContent = dlgInactive ? "Inactive" : "Active";
  t.title = dlgInactive
    ? "Inactive: gets no new messages (and no @all). Its past replies stay."
    : "Active. Turn off to keep this participant and its history but stop sending it new messages.";
}
$("ptoggle").onclick = () => {
  const c = cur(), p = editing;
  if (!c || !p) return;
  if (!dlgInactive) { // turning it off
    if (isRunning(c, p)) return dialogError(`Wait for ${p.name} to finish replying.`);
    if (active(c).filter((q) => q !== p).length < 1) return dialogError("At least one participant must stay active.");
  }
  $("perror").hidden = true;
  dlgInactive = !dlgInactive;
  renderToggle();
};
const askRemove = (on) => { $("pbuttons").hidden = on; $("pconfirm").hidden = !on; };
// Removing a participant asks inside the edit dialog (no modal over a modal); Cancel gets the focus.
$("premove").onclick = () => {
  const c = cur(), p = editing;
  if (!c || !p) return;
  if (busy(c)) return dialogError("Wait for the replies to finish.");
  if (c.participants.length < 2) return dialogError("A conversation needs at least one participant.");
  $("perror").hidden = true;
  $("pconfirmtext").replaceChildren(icon("alert"), el("span", "", `Remove ${p.name}? They leave this conversation, but the existing messages stay.`));
  askRemove(true);
  $("pkeep").focus();
};
$("pkeep").onclick = () => { askRemove(false); $("premove").focus(); };
$("pdoremove").onclick = () => {
  const c = cur(), p = editing;
  if (!c || !p) return;
  if (busy(c)) return dialogError("Wait for the replies to finish.");
  if (c.participants.length < 2) return dialogError("A conversation needs at least one participant.");
  const idx = c.participants.indexOf(p), sendTo = c.sendTo;
  c.participants.splice(idx, 1);
  if (c.sendTo) { c.sendTo = c.sendTo.filter((n) => n !== p.name); if (!c.sendTo.length) c.sendTo = null; }
  $("pdialog").close(); save(); render();
  toast(`Removed ${p.name}`, () => { c.participants.splice(idx, 0, p); c.sendTo = sendTo; save(); render(); });
};
$("pform").onsubmit = (e) => {
  e.preventDefault();
  const c = cur(); if (!c) return;
  if (!editing && c.participants.length >= MAX_PARTICIPANTS) {
    return dialogError(`A conversation can have at most ${MAX_PARTICIPANTS} participants.`);
  }
  const agent = $("pagent").value;
  if (!meta.agents.includes(agent)) return dialogError("Choose an available agent.");
  if ((!editing || editing.agent !== agent) && !ChatOnboarding.ready(readiness, agent)) return dialogError("Choose a ready agent. Check Environment for recovery commands.");
  const name = cleanName($("pname").value) || (editing ? editing.name : uniqueName(c, cleanName(cap(agent)) || "agent"));
  if (!NAME_OK.test(name)) return dialogError("Use letters, digits, - and _ in the name.");
  if (c.participants.some((q) => q !== editing && q.name === name)) return dialogError(`There's already a participant called ${name}.`);
  const fields = { agent, role: $("prole").value.trim(), write: access() === "write", model: $("pmodel").value, effort: $("peffort").value, hue: dlgHue };
  if (editing) {
    const p = editing;
    if (isRunning(c, p) && (name !== p.name || agent !== p.agent)) return dialogError(`Wait for ${p.name} to finish replying.`);
    if (name !== p.name) {
      // The name is the key in the log; old prompts others got keep the old
      // name, but systemFor is rebuilt every turn with the new one.
      const old = p.name;
      c.log.forEach((x) => {
        if (x.from === old) x.from = name;
        if (x.to) x.to = x.to.map((n) => (n === old ? name : n));
      });
      if (c.sendTo) c.sendTo = c.sendTo.map((n) => (n === old ? name : n));
      p.name = name;
    }
    if (agent !== p.agent) { p.agent = agent; p.session = ""; }
    Object.assign(p, { role: fields.role, write: fields.write, model: fields.model, effort: fields.effort, hue: fields.hue });
    if (!!p.inactive !== dlgInactive) {
      p.inactive = dlgInactive;
      if (dlgInactive) {
        if (c.sendTo) { c.sendTo = c.sendTo.filter((n) => n !== p.name); if (!c.sendTo.length) c.sendTo = null; }
      } else p.seen = c.log.length; // no catching up on what it missed
    }
  } else {
    const p = addParticipant(c, { name, ...fields });
    if (c.sendTo) c.sendTo.push(p.name);
  }
  $("pdialog").close();
  c.updated = Date.now(); save(); render();
};

/* ---------- configuration dialog ---------- */
let settingsDraft = null, configSaving = false;
function setOptions(id, names, selected) {
  const select = $(id); select.textContent = "";
  names.forEach((name) => {
    const option = el("option", "", cap(name)); option.value = name;
    option.selected = name === selected;
    select.appendChild(option);
  });
}
async function openConfig() {
  const error = $("cerror"), saveBtn = $("csave");
  error.hidden = true; $("cwarn").hidden = true;
  saveBtn.disabled = true;
  try {
    const res = await fetch("/api/config");
    if (!res.ok) throw new Error(await res.text());
    const config = await res.json();
    await refreshReadiness();
    setOptions("cdefault", ["", ...config.agents], config.default_agent);
    $("cdefault").options[0].textContent = "No preference — ask for each new chat";
    for (const option of $("cdefault").options) option.disabled = !!option.value && option.value !== config.default_agent && !ChatOnboarding.ready(readiness, option.value);
    settingsDraft = { hiddenAgents: new Set(state.hiddenAgents || []), defaultAgent: config.default_agent };
    renderAgentVisibility();
    $("cpermission").value = config.permission_mode;
    $("cskippermissions").checked = !!config.skip_permissions;
    $("csandbox").value = config.sandbox;
    $("csandboximage").value = config.sandbox_image;
    $("cdialog").showModal();
  } catch (err) {
    settingsDraft = null;
    error.textContent = "Couldn't load configuration: " + (err.message || err);
    error.hidden = false;
    $("cdialog").showModal();
  } finally {
    saveBtn.disabled = !settingsDraft;
  }
}
$("ccancel").onclick = () => $("cdialog").close();
$('cdialog').addEventListener('click', event => {
  if (event.target === event.currentTarget && !configSaving) $('cdialog').close();
});
$('cdialog').addEventListener('close', () => { settingsDraft = null; });
$('cdialog').addEventListener('cancel', e => { if (configSaving) e.preventDefault(); });
$('cdefault').addEventListener('change', renderAgentVisibility);
$("cform").onsubmit = async (e) => {
  e.preventDefault();
  if (!settingsDraft || configSaving) return;
  const error = $("cerror"), saveBtn = $("csave");
  const hiddenAgents = [...settingsDraft.hiddenAgents];
  error.hidden = true; saveBtn.disabled = true; configSaving = true; $('ccancel').disabled = true;
  const payload = {
    default_agent: $("cdefault").value,
    confirm_default: $("cdefault").value !== settingsDraft.defaultAgent,
    permission_mode: $("cpermission").value,
    skip_permissions: $("cskippermissions").checked,
    sandbox: $("csandbox").value,
    sandbox_image: $("csandboximage").value.trim(),
  };
  try {
    const res = await fetch("/api/config", {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(await res.text());
    const config = await res.json();
    meta.default = config.default_agent;
    meta.agents = config.agents;
    meta.sandboxed = config.sandbox !== "none";
    state.hiddenAgents = hiddenAgents.filter(name => name !== config.default_agent);
    save(); updateAgentOptions();
    $("cdialog").close();
    try { await refreshReadiness(cur(), { full: true, catalog: true }); }
    catch { toast('Configuration saved. Refresh Environment to update its status.'); render(); return; }
    render();
    toast("Configuration saved. New agent turns use it.");
  } catch (err) {
    error.textContent = "Couldn't save configuration: " + (err.message || err);
    error.hidden = false;
  } finally {
    configSaving = false; $('ccancel').disabled = false; saveBtn.disabled = !settingsDraft;
  }
};

/* ---------- buttons and shortcuts ---------- */
$("new").appendChild(icon("plus"));
$("new").setAttribute("aria-label", "New conversation");
$("new").onclick = newConv;
$("importfile").onchange = (e) => {
  const file = e.target.files[0]; e.target.value = "";
  importFile(file);
};
$("menu").appendChild(icon("menu"));
$("menu").onclick = () => document.body.classList.add("drawer");
$("scrim").onclick = () => document.body.classList.remove("drawer");
$("addbtn").appendChild(icon("plus"));
setIcon($("copyrepo"), "copy");
setIcon($("branchicon"), "branch");
setIcon($("planicon"), "plan");
setIcon($("advancedicon"), "settings");
setIcon($("historyicon"), "history");
setIcon($("advancedclose"), "close");
setIcon($("historyclose"), "close");
$("copyrepo").onclick = () => copyText(meta.repo, $("copyrepo"));
$("addbtn").onclick = () => openDialog(null);
$("configbtn").appendChild(icon("settings"));
$("configbtn").onclick = openConfig;
const systemTheme = matchMedia('(prefers-color-scheme: dark)');
function currentTheme() {
  return document.documentElement.dataset.theme || (systemTheme.matches ? 'dark' : 'light');
}
function renderThemeButton() {
  const next = currentTheme() === 'dark' ? 'light' : 'dark';
  setIcon($("themebtn"), next === 'dark' ? 'moon' : 'sun');
  const label = `Switch to ${next} mode`;
  $("themebtn").setAttribute('aria-label', label);
  $("themebtn").title = label;
}
renderThemeButton();
systemTheme.addEventListener?.('change', () => { if (!document.documentElement.dataset.theme) renderThemeButton(); });
$("themebtn").onclick = () => {
  const theme = currentTheme() === 'dark' ? 'light' : 'dark';
  document.documentElement.dataset.theme = theme;
  try { localStorage.setItem(THEME_KEY, theme); } catch {}
  renderThemeButton();
  announce(`${cap(theme)} mode enabled`);
};
$("quotabtn").appendChild(icon("meter"));
$("quotarefresh").appendChild(icon("retry"));
$("quotaclose").appendChild(icon("close"));
function closeQuotas() { $("quotas").hidden = true; $("quotabtn").setAttribute("aria-expanded", "false"); }
$("quotabtn").onclick = () => {
  const open = $("quotas").hidden; $("quotas").hidden = !open;
  $("quotabtn").setAttribute("aria-expanded", String(open));
  if (open) { renderQuotas(); fetchQuotaState(); }
};
$("quotaclose").onclick = () => { closeQuotas(); $("quotabtn").focus(); };
$("quotarefresh").onclick = () => refreshQuotas();
document.addEventListener("click", e => { if (!$("quotas").hidden && !$("quotas").contains(e.target) && !$("quotabtn").contains(e.target)) closeQuotas(); });
$("search").oninput = renderSidebar;
$("search").onkeydown = (e) => { if (e.key === "Escape" && $("search").value) { e.stopPropagation(); $("search").value = ""; renderSidebar(); } };

document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.shiftKey && (e.key === "O" || e.key === "o")) { if (document.querySelector('dialog[open]')) return; e.preventDefault(); newConv(); return; }
  if (e.key !== "Escape" || e.defaultPrevented || document.querySelector('dialog[open]')) return;
  const contextPopover = $('conversationadvanced').open ? $('conversationadvanced') : $('planorigin').open ? $('planorigin') : null;
  if (contextPopover) { e.preventDefault(); contextPopover.open = false; contextPopover.querySelector('summary').focus(); return; }
  if (!$("quotas").hidden) { e.preventDefault(); closeQuotas(); $("quotabtn").focus(); return; }
  if (openMenu) { closeMenu(); return; }
  if (document.body.classList.contains("drawer")) { document.body.classList.remove("drawer"); return; }
  const c = cur();
  if (c && collapseAll(c)) { e.preventDefault(); return; }
  if (c && busy(c)) { e.preventDefault(); stopAll(c); }
});

(async () => {
  loadLocal();
  try {
    meta = Object.assign(meta, await (await fetch("/api/agents")).json());
    serverCheckout = meta.repo;
  } catch (err) {
    $("msgs").textContent = "Couldn't reach the aic chat server: " + err.message; return;
  }
  try {
    const res = await fetch("/api/chat/state");
    if (!res.ok) throw new Error(await res.text());
    const saved = await res.json();
    if (Number.isSafeInteger(saved.revision)) remoteRevision = saved.revision;
    if (saved.state && typeof saved.state === "object" && Array.isArray(saved.state.convs)) {
      const localPastes = draftPastes;
      state = Object.assign({ convs: [], current: null, layout: "auto" }, saved.state);
      const remotePastes = state.pastedDrafts && typeof state.pastedDrafts === "object" && !Array.isArray(state.pastedDrafts) ? state.pastedDrafts : {};
      draftPastes = { ...remotePastes, ...localPastes };
      state.pastedDrafts = draftPastes;
      const localDrafts = drafts;
      drafts = saved.drafts && typeof saved.drafts === "object" ? saved.drafts : {};
      // Keep unsent local work after a version conflict or interrupted save.
      for (const c of state.convs) if (typeof localDrafts[c.id] === 'string' && localDrafts[c.id]) drafts[c.id] = localDrafts[c.id];
      // Apply compatibility cleanups to records saved by older UI versions.
      normalizeState();
    } else {
      // First launch after this upgrade: promote the browser history exactly
      // once, then make SQLite the source of truth for future launches.
      remoteDirty = true;
    }
    remoteReady = true;
    if (remoteDirty) saveRemote();
  } catch (err) {
    remoteError(err);
    // The app remains usable with its cache when an old server is still
    // running during an upgrade.
  }
  try { await refreshReadiness(); } catch { readiness = {agents: []}; }
  try { skills = await (await fetch("/api/skills")).json(); } catch { skills = []; }
  updateAgentOptions('');
  if (!cur()) state.current = state.convs[0]?.id || null;
  if (!cur()) await newConv(); else { $("input").value = drafts[state.current] || ""; autosize(); render(true); }
  resumePendingRuns();
  refreshPlanBinding(cur()).catch(error => showPlanError(error.message));
  fetchQuotaState();
})();

// Poll only in-flight reads. An expired provider window gets at most two
// automatic retries, one minute apart. A changed reset timestamp is a new
// window; manual refresh remains available after the automatic limit.
setInterval(() => {
  if (quotaData.agents.some(q => q.reading)) fetchQuotaState();
  const now = Date.now();
  quotaData.agents.forEach(q => {
    if (!q.refreshable || q.reading) return;
    let attempts = quotaExpiryAttempts.get(q.agent);
    if (!attempts) { attempts = new Map(); quotaExpiryAttempts.set(q.agent, attempts); }
    if (ChatQuotas.shouldRetryExpired(q, attempts, now)) refreshQuotas(q.agent, true);
  });
}, 1000);
setInterval(() => { if (!$("quotas").hidden) renderQuotas(); }, 60000);

/* ---------- shared plans ---------- */
const planRecords = new Map();
const planErrors = new Map();
let planEdit = null;
const planCheckout = c => c.checkout || serverCheckout || meta.repo;
const planRecordKey = (checkout, id) => JSON.stringify([checkout, id]);
const conversationPlanRoute = (c, action = 'plan') => '/api/conversations/' + encodeURIComponent(c.id) + '/' + action;
const planRoute = (id, action = '') => '/api/plans/' + encodeURIComponent(id) + (action ? '/' + action : '');
const planFor = c => c?.planBinding?.plan_id ? planRecords.get(planRecordKey(planCheckout(c), c.planBinding.plan_id)) : null;
const planQuery = (c, extra = {}) => '?' + new URLSearchParams({checkout: planCheckout(c), ...extra});
function planRunFor(c) {
  if (!c.planBinding?.plan_id) return null;
  return {binding_version: c.planBinding.revision, intent: 'discussion'};
}
function hydratePlanBinding(c, binding) {
  c.plan_binding = binding;
  c.planBinding = {plan_id: binding.plan_slug, revision: binding.version};
  if (binding.origin) c.planOrigin = {...binding.origin, previous_plan_id: binding.origin.previous_plan};
}
async function planRequest(route, body) {
  const response = await fetch(route, body === undefined ? {} : {
    method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body),
  });
  if (!response.ok) {
    const message = await response.text();
    const error = new Error(response.status === 409
      ? 'The plan changed. Refresh before trying again. ' + message
      : response.status === 404 ? (message || 'Plan or conversation not found.') + ' Refresh plan data. Restart aic if this server predates plan support.'
      : message || 'Could not reach the plan service.');
    error.status = response.status; throw error;
  }
  return response.json();
}
function cachePlan(plan) {
  if (!plan || typeof plan.slug !== 'string' || !Number.isSafeInteger(plan.version)) {
    throw new Error('The server returned an invalid plan. Your changes were not applied.');
  }
  const record = {...plan, id: plan.slug, revision: plan.version};
  const cached = planRecords.get(planRecordKey(record.checkout, record.id));
  if (cached && cached.revision > record.revision) return cached;
  planRecords.set(planRecordKey(record.checkout, record.id), record);
  return record;
}
async function refreshPlanBinding(c, {verify = true} = {}) {
  if (!c) return null;
  let result, bindingFound = false;
  try {
    const binding = await planRequest(conversationPlanRoute(c) + planQuery(c));
    bindingFound = true;
    const plan = binding.plan_slug ? await planRequest(planRoute(binding.plan_slug) + planQuery({...c, checkout: binding.checkout})) : null;
    result = {binding, plan};
  }
  catch (error) {
    if (error.status === 404 && !bindingFound && !c.planBinding?.plan_id) return null; // Older server, free chat.
    planErrors.set(c.id, error.message);
    if (cur() === c) renderPlanContext();
    throw error;
  }
  if (!result.binding || !Number.isSafeInteger(result.binding.version)) throw new Error('Invalid conversation plan binding.');
  if (c.planBinding?.revision > result.binding.version) {
    if (verify) throw new Error('The plan changed while refreshing. Try again; your draft is preserved.');
    return null;
  }
  if (verify && c.planBinding && (c.planBinding.plan_id !== result.binding.plan_slug || result.binding.checkout !== planCheckout(c))) {
    throw new Error('The linked plan changed in another window. Reload this conversation before sending. Your draft is preserved.');
  }
  if (result.plan) cachePlan(result.plan);
  const changed = JSON.stringify(c.plan_binding) !== JSON.stringify(result.binding);
  hydratePlanBinding(c, result.binding);
  planErrors.delete(c.id);
  if (changed) saveSoon();
  if (cur() === c) renderPlanContext();
  return result;
}
function showPlanError(message) {
  const c = cur(); if (c) planErrors.set(c.id, message);
  renderPlanContext();
}
function renderPlanContext() {
  const c = cur(), linked = !!c?.planBinding?.plan_id, plan = planFor(c);
  $('planlink').hidden = !c;
  $('planlabel').textContent = linked ? (plan?.title || c.planBinding.plan_id) : 'Free conversation';
  $('planlink').title = linked ? 'Change plan' : 'Link or create plan';
  $('planlink').setAttribute('aria-label', $('planlink').title + ': ' + $('planlabel').textContent);
  $('planlink').disabled = !c || busy(c);
  const origin = c?.planOrigin;
  $('planorigin').hidden = !origin;
  if (origin) {
    const source = state.convs.find(conversation => conversation.id === origin.conversation_id);
    $('forkconversation').textContent = source?.title || 'Original conversation';
    $('forkconversation').title = origin.conversation_id || '';
    $('forkplan').textContent = origin.previous_plan_id || 'Free conversation';
    $('forkmessages').textContent = origin.message_count + (origin.message_count === 1 ? ' message' : ' messages');
  } else {
    $('planorigin').open = false;
    $('forkconversation').textContent = $('forkplan').textContent = $('forkmessages').textContent = '';
    $('forkconversation').removeAttribute('title');
  }
  if (c && planErrors.has(c.id)) {
    $('planlabel').textContent += ' · unable to verify';
    $('planlabel').title = planErrors.get(c.id);
  } else $('planlabel').title = '';
}
async function flushChatForPlanMutation() {
  if (!remoteReady) throw new Error('Chat storage is unavailable. Reconnect before changing plans.');
  if (remoteConflict) throw new Error('This conversation changed in another window. Reload before continuing. Your draft is preserved.');
  const c = cur(); if (c) drafts[c.id] = $('input').value;
  clearTimeout(save.t); save.t = 0; clearTimeout(saveRemoteSoon.t); clearTimeout(saveDrafts.t); saveDrafts.t = 0;
  const until = Date.now() + 10000;
  while (remoteSaving) {
    if (Date.now() > until) throw new Error('Chat is still saving. Try again shortly.');
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  storeItem(KEY, JSON.stringify(state)); storeItem(DRAFTS_KEY, JSON.stringify(drafts));
  remoteDirty = true;
  if (!(await saveRemote())) throw new Error('Could not save the source conversation. Your draft is preserved; try again.');
}
function verifiedPlanFork(result) {
  const fork = result.conversation;
  if (!result.forked || !fork || fork.id !== result.binding?.conversation_id || !Array.isArray(fork.log) || !Array.isArray(fork.participants)
    || !Number.isSafeInteger(result.binding.version)) throw new Error('The server did not confirm the fork. Reload before trying again.');
  hydratePlanBinding(fork, result.binding);
  for (const participant of fork.participants) { participant.session = ''; participant.seen = fork.log.length; }
  return fork;
}
async function undoPlanFork(fork, source) {
  if (busy(fork)) { toast('Replies have started. The fork is preserved. Returning to the original conversation.'); switchTo(source.id); return; }
  try {
    await flushChatForPlanMutation();
    const result = await planRequest(conversationPlanRoute(fork, 'plan/undo'), {checkout: planCheckout(fork), version: fork.planBinding.revision, state_revision: remoteRevision});
    if (!Number.isSafeInteger(result.revision)) throw new Error('The server did not confirm undo. Refresh before trying again.');
    remoteRevision = result.revision;
    if (result.undone) { state.convs = state.convs.filter(c => c !== fork); delete drafts[fork.id]; }
    switchTo(result.conversation_id);
    toast(result.undone ? 'Plan change undone. Original conversation restored.' : 'The fork has been used and is preserved. Returned to the original conversation.');
  } catch (error) { toast(error.message + ' The fork is preserved.'); }
}
function adoptPlanMutation(result, c, source = null) {
  if (Number.isSafeInteger(result.revision)) remoteRevision = result.revision;
  hydratePlanBinding(c, result.binding);
  if (c.planBinding.plan_id) c.keepEmpty = true;
  if (source) {
    state.convs.unshift(c); switchTo(c.id);
    // Once any new turn has started, this offers navigation only; the fork is
    // preserved instead of being deleted by a toast action.
    toast('Fork created. Sessions restarted.', () => undoPlanFork(c, source));
  } else if (!state.convs.includes(c)) {
    state.convs.unshift(c); switchTo(c.id); toast('Fork created. Sessions restarted.');
  } else {
    for (const p of c.participants) { p.session = ''; p.seen = 0; }
    save(); render(); toast(c.planBinding.plan_id ? 'Plan linked to this conversation.' : 'This is now a free conversation.');
  }
  refreshPlanBinding(c).catch(error => { if (cur() === c) showPlanError(error.message); });
}
async function openPlanDialog(c = cur(), {targetCheckout = null} = {}) {
  if (!c || busy(c)) { toast('Wait for replies to finish before changing plans.'); return; }
  const editor = {c, targetCheckout, plans: [], loading: true, saving: false}; planEdit = editor;
  $('planlinkerror').hidden = true;
  for (const id of ['newplantitle', 'newplanslug', 'newplanobjective', 'newplancriteria']) $(id).value = '';
  $('plancheckoutnote').hidden = !targetCheckout;
  $('plancheckoutnote').textContent = targetCheckout ? 'Checkout will change to ' + targetCheckout + '. Detach the current plan or select a plan in that checkout. Sessions restart; files are not migrated.' : '';
  $('planmigrateoption').hidden = !targetCheckout || !c.planBinding?.plan_id;
  $('planmigrateoption').disabled = !targetCheckout || !c.planBinding?.plan_id;
  $('migrateplanslug').value = c.planBinding?.plan_id || '';
  $('planchoice').value = c.planBinding?.plan_id && !targetCheckout ? 'existing' : 'free';
  updatePlanChoice(); $('plandialog').showModal();
  try {
    await flushChatForPlanMutation();
    const [binding, listing] = await Promise.all([
      refreshPlanBinding(c, {verify: true}),
      planRequest('/api/plans' + planQuery(targetCheckout ? {...c, checkout: targetCheckout} : c)),
    ]);
    if (planEdit !== editor || !$('plandialog').open) return;
    if (!binding) throw new Error('Plan features require the updated aic server.');
    editor.binding = binding.binding;
    if (!Array.isArray(listing.plans)) throw new Error('Invalid plan list received from the server.');
    editor.plans = listing.plans;
    const select = $('existingplan'); select.replaceChildren();
    const placeholder = el('option', '', listing.plans.length ? 'Choose a plan' : 'No plans in this checkout'); placeholder.value = ''; select.append(placeholder);
    for (const plan of listing.plans) { const option = el('option', '', plan.title + ' · ' + plan.slug); option.value = plan.slug; select.append(option); }
    select.value = targetCheckout ? '' : c.planBinding?.plan_id || '';
    editor.loading = false; updatePlanChoice();
  } catch (error) {
    if (planEdit === editor) { $('planlinkerror').textContent = error.message; $('planlinkerror').hidden = false; }
  }
}
function updatePlanChoice() {
  const editor = planEdit, choice = $('planchoice').value;
  $('existingplanfield').hidden = choice !== 'existing'; $('createplanfields').hidden = choice !== 'create';
  $('migrateplanfield').hidden = choice !== 'migrate';
  $('migrateplanslug').required = choice === 'migrate'; $('migrateplanslug').disabled = choice !== 'migrate' || !!editor?.saving;
  if (editor?.targetCheckout) $('plancheckoutnote').textContent = choice === 'migrate'
    ? 'Copy the plan to ' + editor.targetCheckout + '. The original plan stays in its checkout.'
    : 'Checkout will change to ' + editor.targetCheckout + '. Detach the current plan or select a plan in that checkout. Sessions restart; files are not migrated.';
  for (const id of ['newplantitle', 'newplanslug', 'newplanobjective']) { $(id).required = choice === 'create'; $(id).disabled = choice !== 'create' || !!editor?.saving; }
  $('newplancriteria').disabled = choice !== 'create' || !!editor?.saving;
  const history = !!editor?.c.log.length;
  $('planchangeeffect').textContent = history
    ? 'Changing the plan creates a fork. History and participants are preserved; agent sessions restart.'
    : 'This changes the plan of this empty conversation. No plan files are copied.';
  $('planapply').textContent = history ? 'Create Fork and Change Plan' : 'Apply Plan';
  const same = editor && !editor.targetCheckout && ((choice === 'free' && !editor.c.planBinding?.plan_id)
    || (choice === 'existing' && $('existingplan').value === editor.c.planBinding?.plan_id));
  $('planapply').disabled = !editor || editor.loading || editor.saving || same || (choice === 'existing' && !$('existingplan').value);
  $('planchoice').disabled = !!editor?.saving; $('existingplan').disabled = !!editor?.saving; $('plancancel').disabled = !!editor?.saving;
}
async function preparePlanSend(c) {
  try {
    if (remoteConflict) throw new Error('This conversation changed in another window. Reload before continuing.');
    const result = await refreshPlanBinding(c, {verify: true});
    if (cur() !== c) return false;
    if (!result || !c.planBinding?.plan_id) return true;
    if (!planFor(c)) throw new Error('Plan details are unavailable. Refresh before sending.');
    await flushChatForPlanMutation();
    return true;
  } catch (error) {
    if (cur() === c) { showPlanError(error.message); toast(error.message + ' Your draft is preserved.'); }
    return false;
  }
}
$('planlink').onclick = () => openPlanDialog();
$('conversationadvanced').ontoggle = () => { if ($('conversationadvanced').open) $('planorigin').open = false; };
$('planorigin').ontoggle = () => { if ($('planorigin').open) $('conversationadvanced').open = false; };
$('advancedclose').onclick = () => { $('conversationadvanced').open = false; $('conversationadvanced').querySelector('summary').focus(); };
$('historyclose').onclick = () => { $('planorigin').open = false; $('planorigin').querySelector('summary').focus(); };
document.addEventListener('click', event => {
  for (const details of [$('conversationadvanced'), $('planorigin')]) {
    if (details.open && !details.contains(event.target)) details.open = false;
  }
});
$('planchoice').onchange = updatePlanChoice; $('existingplan').onchange = updatePlanChoice;
$('newplantitle').oninput = () => { if (!$('newplanslug').dataset.edited) $('newplanslug').value = $('newplantitle').value.toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 80); };
$('newplanslug').oninput = () => { $('newplanslug').dataset.edited = 'true'; };
$('plancancel').onclick = () => $('plandialog').close();
$('plandialog').onclick = event => {
  if (event.target === event.currentTarget && !planEdit?.saving) $('plandialog').close();
};
$('plandialog').addEventListener('close', () => { planEdit = null; delete $('newplanslug').dataset.edited; });
$('plandialog').addEventListener('cancel', event => { if (planEdit?.saving) event.preventDefault(); });
$('planform').onsubmit = async event => {
  event.preventDefault(); const editor = planEdit;
  if (!editor || editor.loading || editor.saving || busy(editor.c)) return;
  editor.saving = true; updatePlanChoice(); $('planlinkerror').hidden = true;
  try {
    if (cur() !== editor.c) throw new Error('The current conversation changed. Reopen the plan selector.');
    await flushChatForPlanMutation();
    let id = $('planchoice').value === 'existing' ? $('existingplan').value : $('planchoice').value === 'migrate' ? $('migrateplanslug').value.trim() : null;
    if ($('planchoice').value === 'create') {
      const created = cachePlan(await planRequest('/api/plans', {
        checkout: editor.targetCheckout || planCheckout(editor.c), slug: $('newplanslug').value.trim(),
        title: $('newplantitle').value.trim(),
        content: '# ' + $('newplantitle').value.trim() + '\n\n## Objective and scope\n\n' + $('newplanobjective').value.trim() + '\n\n## Acceptance criteria\n\n' + $('newplancriteria').value.trim() + '\n',
      }));
      id = created.id;
      // A failed subsequent link can be retried without creating a duplicate.
      const option = el('option', '', created.title + ' · ' + created.slug); option.value = id; $('existingplan').append(option);
      $('planchoice').value = 'existing'; $('existingplan').value = id;
    }
    const result = await planRequest(conversationPlanRoute(editor.c, editor.targetCheckout ? 'checkout' : 'plan'), {
      checkout: editor.targetCheckout || planCheckout(editor.c), plan_slug: id,
      version: editor.binding.version, state_revision: remoteRevision, confirm_fork: !!editor.c.log.length,
      ...(editor.targetCheckout ? {plan_action: $('planchoice').value === 'migrate' ? 'migrate' : id ? 'select' : 'unlink'} : {}),
    });
    if (!result.binding || !Number.isSafeInteger(result.binding.version)) throw new Error('The server did not confirm the plan link. Reload before trying again.');
    const next = result.forked ? verifiedPlanFork(result) : editor.c;
    if (editor.targetCheckout) { next.checkout = editor.targetCheckout; next.gitOrigin = null; next.gitAcknowledged = null; next.gitCurrent = null; }
    $('plandialog').close();
    adoptPlanMutation(result, next, result.forked ? editor.c : null);
    if (editor.targetCheckout) {
      await refreshReadiness(next, {full: true, catalog: true});
      if (gitContext) next.gitOrigin = structuredClone(gitContext); saveSoon(); render();
    }
  } catch (error) { $('planlinkerror').textContent = error.message; $('planlinkerror').hidden = false; }
  finally { editor.saving = false; if (planEdit === editor) updatePlanChoice(); }
};
/* ---------- onboarding and contextual preflight ---------- */
async function refreshReadiness(conversation = cur(), { full = false, catalog = false, targets = null } = {}) {
  const query = conversation?.checkout ? '?checkout=' + encodeURIComponent(conversation.checkout) : '';
  // Agent/skill catalogs change with checkout or configuration, not every send.
  if (catalog || (conversation?.checkout || serverCheckout) !== meta.repo) {
    const [agents, catalogResponse] = await Promise.all([fetch('/api/agents' + query), fetch('/api/skills' + query)]);
    const nextMeta = agents.ok ? await agents.json() : null;
    const nextSkills = catalogResponse.ok ? await catalogResponse.json() : [];
    if (cur() !== conversation) return null;
    if (nextMeta) meta = Object.assign(meta, nextMeta);
    skills = nextSkills;
  }
  const response = full ? await fetch('/api/diagnostics' + query) : await fetch('/api/chat/preflight', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ checkout: conversation?.checkout || null, agents: targets ? uniq(targets.map(p => p.agent)) : meta.agents }),
  });
  if (!response.ok) throw new Error('Environment status unavailable. Run aic doctor in the terminal.');
  const report = await response.json();
  if (cur() !== conversation) return report;
  // A targeted send must not erase other agents' known state.
  if (targets && readiness) {
    const updated = new Map(report.agents.map(a => [a.agent, a]));
    readiness = { ...report, agents: readiness.agents.map(a => updated.get(a.agent) || a) };
  } else readiness = report;
  gitContext = report.git; meta.default = report.default_agent;
  renderSidebar(); renderComposer(); renderEnvironmentStatus();
  if ($('pdialog').open) { updateAgentOptions(); updateParticipantReadiness(); }
  return report;
}
function readinessCommand(parent, text) {
  if (!text) return;
  const row = el('div', 'readycommand'); const code = el('code', '', text);
  const copy = el('button', '', 'Copy'); copy.type = 'button';
  copy.onclick = () => copyText(text, copy); row.append(code, copy); parent.appendChild(row);
}
function readinessDot(state) {
  const dot = el('span', 'readinessdot'), label = cap(state || 'unknown');
  dot.title = label; dot.setAttribute('role', 'img'); dot.setAttribute('aria-label', 'Status: ' + label); return dot;
}
const readinessValue = value => value === 'ready_sandbox' ? 'ready' : value;
function renderEnvironmentStatus() {
  const status = ChatOnboarding.environment(readiness, meta.default, cur()?.participants || []);
  const description = status.state === 'unknown' ? 'Environment status unknown' : status.state === 'blocked' ? 'Environment blocked' : status.pending ? 'Environment: ' + status.pending + (status.pending === 1 ? ' pending item' : ' pending items') : 'Environment ready';
  for (const button of [$('readybtn'), $('penvironment')]) {
    button.dataset.state = status.state;
    button.textContent = 'Environment';
    button.setAttribute('aria-label', description);
    button.title = description;
  }
}
function renderReadiness() {
  const rows = $('readyrows'); rows.textContent = '';
  const overall = ChatOnboarding.environment(readiness, meta.default, cur()?.participants || []);
  $('readystate').className = 'ready-' + overall.state;
  $('readystate').textContent = overall.state === 'ready' ? 'Ready' : overall.state === 'blocked' ? 'Blocked'
    : overall.pending ? overall.pending + (overall.pending === 1 ? ' pending item' : ' pending items') : 'Unknown';
  const workspace = el('section', 'readysection'); workspace.appendChild(el('h3', '', 'Workspace'));
  const workspaceGrid = el('div', 'readygrid'); workspace.appendChild(workspaceGrid); rows.appendChild(workspace);
  for (const [label, check] of [['Repository', readiness?.repository], ['Runtime', readiness?.runtime], ['Sandbox', readiness?.image]]) {
    const state = check?.state || 'unknown', item = el('article', 'readycard ready-' + state);
    const head = el('div', 'readycardhead'); head.append(readinessDot(state), el('strong', '', label)); item.append(head, el('p', 'note', check?.reason || 'Not checked'));
    readinessCommand(item, check?.action); workspaceGrid.append(item);
  }
  const selection = ChatOnboarding.selection(readiness, meta.default);
  $('readyreason').textContent = selection.reason === 'preferred_unavailable' ? 'Your preferred agent (' + meta.default + ') is unavailable. Recover it in the terminal, or choose another ready agent with Add participant. The global preference stays unchanged.' : selection.reason === 'none_ready' ? 'No agents are ready. Install, sign in and test an agent in the terminal, then refresh.' : 'Environment status for this checkout.';
  const agents = el('section', 'readysection'); agents.appendChild(el('h3', '', 'Agents'));
  const agentGrid = el('div', 'readygrid readyagents'); agents.appendChild(agentGrid); rows.appendChild(agents);
  for (const a of readiness?.agents || []) {
    const card = el('article', 'readycard ready-' + a.readiness), head = el('div', 'readycardhead');
    head.append(readinessDot(a.readiness), el('strong', '', cap(a.agent))); card.appendChild(head);
    const fields = el('dl'); for (const key of ['installation', 'authentication', 'execution']) { const field = el('div'), value = readinessValue(String(a[key] || 'unknown')); field.append(el('dt', '', cap(key)), el('dd', '', cap(value))); fields.append(field); } card.append(fields);
    card.append(el('p', 'note', a.reason), el('p', 'note', 'Last test: ' + (a.tested_at ? new Date(a.tested_at * 1000).toLocaleString() : 'never') + ' · ' + a.context));
    readinessCommand(card, a.action); agentGrid.append(card);
  }
  renderEnvironmentStatus();
}
async function openReadiness() {
  environmentFocus = document.activeElement; $('readyerror').hidden = true; $('readyreason').textContent = 'Checking environment…';
  $('readyrows').textContent = ''; $('readyrefresh').disabled = true;
  if (!$('readydialog').open) $('readydialog').showModal();
  try { await refreshReadiness(cur(), { full: true, catalog: true }); renderReadiness(); }
  catch (error) { $('readyerror').textContent = error.message; $('readyerror').hidden = false; }
  finally { $('readyrefresh').disabled = false; }
}
$('readybtn').onclick = () => openReadiness();
$('penvironment').onclick = () => openReadiness();
$('readyclose').onclick = () => $('readydialog').close();
$('readydialog').addEventListener('click', event => {
  if (event.target === event.currentTarget) $('readydialog').close();
});
$('readydialog').addEventListener('close', () => { (environmentFocus?.isConnected ? environmentFocus : $('readybtn')).focus(); });
$('readyrefresh').onclick = async () => { $('readyrefresh').disabled = true; $('readyerror').hidden = true; try { await refreshReadiness(cur(), { full: true, catalog: true }); renderReadiness(); } catch (e) { $('readyerror').textContent = e.message; $('readyerror').hidden = false; } finally { $('readyrefresh').disabled = false; } };
function updateParticipantReadiness() {
  const agent = $('pagent').value;
  $('psave').disabled = (!editing || editing.agent !== agent) && !ChatOnboarding.ready(readiness, agent);
}
$('pagent').addEventListener('change', updateParticipantReadiness);
function renderAgentVisibility() {
  const rows = $('agentvisibilityrows'); rows.textContent = '';
  if (!settingsDraft) return;
  const defaults = new Set([meta.default, $('cdefault').value].filter(Boolean));
  for (const name of meta.agents) {
    const label = el('label', 'check configcheck visibilityoption'), box = document.createElement('input'); box.type = 'checkbox';
    if (defaults.has(name)) settingsDraft.hiddenAgents.delete(name);
    box.checked = !settingsDraft.hiddenAgents.has(name);
    box.disabled = defaults.has(name);
    box.onchange = () => {
      if (defaults.has(name)) return;
      if (box.checked) settingsDraft.hiddenAgents.delete(name); else settingsDraft.hiddenAgents.add(name);
    };
    const copy = el('span', 'checkcopy'), title = el('strong', '', cap(name));
    copy.append(title);
    if (defaults.has(name)) copy.append(el('small', '', 'Default · always visible'));
    label.append(box, copy); rows.append(label);
  }
}
async function prepareSend(c, targets) {
  try {
    const report = await refreshReadiness(c, { targets });
    if (cur() !== c) return false;
    const context = report.git;
    const blocked = targets.filter(p => !ChatOnboarding.ready(report, p.agent));
    if (blocked.length) { toast('Unavailable: ' + blocked.map(p => p.name).join(', ') + '. Your draft is preserved.'); await openReadiness(); return false; }
    if (!context) { toast('Git context unavailable. Check the checkout in the terminal.'); return false; }
    // Old conversations gain an origin without rewriting their sessions/history.
    if (!c.gitOrigin) { c.gitOrigin = structuredClone(context); saveSoon(); }
    if (ChatOnboarding.branchChanged(c.gitOrigin, context, c.gitAcknowledged)) {
      $('branchtext').textContent = 'Branch changed from ' + c.gitOrigin.label + ' to ' + context.label + '.';
      $('branchorigin').textContent = c.gitOrigin.label || c.gitOrigin.branch || (c.gitOrigin.sha ? c.gitOrigin.sha.slice(0, 8) : 'Unknown');
      $('branchcurrent').textContent = context.label || context.branch || (context.sha ? context.sha.slice(0, 8) : 'Unknown');
      $('branchcheckout').textContent = context.checkout;
      const approved = await new Promise(resolve => {
        const dialog = $('branchdialog');
        const end = value => { dialog.close(); resolve(value); };
        $('branchcontinue').onclick = () => { c.gitAcknowledged = ChatOnboarding.gitKey(context); saveSoon(); end(true); };
        $('branchnew').onclick = () => { end(false); newConv(); };
        $('branchcancel').onclick = () => end(false);
        dialog.onclick = event => { if (event.target === dialog) end(false); };
        dialog.oncancel = () => resolve(false); dialog.showModal();
      });
      if (!approved) return false;
    }
    if (cur() !== c) return false;
    c.gitCurrent = structuredClone(context);
    return await preparePlanSend(c);
  } catch (e) { toast(e.message + ' Your draft is preserved.'); return false; }
}

$('worktreebtn').onclick = async () => {
  const c = cur(); if (!c || busy(c)) { toast('Wait for replies to finish before changing checkout.'); return; }
  $('worktreeerror').hidden = true;
  try {
    const response = await fetch('/api/worktrees'); if (!response.ok) throw new Error('Worktree list unavailable');
    const trees = await response.json(); const select = $('worktreeselect'); select.textContent = '';
    for (const tree of trees) { const option = el('option', '', tree.label + ' · ' + tree.checkout); option.value = tree.checkout; select.append(option); }
    select.value = c.checkout || meta.repo; $('conversationadvanced').open = false; $('worktreedialog').showModal();
  } catch (e) { toast(e.message); }
};
$('worktreecancel').onclick = () => $('worktreedialog').close();
$('worktreedialog').onclick = event => {
  if (event.target === event.currentTarget) $('worktreedialog').close();
};
$('worktreeconfirm').onclick = async () => {
  const c = cur(); if (!c || busy(c)) return;
  if ((c.planBinding?.plan_id || c.planBinding?.revision > 0) && $('worktreeselect').value !== (c.checkout || meta.repo)) {
    const targetCheckout = $('worktreeselect').value;
    $('worktreedialog').close();
    await openPlanDialog(c, { targetCheckout });
    return;
  }
  c.checkout = $('worktreeselect').value;
  for (const p of c.participants) { p.session = ''; p.seen = 0; }
  c.gitOrigin = null; c.gitAcknowledged = null; saveSoon(); $('worktreedialog').close();
  await refreshReadiness(c, { full: true, catalog: true }); c.gitOrigin = structuredClone(gitContext); saveSoon(); render();
};

function updateAgentOptions(selected = $('pagent').value) {
  const select = $('pagent'); select.textContent = '';
  const placeholder = el('option', '', 'Choose a ready agent'); placeholder.value = ''; select.append(placeholder);
  for (const a of meta.agents.filter(a => ChatOnboarding.ready(readiness, a)
    && (a === meta.default || !(state.hiddenAgents || []).includes(a) || a === selected || a === editing?.agent))) {
    const option = el('option', '', cap(a)); option.value = a; select.append(option);
  }
  select.value = [...select.options].some(option => option.value === selected) ? selected : '';
}
