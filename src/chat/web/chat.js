"use strict";
/* aic chat — the page script for chat.html. Model text only ever reaches the
   DOM through textContent / createTextNode, never innerHTML. */

const $ = (id) => document.getElementById(id);
const KEY = "aic-chat-v2";
const DRAFTS_KEY = "aic-chat-drafts";
let state = { convs: [], current: null, layout: "auto" };
let drafts = {}; // conversation id -> unsent text, saved apart from the (big) state
let meta = { agents: [], details: {}, default: "", repo: "", sandboxed: false };
let readiness = null, gitContext = null, pendingSelection = null;
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
  edit: I('<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>'),
  close: I('<path d="M6 6l12 12M18 6L6 18"/>'),
  menu: I('<path d="M4 7h16M4 12h16M4 17h16"/>'),
  plus: I('<path d="M12 5v14M5 12h14"/>'),
  down: I('<path d="M12 5v14M6 13l6 6 6-6"/>'),
  send: I('<path d="M12 19V5M6 11l6-6 6 6"/>'),
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

function toast(text, undo) {
  $("toasttext").textContent = text;
  $("undo").hidden = !undo;
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
function loadLocal() {
  try { state = Object.assign(state, JSON.parse(localStorage.getItem(KEY)) || {}); } catch {}
  try { drafts = JSON.parse(localStorage.getItem(DRAFTS_KEY)) || {}; } catch {}
  normalizeState();
}
// Run this after loading either cache or SQLite. In particular, an agent may
// have been streaming when the browser closed, so no stored entry may remain
// permanently pending after a restart.
function normalizeState() {
  if (!Array.isArray(state.convs)) state.convs = [];
  if (state.stack) state.layout = "stack";
  delete state.stack;
  state.convs.forEach((c) => {
    if (c.draft && !drafts[c.id]) drafts[c.id] = c.draft;
    delete c.draft;
    c.log.forEach((e) => {
      if (e.pending) { e.pending = false; if (!e.text) { e.error = true; e.soft = true; e.text = "interrupted"; } }
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
  storeItem(DRAFTS_KEY, JSON.stringify(drafts));
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
  if (!remoteReady || remoteSaving || !remoteDirty) return;
  remoteSaving = true; remoteDirty = false;
  try {
    const res = await fetch("/api/chat/state", {
      method: "POST", headers: { "Content-Type": "application/json" },
      // `keepalive` limits request bodies to roughly 64 KiB in browsers. A
      // conversation snapshot can be much larger, so normal saves must use a
      // regular fetch; localStorage remains the abrupt-close fallback.
      body: JSON.stringify({ state, drafts }),
    });
    if (!res.ok) throw new Error(await res.text());
    $("banner").hidden = true;
  } catch (err) {
    remoteError(err);
  } finally {
    remoteSaving = false;
    if (remoteDirty) saveRemote();
  }
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
const hasInteraction = (c) => !!c.keepEmpty || (Array.isArray(c.log) && c.log.length > 0);
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
// Eight well-separated hues, in the order new participants get them.
const PALETTE = [210, 25, 135, 290, 350, 180, 55, 250];
function hashHue(name) { let h = 0; for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) % 360; return h; }
const hueOf = (p) => (typeof p.hue === "number" ? p.hue : hashHue(p.name)); // old conversations: hash
function hueFor(c, name) { const p = c.participants.find((x) => x.name === name); return p ? hueOf(p) : hashHue(name); }
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
  a.style.setProperty("--h", hueFor(c, name)); a.setAttribute("aria-hidden", "true");
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
      if (changed) { renderBar(); renderComposer(); }
      renderQuotas();
    } catch {
      quotaManualReads.clear();
      // A transport failure keeps old readings visible without claiming freshness.
      quotaData.agents = meta.agents.map(agent => ({ ...(quotaOf(agent) || { agent, windows: [] }),
        refreshable: meta.details?.[agent]?.kind === "codex", state: "read_error", reading: false, error: "Couldn't reach the quota reader", attempted_at: new Date().toISOString() }));
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
function renderQuotaDialog() {
  const agent = $("pagent").value, q = quotaOf(agent, quotaModel(agent, $("pmodel").value)), note = $("pquota");
  if (!agent) { note.textContent = ""; return; }
  const level = ChatQuotas.level(q), w = ChatQuotas.critical(q);
  note.className = "note" + (level === "out" ? " err" : level === "warn" ? " warn" : "");
  const quota = q?.state === "read_error" ? "Couldn't refresh quota · " + ChatQuotas.updated(q)
    : level === "out" ? `${cap(agent)} is out of quota. Replies may fail · ${ChatQuotas.resetText(w)}`
    : q?.windows?.length ? q.windows.map(w => `${w.label}: ${ChatQuotas.percent(w.used_pct) || "n/a"}`).join(" · ") + " · " + ChatQuotas.updated(q)
    : ChatQuotas.summary(q || { reason: "No quota reading yet" });
  note.textContent = quota + " · This conversation: " + (quotaConversation === (cur()?.id || "") ? ChatQuotas.consumptionText(quotaData.consumption[agent]) : "Loading usage…");
  note.title = q?.source || "";
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
  meta.agents.forEach(agent => {
    const q = quotaOf(agent) || { agent, windows: [], reason: "No quota reading yet" };
    const w = ChatQuotas.critical(q), stale = ChatQuotas.stale(q) || ChatQuotas.expired(q) || q.state === "read_error";
    const row = el("div", "quota " + ChatQuotas.level(q) + (stale ? " stale" : ""));
    const head = el("div", "head"), details = el("div", "quota-details");
    head.appendChild(el("strong", "", cap(agent)));
    if (q.reading && (!q.fetched_at || quotaManualReads.has(agent))) { const dots = el("span", "dots"); dots.setAttribute("aria-label", "Reading quota"); head.appendChild(dots); }
    head.appendChild(el("span", "val", ChatQuotas.summary(q)));
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
    const consumption = el("div", "sub quota-consumption", "This conversation: " + (sameConversation ? ChatQuotas.consumptionText(quotaData.consumption[agent]) : "Loading usage…"));
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
  if ($("pdialog").open) renderQuotaDialog();
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
const defaultAgent = () => (state.hiddenAgents || []).includes(meta.default) ? "" : ChatOnboarding.initial(readiness, meta.default);
async function newConv() {
  try { await refreshReadiness(); } catch { readiness = {agents: []}; }
  const c = { id: crypto.randomUUID(), title: "New conversation", participants: [], log: [], round: 0, updated: Date.now(), sendTo: null, checkout: cur()?.checkout || null, gitOrigin: gitContext ? structuredClone(gitContext) : null };
  const agent = defaultAgent();
  if (agent) addParticipant(c, { agent });
  state.convs.unshift(c);
  $("search").value = "";
  switchTo(c.id);
  if (!agent) openReadiness(c);
}
function copyParticipants(source, seen) {
  // Native CLI sessions cannot be forked. Start fresh sessions in the new
  // conversation, while preserving every participant setting.
  return source.participants.map((p) => ({
    name: p.name, agent: p.agent, role: p.role || "", write: !!p.write,
    model: p.model || "", effort: p.effort || "", hue: hueOf(p), session: "", seen,
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
function copyConversation(source, withHistory) {
  if (withHistory && busy(source)) {
    toast("Wait for replies to finish before forking this conversation.");
    return;
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
function switchTo(id) {
  hist.idx = -1;
  const old = cur();
  if (old && !discardEmptyConversation(old)) { drafts[old.id] = $("input").value; saveDraftsSoon(); }
  state.current = id;
  $("input").value = drafts[id] || ""; autosize();
  document.body.classList.remove("drawer");
  save(); render(true); $("input").focus();
  refreshReadiness(cur()).catch(() => {});
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
  setGenLabel($("rgen"), "Generate title");
  renderRenameCount();
  $("rdialog").showModal();
  $("rinput").focus(); $("rinput").select();
}
$("rinput").oninput = renderRenameCount;
$("rcancel").onclick = () => $("rdialog").close();
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
    setGenLabel(btn, "Generate title");
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
  $("dtext").textContent = `This will delete “${c.title}” and all its messages. You can undo right after.`;
  $("ddialog").showModal();
  $("dcancel").focus();
}
$("dcancel").onclick = () => $("ddialog").close();
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
  $("contextrepo").textContent = gitContext?.repository || name;
  $("contextbranch").textContent = gitContext?.label || "Git context unavailable";
  $("contextcheckout").textContent = gitContext?.checkout || meta.repo; $("contextcheckout").title = gitContext?.checkout || meta.repo;
  $("repobranch").textContent = gitContext?.label || "Git context unavailable"; $("repobranch").title = gitContext?.label || "";
}
function convRow(c) {
  const row = el("div", "conv" + (c.id === state.current ? " active" : ""));
  const open = el("button", "open"); open.type = "button";
  open.appendChild(el("span", "title", c.title));
  const sub = el("span", "sub");
  c.participants.slice(0, 8).forEach((p) => { const d = el("span", "dot"); d.style.setProperty("--h", hueOf(p)); d.title = p.name; sub.appendChild(d); });
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
    chip.title = (p.role ? p.role + "\n\n" : "") + (p.inactive ? p.name + " — inactive. " : "") + "Edit " + p.name;
    if (p.inactive) chip.setAttribute("aria-label", p.name + ", inactive");
    chip.appendChild(avatar(c, p.name));
    if (p.inactive) { const ps = icon("pause"); ps.classList.add("pause"); chip.appendChild(ps); }
    chip.appendChild(el("span", "", p.name));
    chip.appendChild(el("small", "", cap(p.agent) + (p.model ? " · " + cap(p.model) : "")));
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
  return e.dur != null ? `${t} · ${(e.dur / 1000).toFixed(1)}s` : t;
}
function usageText(u) {
  if (!u) return ["", ""];
  const inTok = (u.input_tokens || 0) + (u.cache_read_tokens || 0) + (u.cache_write_tokens || 0);
  const parts = [];
  if (inTok || u.output_tokens) parts.push(`${fmtK(inTok)} in · ${fmtK(u.output_tokens || 0)} out`);
  if (u.cost_usd != null) parts.push("$" + u.cost_usd.toFixed(u.cost_usd < 1 ? 4 : 2));
  const detail = [
    u.input_tokens != null && `input: ${u.input_tokens}`,
    u.cache_read_tokens != null && `cache read: ${u.cache_read_tokens}`,
    u.cache_write_tokens != null && `cache write: ${u.cache_write_tokens}`,
    u.output_tokens != null && `output: ${u.output_tokens}`,
    u.num_turns != null && `agent turns: ${u.num_turns}`,
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
  d.style.setProperty("--h", hueFor(c, e.from));
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
  const changed = (part, key) => { const s = JSON.stringify(key); if (k.keys[part] === s) return false; k.keys[part] = s; return true; };
  k.card.classList.toggle("error", !!e.error && !e.soft);
  k.card.classList.toggle("soft", !!e.error && !!e.soft);

  if (changed("head", [turnAgent, turnModel, p?.write, e.skill, e.pending, hueFor(c, e.from), isExpanded(c, e)])) {
    k.card.style.setProperty("--h", hueFor(c, e.from));
    k.head.textContent = "";
    k.head.append(avatar(c, e.from), el("span", "name", e.from));
    if (turnAgent) k.head.appendChild(el("span", "badge", cap(turnAgent) + (turnModel ? " · " + cap(turnModel) : "")));
    if (p?.write) k.head.appendChild(writeBadge());
    if (e.skill) { const s = el("span", "badge skill", "/" + e.skill); s.title = "ran the skill /" + e.skill; k.head.appendChild(s); }
    k.head.appendChild(el("span", "grow"));
    if (e.pending) k.head.appendChild(iconBtn("stop", "Stop " + e.from, () => running.get(runKey(c, { name: e.from }))?.abort()));
    const on = isExpanded(c, e), ex = iconBtn(on ? "collapse" : "expand", on ? "Collapse reply" : "Expand reply", () => toggleExpand(c, e));
    ex.classList.add("expandbtn"); ex.setAttribute("aria-expanded", String(on));
    k.head.appendChild(ex);
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

function userBubble(c, e) {
  const u = el("div", "user");
  const fill = () => {
    u.textContent = "";
    if (e.to) u.appendChild(el("span", "to", "→ " + e.to.join(", ")));
    const text = el("div", "text");
    const sk = skillOf(e.text);
    if (sk) {
      const s = el("span", "badge skill", "/" + sk); s.title = knownSkill(sk)?.description || "skill";
      text.append(s, document.createTextNode(e.text.slice(sk.length + 1)));
    } else text.textContent = e.text;
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
  const importButton = el("button", "person add");
  importButton.type = "button"; importButton.onclick = chooseImport;
  importButton.append(icon("download"), el("span", "", "Import conversation"), el("small", "", ".json · .md"));
  const people = el("div", "people");
  if (!meta.agents.length) {
    d.append(el("h1", "", "No agents configured"), el("p", "", "Add one with `aic config` and reload this page."));
    people.appendChild(importButton); d.appendChild(people);
    return d;
  }
  if (!c) { people.appendChild(importButton); d.appendChild(people); return d; }
  const group = c.participants.length > 1;
  d.appendChild(el("h1", "", group ? "Talk to several agents at once" : "Start a conversation"));
  d.appendChild(el("p", "", group
    ? "Your message goes to everyone in “To:”. Once they've answered, you can ask them to comment on each other's replies."
    : "Ask anything about this repository. Add a participant (even a second Claude) to compare answers side by side."));
  c.participants.forEach((p) => {
    const b = el("button", "person"); b.type = "button"; b.title = "Edit " + p.name;
    b.style.setProperty("--h", hueOf(p));
    b.append(avatar(c, p.name, true), el("span", "name", p.name));
    const tags = el("span"); tags.appendChild(el("span", "badge", cap(p.agent) + (p.model ? " · " + cap(p.model) : "")));
    if (p.write) { tags.append(" "); tags.appendChild(writeBadge()); }
    b.appendChild(tags);
    if (p.role) b.appendChild(el("span", "role", p.role));
    b.onclick = () => openDialog(p);
    people.appendChild(b);
  });
  const add = el("button", "person add"); add.type = "button";
  add.append(icon("plus"), el("span", "", "Add participant"));
  add.onclick = () => openDialog(null);
  people.append(add, importButton);
  d.appendChild(people);
  const sug = el("div", "suggest");
  SUGGESTIONS.forEach((s) => {
    const b = el("button", "", s); b.type = "button";
    b.onclick = () => { const i = $("input"); i.value = s; autosize(); i.focus(); onInput(); };
    sug.appendChild(b);
  });
  d.appendChild(sug);
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
      b.style.setProperty("--h", hueOf(p));
      b.setAttribute("aria-pressed", String(on));
      b.appendChild(el("span", "dot"));
      b.appendChild(el("span", "", p.name));
      const pm = quotaModel(p.agent, p.model), alert = quotaAlert(p.agent, pm);
      if (alert) { const badge = icon("alert"); badge.classList.add("quota-" + ChatQuotas.level(quotaOf(p.agent, pm))); b.appendChild(badge); b.setAttribute("aria-label", p.name + ": " + alert); }
      const n = unseen(c, p).length;
      if (n && c.log.length) { const cnt = el("span", "count", String(n)); b.appendChild(cnt); }
      b.title = (on ? "Receives your message" : "Doesn't receive your message") + (n ? ` · ${n} new message${n > 1 ? "s" : ""} to read` : "");
      if (alert) b.title += "\n" + alert;
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
  let msg = "Enter sends · Shift+Enter new line · @ mentions · /fork · /clear" + (skills.length ? " · / skills" : ""), warn = false;
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
  renderQuotas();
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
  if (kindOf(p) !== "agy" && skills.length) {
    s += " A user message that starts with /name (for example /aic-plan my-plan) asks you to run the skill `name` with the rest of that line as its arguments."
      + " If a skill asks you to interview the user, ask your questions as plain replies in this chat; there is no question tool here.";
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
    c.log.push({ from: "you", round, text: userText, t0: Date.now(), ...(to ? { to } : {}), ...(opts.edited ? { edited: true } : {}) });
    if (first && !c.renamed) c.title = userText.slice(0, 48);
  }
  const jobs = targets
    .map((p) => ({ p, ...promptFor(c, p, opts), oldSeen: p.seen }))
    .filter((j) => j.text.trim() && unseen(c, j.p, opts.until).length);
  jobs.forEach((j) => {
    if (!j.p.session) j.text = bootstrapPrompt(c, j);
  });
  jobs.forEach((j) => { j.p.seen = opts.until ?? c.log.length; });
  jobs.forEach((j) => {
    j.entry = { from: j.p.name, agent: j.p.agent, model: j.p.model || meta.details?.[j.p.agent]?.model || "", round, text: "", pending: true, t0: Date.now(), seen0: j.oldSeen, actions: [], ...(j.skill ? { skill: j.skill } : {}) };
    c.log.push(j.entry);
    running.set(runKey(c, j.p), new AbortController());
  });
  if (!jobs.length && !userText) toast("Nothing new for them to answer yet.");
  save(); render(true);
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

function stopAll(c) { running.forEach((ctl, k) => { if (k.startsWith(c.id + "\n")) ctl.abort(); }); }

async function turn(c, { p, text: prompt, entry }) {
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
  try {
    const res = await fetch("/api/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        checkout: c.checkout || null, git_context: c.gitCurrent || c.gitOrigin || null, agent: entry.agent, conversation_id: c.id, prompt, session_id: p.session, system: systemFor(c, p), write: !!p.write,
        model: p.model || null, effort: p.effort || null,
      }),
      signal: running.get(key).signal,
    });
    if (!res.ok) {
      if (res.status === 409) { const last = [...c.log].reverse().find(e => e.from === "you"); if (last && !drafts[c.id]) { drafts[c.id] = last.text; if (cur() === c && !$("input").value) { $("input").value = last.text; autosize(); } saveDraftsSoon(); } }
      throw new Error(await res.text());
    }
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = "", gap = false;
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let i;
      while ((i = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, i); buf = buf.slice(i + 1);
        if (!line.trim()) continue; // heartbeat
        const ev = JSON.parse(line);
        if (ev.type === "session") p.session = ev.id;
        else if (ev.type === "activity") {
          entry.activity = ev.text.split(meta.repo + "/").join("");
          gap = true;
          if (entry.actions.length < 500) entry.actions.push(entry.activity);
          paint();
        } else if (ev.type === "delta") {
          // text said before and after a tool call arrives as separate blocks; keep them apart
          if (gap && entry.text && !/\s$/.test(entry.text)) entry.text += "\n\n";
          gap = false; entry.text += ev.text; paint();
        } else if (ev.type === "done") {
          if (ev.usage) entry.usage = ev.usage;
          if (!ev.ok) fail(ev.error || "agent failed");
          else if (!entry.text) entry.text = ev.text;
        }
      }
    }
  } catch (err) {
    if (err.name === "AbortError") { if (!entry.text) fail("Stopped.", true); else entry.stopped = true; }
    else fail(String(err.message || err));
  } finally {
    entry.pending = false; entry.activity = ""; entry.dur = Date.now() - entry.t0;
    running.delete(key);
    c.updated = Date.now(); save();
    if (cur() === c) {
      paintCard(c, entry);
      if (!busy(c)) c.log.forEach((e) => paintCard(c, e)); // regenerate/delete come back
      if (view.lastUser && !busy(c)) view.lastUser._fill();
    }
    render();
    fetchQuotaState(); // Server already refreshes only this turn's agent.
    if (!busy(c)) announce(cur() === c ? "Replies finished." : `Replies finished in “${c.title}”.`);
    else announce(`${p.name} finished.`);
  }
}

/* ---------- input ---------- */
function clearInput(c) { hist.idx = -1; $("input").value = ""; drafts[c.id] = ""; saveDraftsSoon(); c.toFromText = false; autosize(); acClose(); }
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
  const to = targets.length === active(c).length ? null : targets.map((p) => p.name);
  if (!body) { // nothing to add: let them answer what's new to them
    const ready = targets.filter((p) => unseen(c, p).length);
    if (!ready.length) { toast(text ? "Nothing new for them to answer yet." : "Write a message first."); return; }
    if (!(await prepareSend(c, ready))) return;
    clearInput(c); launch(c, ready, null, null, { preflightDone: true }); return;
  }
  const sk = skillOf(body), problem = sk && skillProblem(sk, targets);
  if (problem) { toast(problem); return; }
  if (!(await prepareSend(c, targets))) return;
  clearInput(c);
  launch(c, targets, body, to, { preflightDone: true });
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
    const clear = { type: "/", value: "clear", label: "/clear", sub: "Clear the conversation and reset the context" };
    acItems = [fork, clear, ...skills.filter((s) => s.name.toLowerCase().includes(q))
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
function openDialog(p) {
  const c = cur(); if (!c || !meta.agents.length) return;
  editing = p || null;
  $("ptitle").textContent = p ? "Edit " + p.name : "Add participant";
  $("psave").textContent = p ? "Save" : "Add";
  $("pname").value = p ? p.name : "";
  updateAgentOptions();
  $("pagent").value = p ? p.agent : defaultAgent();
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
  renderQuotaDialog();
  fetchModels(agent).then((live) => { if ($("pagent").value === agent) suggest(live); });
  // A new participant, or an existing one moved to another agent, starts a
  // fresh native session. Explain how its first prompt is seeded.
  $("pagentwarn").hidden = !!editing && editing.agent === agent;
  const w = $("pwritewarn");
  w.hidden = access() !== "write";
  w.replaceChildren(icon("alert"), el("span", "", `Can create, change and delete files in ${meta.repo || "this repository"}, following the permission mode in your config.`));
  const hues = $("phues"); hues.textContent = "";
  const options = PALETTE.includes(dlgHue) ? PALETTE : [...PALETTE, dlgHue];
  const taken = new Set(c ? c.participants.filter((q) => q !== editing).map(hueOf) : []);
  options.forEach((h) => {
    const b = el("button", "hue"); b.type = "button";
    b.style.setProperty("--h", h);
    b.setAttribute("role", "radio"); b.setAttribute("aria-checked", String(h === dlgHue));
    b.setAttribute("aria-label", `Hue ${h}` + (taken.has(h) ? " (used by someone else)" : ""));
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
$("pmodel").onchange = () => { dlgModel = $("pmodel").value; renderQuotaDialog(); }; // the limit that applies depends on the model
$("peffort").onchange = () => { dlgEffort = $("peffort").value; };
$("pagent").onchange = () => { dlgModel = ""; dlgEffort = ""; updateDialog(); }; // each agent has its own models
document.querySelectorAll('input[name="paccess"]').forEach((r) => { r.onchange = updateDialog; });
$("pname").oninput = () => { $("pname").value = $("pname").value.replace(/\s+/g, "-").replace(/[^\w-]/g, ""); };
$("pcancel").onclick = () => $("pdialog").close();
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
    $("cconfirmdefault").checked = false;
    $("cpermission").value = config.permission_mode;
    $("cskippermissions").checked = !!config.skip_permissions;
    $("csandbox").value = config.sandbox;
    $("csandboximage").value = config.sandbox_image;
    $("cdialog").showModal();
  } catch (err) {
    error.textContent = "Couldn't load configuration: " + (err.message || err);
    error.hidden = false;
    $("cdialog").showModal();
  } finally {
    saveBtn.disabled = false;
  }
}
$("ccancel").onclick = () => $("cdialog").close();
$("cform").onsubmit = async (e) => {
  e.preventDefault();
  const error = $("cerror"), saveBtn = $("csave");
  error.hidden = true; saveBtn.disabled = true;
  const payload = {
    default_agent: $("cdefault").value,
    confirm_default: $("cconfirmdefault").checked,
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
    $("cdialog").close();
    render();
    toast("Configuration saved. New agent turns use it.");
  } catch (err) {
    error.textContent = "Couldn't save configuration: " + (err.message || err);
    error.hidden = false;
  } finally {
    saveBtn.disabled = false;
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
$("copyrepo").onclick = () => copyText(meta.repo, $("copyrepo"));
$("addbtn").onclick = () => openDialog(null);
$("configbtn").appendChild(icon("settings"));
$("configbtn").onclick = openConfig;
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
  if (e.ctrlKey && e.shiftKey && (e.key === "O" || e.key === "o")) { e.preventDefault(); newConv(); return; }
  if (e.key !== "Escape" || e.defaultPrevented || $("pdialog").open || $("cdialog").open || $("ddialog").open || $("rdialog").open) return;
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
  } catch (err) {
    $("msgs").textContent = "Couldn't reach the aic chat server: " + err.message; return;
  }
  try {
    const res = await fetch("/api/chat/state");
    if (!res.ok) throw new Error(await res.text());
    const saved = await res.json();
    if (saved.state && typeof saved.state === "object" && Array.isArray(saved.state.convs)) {
      state = Object.assign({ convs: [], current: null, layout: "auto" }, saved.state);
      drafts = saved.drafts && typeof saved.drafts === "object" ? saved.drafts : {};
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
  meta.agents.forEach((a) => { const o = el("option", "", cap(a)); o.value = a; $("pagent").appendChild(o); });
  if (!cur()) state.current = state.convs[0]?.id || null;
  if (!cur()) await newConv(); else { $("input").value = drafts[state.current] || ""; autosize(); render(true); }
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

/* ---------- onboarding and contextual preflight ---------- */
async function refreshReadiness(conversation = cur()) {
  const query = conversation?.checkout ? '?checkout=' + encodeURIComponent(conversation.checkout) : '';
  const response = await fetch('/api/diagnostics' + query);
  if (!response.ok) throw new Error('Readiness unavailable. Run aic doctor in the terminal.');
  const report = await response.json();
  if (cur() !== conversation) return report;
  const agents = await fetch('/api/agents' + query);
  if (cur() !== conversation) return report;
  if (agents.ok) meta = Object.assign(meta, await agents.json());
  try { const catalog = await fetch('/api/skills' + query); if (catalog.ok) skills = await catalog.json(); } catch {}
  if (cur() !== conversation) return report;
  readiness = report; gitContext = report.git; meta.default = report.default_agent;
  renderSidebar(); renderComposer(); return report;
}
function readinessCommand(parent, text) {
  if (!text) return;
  const row = el('p', 'note'); const code = el('code', '', text);
  const copy = el('button', '', 'Copy command'); copy.type = 'button';
  copy.onclick = () => copyText(text, copy); row.append(code, document.createTextNode(' '), copy); parent.appendChild(row);
}
function renderReadiness() {
  const rows = $('readyrows'); rows.textContent = '';
  const summary = el('p', '', 'Repository: ' + (readiness?.repository.state || 'unknown') + ' · Runtime: ' + (readiness?.runtime.state || 'unknown') + ' · Sandbox: ' + (readiness?.image.state || 'unknown'));
  rows.append(summary); readinessCommand(rows, readiness?.image.action || readiness?.runtime.action);
  const selection = ChatOnboarding.selection(readiness, meta.default);
  $('readyreason').textContent = selection.reason === 'preferred_unavailable' ? 'Your preferred agent (' + meta.default + ') is unavailable. Choose a ready agent for this conversation; the global preference stays unchanged.' : selection.reason === 'none_ready' ? 'No agents are ready. Install, sign in and test an agent in the terminal, then refresh.' : 'Confirm the agent for this conversation. A participant is an instance of that agent.';
  for (const a of readiness?.agents || []) {
    const card = el('article'); card.appendChild(el('strong', '', cap(a.agent) + ' · ' + a.readiness));
    const fields = el('dl'); for (const key of ['installation', 'authentication', 'execution', 'quota']) { const field = el('div'); field.append(el('dt', '', cap(key)), el('dd', '', a[key])); fields.append(field); } card.append(fields);
    card.append(el('p', 'note', a.reason), el('p', 'note', 'Last test: ' + (a.tested_at ? new Date(a.tested_at * 1000).toLocaleString() : 'never') + ' · ' + a.context));
    readinessCommand(card, a.action); rows.append(card);
  }
  for (const id of ['readyagent', 'readysecond']) { const select = $(id); const previous = select.value; select.textContent = ''; const placeholder = el('option', '', 'Choose a ready agent'); placeholder.value = ''; select.append(placeholder); for (const name of selection.ready.filter(name => !(state.hiddenAgents || []).includes(name))) { const option = el('option', '', cap(name)); option.value = name; select.append(option); } select.value = selection.ready.includes(previous) ? previous : ''; }
  const visibility = $('agentvisibilityrows'); visibility.textContent = '';
  for (const a of readiness?.agents || []) { const label = el('label', 'check'); const box = document.createElement('input'); box.type = 'checkbox'; box.checked = !(state.hiddenAgents || []).includes(a.agent); box.onchange = () => { const hidden = new Set(state.hiddenAgents || []); if (box.checked) hidden.delete(a.agent); else hidden.add(a.agent); state.hiddenAgents = [...hidden]; saveSoon(); updateAgentOptions(); renderReadiness(); }; label.append(box, document.createTextNode(cap(a.agent))); visibility.append(label); }
  $('readycontinue').disabled = !selection.ready.length;
}
async function openReadiness(conversation = null) {
  pendingSelection = conversation; $('readyerror').hidden = true; $('readycontinue').disabled = true; $('readyreason').textContent = 'Checking environment…';
  if (!$('readydialog').open) $('readydialog').showModal();
  try { await refreshReadiness(); renderReadiness(); }
  catch (error) { $('readyerror').textContent = error.message; $('readyerror').hidden = false; $('readycontinue').disabled = true; }
}
$('readybtn').onclick = () => openReadiness(cur()?.participants.length ? null : cur());
$('readyclose').onclick = () => { $('readydialog').close(); $('readybtn').focus(); };
$('readyrefresh').onclick = async () => { $('readyrefresh').disabled = true; try { await refreshReadiness(); renderReadiness(); } catch (e) { $('readyerror').textContent = e.message; $('readyerror').hidden = false; } finally { $('readyrefresh').disabled = false; } };
$('readymode').onchange = () => { $('readysecondlabel').hidden = $('readymode').value === 'individual'; };
$('newoptions').onclick = async () => { await newConv(); openReadiness(cur()); };
$('readycontinue').onclick = async () => {
  await refreshReadiness(); const name = $('readyagent').value, second = $('readysecond').value, mode = $('readymode').value;
  if (!ChatOnboarding.ready(readiness, name) || (mode !== 'individual' && !ChatOnboarding.ready(readiness, second))) { $('readyerror').textContent = 'Confirm a ready agent for each participant.'; $('readyerror').hidden = false; return; }
  const c = pendingSelection || cur(); if (!c) return;
  if (!c.log.length && !busy(c)) c.participants = [];
  const added = [addParticipant(c, { agent: name, role: mode === 'team' ? 'Lead developer' : '' })];
  if (mode !== 'individual') added.push(addParticipant(c, { agent: second, role: mode === 'team' ? 'Reviewer' : '' }));
  c.sendTo = added.length === active(c).length ? null : added.map(p => p.name);
  saveSoon(); render(); $('readydialog').close(); $('input').focus();
};
function updateParticipantReadiness() {
  const a = readiness?.agents?.find(a => a.agent === $('pagent').value);
  $('preadiness').textContent = a ? a.agent + ' · ' + a.readiness + ' · ' + a.reason + (a.action ? ' · Next: ' + a.action : '') : 'Choose an agent. Readiness must be verified before sending.';
}
$('pagent').addEventListener('change', updateParticipantReadiness);
async function prepareSend(c, targets) {
  try {
    const report = await refreshReadiness(c);
    if (cur() !== c) return false;
    const context = report.git;
    const blocked = targets.filter(p => !ChatOnboarding.ready(report, p.agent));
    if (blocked.length) { toast('Unavailable: ' + blocked.map(p => p.name).join(', ') + '. Your draft is preserved.'); await openReadiness(); return false; }
    if (!context) { toast('Git context unavailable. Check the checkout in the terminal.'); return false; }
    // Old conversations gain an origin without rewriting their sessions/history.
    if (!c.gitOrigin) { c.gitOrigin = structuredClone(context); saveSoon(); }
    if (ChatOnboarding.branchChanged(c.gitOrigin, context, c.gitAcknowledged)) {
      $('branchtext').textContent = 'Branch changed from ' + c.gitOrigin.label + ' to ' + context.label + '.';
      $('branchdetails').textContent = 'Origin: ' + c.gitOrigin.checkout + '\nCurrent: ' + context.checkout;
      const approved = await new Promise(resolve => {
        const dialog = $('branchdialog');
        const end = value => { dialog.close(); resolve(value); };
        $('branchcontinue').onclick = () => { c.gitAcknowledged = ChatOnboarding.gitKey(context); saveSoon(); end(true); };
        $('branchnew').onclick = () => { end(false); newConv(); };
        $('branchcancel').onclick = () => end(false);
        dialog.oncancel = () => resolve(false); dialog.showModal();
      });
      if (!approved) return false;
    }
    if (cur() !== c) return false;
    c.gitCurrent = structuredClone(context);
    return true;
  } catch (e) { toast(e.message + ' Your draft is preserved.'); return false; }
}

$('worktreebtn').onclick = async () => {
  const c = cur(); if (!c || busy(c)) { toast('Wait for replies to finish before changing checkout.'); return; }
  $('worktreeerror').hidden = true;
  try {
    const response = await fetch('/api/worktrees'); if (!response.ok) throw new Error('Worktree list unavailable');
    const trees = await response.json(); const select = $('worktreeselect'); select.textContent = '';
    for (const tree of trees) { const option = el('option', '', tree.label + ' · ' + tree.checkout); option.value = tree.checkout; select.append(option); }
    select.value = c.checkout || meta.repo; $('worktreedialog').showModal();
  } catch (e) { toast(e.message); }
};
$('worktreecancel').onclick = () => $('worktreedialog').close();
$('worktreeconfirm').onclick = async () => {
  const c = cur(); if (!c || busy(c)) return;
  c.checkout = $('worktreeselect').value;
  for (const p of c.participants) { p.session = ''; p.seen = 0; }
  c.gitOrigin = null; c.gitAcknowledged = null; saveSoon(); $('worktreedialog').close();
  await refreshReadiness(); c.gitOrigin = structuredClone(gitContext); saveSoon(); render();
};

function updateAgentOptions() {
  const select = $('pagent'), selected = select.value; select.textContent = '';
  for (const a of meta.agents.filter(a => !(state.hiddenAgents || []).includes(a) || a === selected)) { const option = el('option', '', cap(a)); option.value = a; select.append(option); }
  select.value = selected;
}
