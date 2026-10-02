"use strict";
/* aic chat — the page script for chat.html. Model text only ever reaches the
   DOM through textContent / createTextNode, never innerHTML. */

const $ = (id) => document.getElementById(id);
const KEY = "aic-chat-v2";
const DRAFTS_KEY = "aic-chat-drafts";
let state = { convs: [], current: null, layout: "auto" };
let drafts = {}; // conversation id -> unsent text, saved apart from the (big) state
let meta = { agents: [], details: {}, default: "", repo: "", sandboxed: false };
let skills = []; // [{ name, description, argument_hint, requires_write }]
const running = new Map(); // `${conv id}\n${participant}` -> AbortController
const cardParts = new WeakMap(); // log entry -> DOM parts of its card

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
  more: I('<circle cx="5" cy="12" r="1.2"/><circle cx="12" cy="12" r="1.2"/><circle cx="19" cy="12" r="1.2"/>'),
  columns: I('<rect x="3" y="5" width="7.5" height="14" rx="1.5"/><rect x="13.5" y="5" width="7.5" height="14" rx="1.5"/>'),
  stack: I('<rect x="4" y="3.5" width="16" height="7.5" rx="1.5"/><rect x="4" y="13" width="16" height="7.5" rx="1.5"/>'),
  comment: I('<path d="M4 5h11a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H9l-4 3v-3H4z"/><path d="M17 9h3v8h-1v3l-3-3h-4"/>'),
  download: I('<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>'),
  alert: I('<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17v.5"/>'),
  rename: I('<path d="M4 20h16"/><path d="M6 16l9.5-9.5 3 3L9 19H6z"/>'),
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
// Saving is debounced while typing and reports failures (a full storage
// quota) in a banner that stays until a save works again.
function load() {
  try { state = Object.assign(state, JSON.parse(localStorage.getItem(KEY)) || {}); } catch {}
  try { drafts = JSON.parse(localStorage.getItem(DRAFTS_KEY)) || {}; } catch {}
  if (state.stack) state.layout = "stack";
  delete state.stack;
  state.convs.forEach((c) => {
    if (c.draft && !drafts[c.id]) drafts[c.id] = c.draft;
    delete c.draft;
    c.log.forEach((e) => {
      if (e.pending) { e.pending = false; if (!e.text) { e.error = true; e.soft = true; e.text = "interrupted"; } }
    });
  });
}
function storeItem(key, value) {
  try { localStorage.setItem(key, value); return true; }
  catch (err) {
    const b = $("banner");
    b.replaceChildren(icon("alert"), document.createTextNode(
      " Couldn't save your conversations (" + (err && err.name === "QuotaExceededError" ? "browser storage is full" : String(err && err.message || err)) +
      "). New messages will be lost when you close this tab: export and delete old conversations to free space."));
    b.hidden = false;
    return false;
  }
}
function save() {
  clearTimeout(save.t); save.t = 0;
  if (storeItem(KEY, JSON.stringify(state))) $("banner").hidden = true;
}
function saveSoon() { clearTimeout(save.t); save.t = setTimeout(save, 500); }
function saveDrafts() { clearTimeout(saveDrafts.t); saveDrafts.t = 0; storeItem(DRAFTS_KEY, JSON.stringify(drafts)); }
function saveDraftsSoon() { clearTimeout(saveDrafts.t); saveDrafts.t = setTimeout(saveDrafts, 400); }
window.addEventListener("pagehide", () => { if (save.t) save(); if (saveDrafts.t) saveDrafts(); });

const cur = () => state.convs.find((c) => c.id === state.current);
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

/* ---------- participants / conversations ---------- */
const NAME_OK = /^[\w-]+$/;
const cleanName = (s) => s.trim().replace(/\s+/g, "-").replace(/[^\w-]/g, "");
function uniqueName(c, base) {
  let name = base, n = 2;
  while (c.participants.some((p) => p.name === name)) name = base + "-" + n++;
  return name;
}
function addParticipant(c, f) {
  const p = { name: uniqueName(c, cleanName(f.name || f.agent) || "agent"), agent: f.agent, role: f.role || "", write: !!f.write,
    model: f.model || "", effort: f.effort || "", session: "", seen: 0 };
  p.hue = typeof f.hue === "number" ? f.hue : pickHue(c);
  c.participants.push(p);
  return p;
}
const defaultAgent = () => (meta.agents.includes(meta.default) ? meta.default : meta.agents[0] || "");
function newConv() {
  const c = { id: crypto.randomUUID(), title: "New conversation", participants: [], log: [], round: 0, updated: Date.now(), sendTo: null };
  const agent = defaultAgent();
  if (agent) addParticipant(c, { name: agent, agent });
  state.convs.unshift(c);
  $("search").value = "";
  switchTo(c.id);
  if (!agent) toast("No agents configured: check `aic config show`.");
}
function switchTo(id) {
  const old = cur(); if (old) { drafts[old.id] = $("input").value; saveDraftsSoon(); }
  state.current = id;
  $("input").value = drafts[id] || ""; autosize();
  document.body.classList.remove("drawer");
  save(); render(true); $("input").focus();
}
function removeConv(c) {
  if (busy(c)) { toast("Stop its replies before deleting it."); return; }
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
let renaming = null; // id of the conversation being renamed inline
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
  // Don't pull the inline rename box out from under the user's typing.
  if (renaming && document.activeElement?.closest?.("#list") && document.activeElement.tagName === "INPUT") return;
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
  $("repo").textContent = meta.repo; $("repo").title = meta.repo;
}
function convRow(c) {
  const row = el("div", "conv" + (c.id === state.current ? " active" : ""));
  if (renaming === c.id) {
    const input = el("input"); input.type = "text"; input.value = c.title; input.setAttribute("aria-label", "Conversation title");
    let done = false;
    const finish = (keep) => {
      if (done) return; done = true;
      const t = input.value.trim();
      if (keep && t) { c.title = t.slice(0, 80); c.renamed = true; save(); }
      renaming = null; input.blur(); renderSidebar(); setTitle();
    };
    input.onkeydown = (e) => { if (e.key === "Enter") finish(true); else if (e.key === "Escape") { e.stopPropagation(); finish(false); } };
    input.onblur = () => finish(true);
    row.appendChild(input);
    requestAnimationFrame(() => { input.focus(); input.select(); });
    return row;
  }
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
    item("rename", "Rename", () => { renaming = c.id; renderSidebar(); });
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
  let text, type;
  if (kind === "json") { text = JSON.stringify(c, null, 2); type = "application/json"; }
  else {
    const who = (name) => { const p = c.participants.find((x) => x.name === name); return p ? `${name} (${p.agent}${p.model ? ", " + p.model : ""})` : name; };
    const out = [`# ${c.title}`, "", "Participants: " + c.participants.map((p) => who(p.name)).join(", "), ""];
    c.log.forEach((e) => {
      const head = e.from === "you" ? "you" + (e.to ? " → " + e.to.join(", ") : "") : who(e.from);
      out.push(`## ${head}`, "", e.error ? `> ${e.text.replace(/\n/g, "\n> ")}` : e.text, "");
    });
    text = out.join("\n"); type = "text/markdown";
  }
  const a = el("a");
  a.href = URL.createObjectURL(new Blob([text], { type }));
  a.download = (c.title.replace(/[^\w-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 60) || "conversation") + "." + kind;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 2000);
}

/* ---------- rendering: top bar and layout switch ---------- */
function renderBar() {
  const c = cur(), chips = $("chips"); chips.textContent = "";
  if (c) c.participants.forEach((p) => {
    const chip = el("button", "chip"); chip.type = "button";
    chip.title = (p.role ? p.role + "\n\n" : "") + "Edit " + p.name;
    chip.appendChild(avatar(c, p.name));
    chip.appendChild(el("span", "", p.name));
    chip.appendChild(el("small", "", p.agent + (p.model ? " · " + p.model : "")));
    if (p.write) chip.appendChild(writeBadge());
    chip.onclick = () => openDialog(p);
    chips.appendChild(chip);
  });
  $("addbtn").disabled = !c || !meta.agents.length;

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
  document.body.classList.toggle("wide", !!c && c.participants.length >= 3);
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

function card(c, e) {
  const d = el("article", "card");
  d.style.setProperty("--h", hueFor(c, e.from));
  const head = el("header"), status = el("div", "status"), body = el("div", "body md"), acts = el("div"), foot = el("footer");
  d.append(head, status, body, acts, foot);
  cardParts.set(e, { card: d, head, status, body, acts, foot, mode: "", rendered: null, stream: null, keys: {} });
  paintCard(c, e);
  return d;
}

// Repaints one card from its entry; used for streaming without rebuilding the
// page. Each part is rebuilt only when what it shows changed.
function paintCard(c, e) {
  const k = cardParts.get(e); if (!k) return;
  const p = c.participants.find((x) => x.name === e.from);
  const changed = (part, key) => { const s = JSON.stringify(key); if (k.keys[part] === s) return false; k.keys[part] = s; return true; };
  k.card.classList.toggle("error", !!e.error && !e.soft);
  k.card.classList.toggle("soft", !!e.error && !!e.soft);

  if (changed("head", [p?.agent, p?.model, p?.write, e.skill, e.pending, hueFor(c, e.from)])) {
    k.card.style.setProperty("--h", hueFor(c, e.from));
    k.head.textContent = "";
    k.head.append(avatar(c, e.from), el("span", "name", e.from));
    if (p) k.head.appendChild(el("span", "badge", p.agent + (p.model ? " · " + p.model : "")));
    if (p?.write) k.head.appendChild(writeBadge());
    if (e.skill) { const s = el("span", "badge skill", "/" + e.skill); s.title = "ran the skill /" + e.skill; k.head.appendChild(s); }
    k.head.appendChild(el("span", "grow"));
    if (e.pending) k.head.appendChild(iconBtn("stop", "Stop " + e.from, () => running.get(runKey(c, { name: e.from }))?.abort()));
  }

  if (changed("status", [e.pending, e.activity || "", !!e.text])) {
    k.status.textContent = "";
    if (e.pending) {
      const act = el("span", "act dots", e.activity || (e.text ? "writing" : "thinking"));
      k.status.appendChild(act);
      const t = el("span"); t.dataset.tick = e.t0; k.status.appendChild(t);
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
    if (!e.pending) {
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
  const u = el("div", "user" + (c.participants.length > 1 ? " center" : ""));
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
const view = { id: null, sig: "", entries: [], wrap: null, after: null, row: null, lastUser: null };
const logSig = (c) => JSON.stringify([c.participants.map((p) => [p.name, p.agent, p.model, p.write, hueOf(p)]), state.layout]);
function renderLog(force) {
  const c = cur(), msgs = $("msgs");
  msgs.classList.toggle("stack", state.layout === "stack");
  msgs.classList.toggle("cols", state.layout === "columns");
  if (!c || !c.log.length) { view.id = null; msgs.replaceChildren(emptyState(c)); return; }
  const prefix = view.id === c.id && view.sig === logSig(c) && view.entries.length <= c.log.length && view.entries.every((e, i) => c.log[i] === e);
  if (force || !prefix) {
    view.id = c.id; view.sig = logSig(c); view.entries = []; view.row = null; view.lastUser = null;
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
    view.row = null;
    const prev = view.lastUser;
    const u = userBubble(c, e);
    view.wrap.insertBefore(u, view.after);
    view.lastUser = u;
    if (prev) prev._fill(); // only the newest message can be edited
    return;
  }
  let r = view.row;
  if (!r || r.round !== e.round || r.cells.get(e.from)?.taken) r = newRow(c, e.round);
  const node = card(c, e);
  let cell = r.cells.get(e.from);
  if (!cell) { // someone no longer in the conversation: extra column
    r.names.push(e.from); r.el.style.setProperty("--cols", r.names.length);
    cell = { node: null, col: r.names.length }; r.cells.set(e.from, cell);
  }
  if (r.group) node.style.gridColumn = cell.col;
  if (cell.node) cell.node.replaceWith(node); else r.el.appendChild(node);
  cell.node = node; cell.taken = true;
}
// One column per participant, in participant order, so replies line up
// round after round; someone not addressed gets a quiet placeholder.
function newRow(c, round) {
  const names = c.participants.map((p) => p.name);
  const group = names.length > 1;
  const r = { round, group, names: [...names], cells: new Map(), el: el("div", "row" + (group ? "" : " single")) };
  r.el.style.setProperty("--cols", names.length);
  if (group) names.forEach((n, i) => {
    const g = el("div", "gap", "not addressed"); g.style.gridColumn = i + 1;
    r.el.appendChild(g); r.cells.set(n, { node: g, col: i + 1 });
  });
  view.wrap.insertBefore(r.el, view.after);
  view.row = r;
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
  if (!meta.agents.length) {
    d.append(el("h1", "", "No agents configured"), el("p", "", "Add one with `aic config` and reload this page."));
    return d;
  }
  if (!c) return d;
  const group = c.participants.length > 1;
  d.appendChild(el("h1", "", group ? "Talk to several agents at once" : "Start a conversation"));
  d.appendChild(el("p", "", group
    ? "Your message goes to everyone in “To:”. Once they've answered, you can ask them to comment on each other's replies."
    : "Ask anything about this repository. Add a participant (even a second claude) to compare answers side by side."));
  const people = el("div", "people");
  c.participants.forEach((p) => {
    const b = el("button", "person"); b.type = "button"; b.title = "Edit " + p.name;
    b.style.setProperty("--h", hueOf(p));
    b.append(avatar(c, p.name, true), el("span", "name", p.name));
    const tags = el("span"); tags.appendChild(el("span", "badge", p.agent + (p.model ? " · " + p.model : "")));
    if (p.write) { tags.append(" "); tags.appendChild(writeBadge()); }
    b.appendChild(tags);
    if (p.role) b.appendChild(el("span", "role", p.role));
    b.onclick = () => openDialog(p);
    people.appendChild(b);
  });
  const add = el("button", "person add"); add.type = "button";
  add.append(icon("plus"), el("span", "", "Add participant"));
  add.onclick = () => openDialog(null);
  people.appendChild(add);
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
// Who a message typed now would go to: leading @mentions win, else the "To:" pills.
function recipients(c, text) {
  const named = leadingMentions(text ?? $("input").value)?.filter((n) => c.participants.some((p) => p.name === n));
  const names = named && named.length ? named : c.sendTo;
  return names ? c.participants.filter((p) => names.includes(p.name)) : c.participants;
}
function renderComposer() {
  const c = cur(), to = $("to"); to.textContent = "";
  if (c) {
    const sel = recipients(c);
    c.participants.forEach((p) => {
      const on = sel.includes(p);
      const b = el("button", "pill"); b.type = "button";
      b.style.setProperty("--h", hueOf(p));
      b.setAttribute("aria-pressed", String(on));
      b.appendChild(el("span", "dot"));
      b.appendChild(el("span", "", p.name));
      const n = unseen(c, p).length;
      if (n && c.log.length) { const cnt = el("span", "count", String(n)); b.appendChild(cnt); }
      b.title = (on ? "Receives your message" : "Doesn't receive your message") + (n ? ` · ${n} new message${n > 1 ? "s" : ""} to read` : "");
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
  let msg = "Enter sends · Shift+Enter new line · @ mentions" + (skills.length ? " · / skills" : ""), warn = false;
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
  const all = c.participants.map((q) => q.name);
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
  const names = leadingMentions($("input").value)?.filter((n) => c.participants.some((p) => p.name === n));
  if (names && names.length) { c.sendTo = uniq(names); c.toFromText = true; }
  else if (c.toFromText) { c.sendTo = null; c.toFromText = false; }
}

function setTitle() { const c = cur(); document.title = c ? `${c.title} · aic chat` : "aic chat"; }
function render(scrollEnd) {
  renderSidebar(); renderBar(); renderLog(); renderComposer(); setTitle();
  if (scrollEnd) toEnd();
}

/* ---------- scrolling that doesn't fight the reader ---------- */
const nearEnd = () => { const m = $("msgs"); return m.scrollHeight - m.scrollTop - m.clientHeight < 80; };
const toEnd = () => { const m = $("msgs"); m.scrollTop = m.scrollHeight; $("jump").classList.remove("show"); };
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
function systemFor(c, p) {
  const others = c.participants.filter((q) => q !== p).map((q) => q.name);
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
function launch(c, targets, userText, to, opts = {}) {
  if (busy(c)) return;
  c.round = (c.round || 0) + 1; c.updated = Date.now();
  if (userText) {
    const first = !c.log.some((e) => e.from === "you");
    c.log.push({ from: "you", round: c.round, text: userText, t0: Date.now(), ...(to ? { to } : {}), ...(opts.edited ? { edited: true } : {}) });
    if (first && !c.renamed) c.title = userText.slice(0, 48);
  }
  const jobs = targets
    .map((p) => ({ p, ...promptFor(c, p, opts), oldSeen: p.seen }))
    .filter((j) => j.text.trim() && unseen(c, j.p, opts.until).length);
  jobs.forEach((j) => { j.p.seen = opts.until ?? c.log.length; });
  jobs.forEach((j) => {
    j.entry = { from: j.p.name, round: c.round, text: "", pending: true, t0: Date.now(), seen0: j.oldSeen, actions: [], ...(j.skill ? { skill: j.skill } : {}) };
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
  launch(c, [p], null, null, { regen: !e.error, until: idx });
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
        agent: p.agent, prompt, session_id: p.session, system: systemFor(c, p), write: !!p.write,
        model: p.model || null, effort: p.effort || null,
      }),
      signal: running.get(key).signal,
    });
    if (!res.ok) throw new Error(await res.text());
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
        const ev = JSON.parse(line);
        if (ev.type === "session") p.session = ev.id;
        else if (ev.type === "activity") {
          entry.activity = ev.text.split(meta.repo + "/").join("");
          if (entry.actions.length < 500) entry.actions.push(entry.activity);
          paint();
        } else if (ev.type === "delta") { entry.text += ev.text; paint(); }
        else if (ev.type === "done") {
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
    if (!busy(c)) announce(cur() === c ? "Replies finished." : `Replies finished in “${c.title}”.`);
    else announce(`${p.name} finished.`);
  }
}

/* ---------- input ---------- */
function clearInput(c) { $("input").value = ""; drafts[c.id] = ""; saveDraftsSoon(); c.toFromText = false; autosize(); acClose(); }
$("form").onsubmit = (e) => {
  e.preventDefault();
  const c = cur();
  if (!c) return;
  // While replies are coming in, the button is Stop; Enter does nothing, so
  // a reflex Enter on the next message can't throw the replies away.
  if (busy(c)) { if (e.submitter === $("send")) stopAll(c); else renderHint(); return; }
  const text = $("input").value.trim();
  const m = (text + " ").match(/^((?:@[\w-]+\s+)+)([\s\S]*)$/);
  let body = text;
  if (m) {
    const names = m[1].trim().split(/\s+/).map((n) => n.slice(1));
    const unknown = names.filter((n) => !c.participants.some((p) => p.name === n));
    if (unknown.length) { toast("Unknown participant: " + unknown.join(", ")); return; }
    body = m[2].trim();
  }
  const targets = recipients(c, text);
  if (!targets.length) { toast("Pick who receives the message in “To:”."); return; }
  const to = targets.length === c.participants.length ? null : targets.map((p) => p.name);
  if (!body) { // nothing to add: let them answer what's new to them
    const ready = targets.filter((p) => unseen(c, p).length);
    if (!ready.length) { toast(text ? "Nothing new for them to answer yet." : "Write a message first."); return; }
    clearInput(c); launch(c, ready); return;
  }
  const sk = skillOf(body), problem = sk && skillProblem(sk, targets);
  if (problem) { toast(problem); return; }
  clearInput(c);
  launch(c, targets, body, to);
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
    acItems = c.participants.filter((p) => !used.includes(p.name) && p.name.toLowerCase().startsWith(q))
      .map((p) => ({ type: "@", value: p.name, label: "@" + p.name, sub: p.agent + (p.role ? " · " + p.role : "") }));
  } else if (c && skills.length && (m = before.match(SKILL_Q))) {
    const q = m[2].toLowerCase();
    const allAgy = recipients(c).length > 0 && recipients(c).every((p) => kindOf(p) === "agy");
    acItems = skills.filter((s) => s.name.toLowerCase().includes(q))
      .sort((a, b) => b.name.toLowerCase().startsWith(q) - a.name.toLowerCase().startsWith(q))
      .map((s) => ({ type: "/", value: s.name, label: "/" + s.name, arg: s.argument_hint, sub: s.description,
        off: allAgy ? "agy participants can't run skills" : "" }));
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
$("input").onkeydown = (e) => {
  if (acItems.length) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      acSel = (acSel + (e.key === "ArrowDown" ? 1 : acItems.length - 1)) % acItems.length; acUpdate(); return;
    }
    if (e.key === "Tab" || e.key === "Enter") { e.preventDefault(); acPick(acSel); return; }
    if (e.key === "Escape") { e.stopPropagation(); acClose(); return; }
  }
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
  autosize(); acUpdate();
  const c = cur();
  if (c) { drafts[c.id] = $("input").value; saveDraftsSoon(); syncPillsFromText(c); renderComposer(); }
}
$("input").oninput = onInput;
$("input").onblur = acClose;
$("input").onclick = acUpdate;

/* ---------- participant dialog ---------- */
let editing = null; // participant being edited, or null when adding
let dlgHue = 0;
const MODELS = { claude: ["opus", "sonnet", "haiku"], agy: [] };
const EFFORTS = { claude: ["low", "medium", "high", "xhigh", "max"], agy: [] };
const access = () => document.querySelector('input[name="paccess"]:checked').value;
function fillList(id, values) {
  const dl = $(id); dl.textContent = "";
  uniq(values.filter(Boolean)).forEach((v) => { const o = el("option"); o.value = v; dl.appendChild(o); });
}
function openDialog(p) {
  const c = cur(); if (!c || !meta.agents.length) return;
  editing = p || null;
  $("ptitle").textContent = p ? "Edit " + p.name : "Add participant";
  $("psave").textContent = p ? "Save" : "Add";
  $("pname").value = p ? p.name : "";
  $("pagent").value = p ? p.agent : defaultAgent();
  $("pmodel").value = p?.model || ""; $("peffort").value = p?.effort || "";
  $("prole").value = p?.role || "";
  document.querySelector(`input[name="paccess"][value="${p?.write ? "write" : "read"}"]`).checked = true;
  dlgHue = p ? hueOf(p) : pickHue(c);
  const replying = !!p && isRunning(c, p);
  $("pname").disabled = replying; $("pagent").disabled = replying;
  $("pname").title = $("pagent").title = replying ? `Wait for ${p.name} to finish replying` : "";
  $("premove").hidden = !p;
  $("perror").hidden = true;
  updateDialog();
  $("pdialog").showModal();
  (p ? $("prole") : $("pname")).focus();
}
function updateDialog() {
  const c = cur(), agent = $("pagent").value, d = meta.details?.[agent] || {}, kind = d.kind || "claude";
  $("pmodel").placeholder = "agent default" + (d.model ? ` (${d.model})` : "");
  $("peffort").placeholder = "agent default" + (d.effort ? ` (${d.effort})` : "");
  fillList("pmodels", [d.model, ...MODELS[kind]]);
  fillList("pefforts", [d.effort, ...EFFORTS[kind]]);
  $("pagentwarn").hidden = !(editing && editing.agent !== agent && editing.session);
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
$("pagent").onchange = updateDialog;
document.querySelectorAll('input[name="paccess"]').forEach((r) => { r.onchange = updateDialog; });
$("pname").oninput = () => { $("pname").value = $("pname").value.replace(/\s+/g, "-").replace(/[^\w-]/g, ""); };
$("pcancel").onclick = () => $("pdialog").close();
$("premove").onclick = () => {
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
  const agent = $("pagent").value;
  const name = cleanName($("pname").value) || (editing ? editing.name : uniqueName(c, cleanName(agent) || "agent"));
  if (!NAME_OK.test(name)) return dialogError("Use letters, digits, - and _ in the name.");
  if (c.participants.some((q) => q !== editing && q.name === name)) return dialogError(`There's already a participant called ${name}.`);
  const fields = { agent, role: $("prole").value.trim(), write: access() === "write", model: $("pmodel").value.trim(), effort: $("peffort").value.trim(), hue: dlgHue };
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
  } else {
    const p = addParticipant(c, { name, ...fields });
    if (c.sendTo) c.sendTo.push(p.name);
  }
  $("pdialog").close();
  c.updated = Date.now(); save(); render();
};

/* ---------- buttons and shortcuts ---------- */
$("new").appendChild(icon("plus"));
$("new").setAttribute("aria-label", "New conversation");
$("new").onclick = newConv;
$("menu").appendChild(icon("menu"));
$("menu").onclick = () => document.body.classList.add("drawer");
$("scrim").onclick = () => document.body.classList.remove("drawer");
$("addbtn").appendChild(icon("plus"));
$("addbtn").onclick = () => openDialog(null);
$("search").oninput = renderSidebar;
$("search").onkeydown = (e) => { if (e.key === "Escape" && $("search").value) { e.stopPropagation(); $("search").value = ""; renderSidebar(); } };

document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && e.shiftKey && (e.key === "O" || e.key === "o")) { e.preventDefault(); newConv(); return; }
  if (e.key !== "Escape" || e.defaultPrevented || $("pdialog").open) return;
  if (openMenu) { closeMenu(); return; }
  if (document.body.classList.contains("drawer")) { document.body.classList.remove("drawer"); return; }
  const c = cur();
  if (c && busy(c)) { e.preventDefault(); stopAll(c); }
});

(async () => {
  load();
  try {
    meta = Object.assign(meta, await (await fetch("/api/agents")).json());
  } catch (err) {
    $("msgs").textContent = "Couldn't reach the aic server: " + err.message; return;
  }
  try { skills = await (await fetch("/api/skills")).json(); } catch { skills = []; }
  meta.agents.forEach((a) => { const o = el("option", "", a); o.value = a; $("pagent").appendChild(o); });
  if (!cur()) state.current = state.convs[0]?.id || null;
  if (!cur()) newConv(); else { $("input").value = drafts[state.current] || ""; autosize(); render(true); }
})();
