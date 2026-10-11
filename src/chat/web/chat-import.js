"use strict";
// Portable aic conversations. No DOM, storage, or agent execution in this module.
const ChatImport = (() => {
  const MAX_BYTES = 16 * 1024 * 1024;
  const object = (v) => v && typeof v === "object" && !Array.isArray(v);
  const fail = (message) => { throw new Error(message); };
  const strings = (v) => Array.isArray(v) && v.every((s) => typeof s === "string");
  const numeric = (v) => typeof v === "number" && Number.isFinite(v) && v >= 0;
  const cap = (s) => s.charAt(0).toUpperCase() + s.slice(1);
  const label = (c, name) => {
    const p = c.participants.find((p) => p.name === name);
    return p ? `${name} (${cap(p.agent)}${p.model ? ", " + cap(p.model) : ""})` : name;
  };
  const heading = (c, e) => e.from === "you" ? "you" + (e.to ? " → " + e.to.join(", ") : "") : label(c, e.from);

  function portable(c) {
    const participants = c.participants.map((p) => {
      const out = {};
      for (const k of ["name", "agent", "model", "role", "write", "effort", "hue", "inactive"]) {
        if (p[k] !== undefined) out[k] = p[k];
      }
      return out;
    });
    const log = c.log.map((e) => {
      const out = {};
      for (const k of ["from", "agent", "model", "to", "text", "round", "t0", "dur", "usage", "actions", "pasted", "error", "soft", "stopped", "edited"]) {
        if (e[k] !== undefined) out[k] = e[k];
      }
      if (e.pending) { out.stopped = true; if (!out.text) { out.error = true; out.text = "interrupted"; } }
      return out;
    });
    return { version: 1, title: c.title, participants, log, round: c.round || 0, sendTo: c.sendTo || null };
  }
  function exportText(c, kind) {
    const data = portable(c);
    if (kind === "json") return JSON.stringify(data, null, 2);
    // JSON flow values are valid YAML. Line counts keep arbitrary Markdown,
    // including headings and fences inside messages, unambiguous and lossless.
    const info = { ...data, log: data.log.map(({ text, ...e }) => ({ ...e, textLines: text.split("\n").length })) };
    const out = ["---", "aic: " + JSON.stringify(info), "---", `# ${c.title.replace(/\n/g, " ")}`, "",
      "Participants: " + c.participants.map((p) => label(c, p.name)).join(", "), ""];
    data.log.forEach((e) => out.push(`## ${heading(c, e)}`, "", e.text, ""));
    return out.join("\n");
  }

  function normalize(data, agents, maxParticipants = 5) {
    if (!object(data) || !Array.isArray(data.participants) || !Array.isArray(data.log)) fail("This JSON is not an aic conversation (participants and log are required).");
    if (data.version !== undefined && data.version !== 1) fail("Unsupported conversation version: " + data.version);
    if (typeof data.title !== "string" || !data.title.trim()) fail("The conversation title is missing.");
    if (data.participants.length > maxParticipants) fail(`A conversation can have at most ${maxParticipants} participants.`);
    const names = new Set();
    const participants = data.participants.map((p) => {
      if (!object(p) || typeof p.name !== "string" || !/^[\w-]+$/.test(p.name) || p.name === "you" || names.has(p.name)) fail("Invalid or duplicate participant name.");
      names.add(p.name);
      if (typeof p.agent !== "string" || !p.agent.trim()) fail("Missing agent for " + p.name);
      for (const k of ["model", "role", "effort"]) if (p[k] !== undefined && typeof p[k] !== "string") fail("Invalid " + k + " for " + p.name);
      const agent = agents.find((a) => a.toLowerCase() === p.agent.toLowerCase()) || p.agent;
      return { name: p.name, agent, role: p.role || "", model: p.model || "", effort: p.effort || "",
        ...(numeric(p.hue) && p.hue < 360 ? { hue: p.hue } : {}),
        write: false, inactive: !!p.inactive || !agents.includes(agent), session: "", seen: data.log.length };
    });
    const recipients = (to) => {
      if (to == null) return null;
      if (!strings(to) || to.some((name) => !names.has(name))) fail("A message references an unknown recipient.");
      return [...new Set(to)];
    };
    let round = 0;
    const log = data.log.map((e) => {
      if (!object(e) || typeof e.text !== "string" || (e.from !== "you" && !names.has(e.from))) fail("Invalid message or unknown author.");
      if (e.from === "you") round++;
      const out = { from: e.from, text: e.text, round: Number.isSafeInteger(e.round) && e.round >= 0 ? e.round : round };
      for (const field of ["agent", "model"]) {
        if (e[field] !== undefined) { if (typeof e[field] !== "string") fail("Invalid " + field + " for reply"); out[field] = e[field]; }
      }
      const to = recipients(e.to); if (to) out.to = to;
      for (const k of ["t0", "dur"]) if (numeric(e[k])) out[k] = e[k];
      for (const k of ["error", "soft", "stopped", "edited"]) if (e[k] === true) out[k] = true;
      if (Array.isArray(e.pasted)) out.pasted = e.pasted.filter((paste) => object(paste)
        && Number.isSafeInteger(paste.start) && Number.isSafeInteger(paste.end) && Number.isSafeInteger(paste.lines)
        && paste.start >= 0 && paste.end >= paste.start && paste.end <= e.text.length && paste.lines > 0)
        .map(({ start, end, lines }) => ({ start, end, lines }));
      if (e.pending) { out.stopped = true; if (!out.text) { out.error = true; out.text = "interrupted"; } }
      if (strings(e.actions)) out.actions = [...e.actions];
      if (object(e.usage)) out.usage = Object.fromEntries(Object.entries(e.usage).filter(([k, v]) =>
        ["input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "cost_usd", "num_turns"].includes(k) && numeric(v)));
      return out;
    });
    return { title: data.title, participants, log, round: log.reduce((max, e) => Math.max(max, e.round), 0), sendTo: recipients(data.sendTo), keepEmpty: true, renamed: true };
  }

  function markdown(text) {
    const lines = text.split("\n");
    if (lines[0] === "---" && lines[1]?.startsWith("aic:")) {
      if (lines[2] !== "---") fail("Invalid aic Markdown frontmatter.");
      let data;
      try { data = JSON.parse(lines[1].slice(4).trim()); } catch { fail("Invalid aic Markdown metadata."); }
      if (!object(data) || !Array.isArray(data.log) || !Array.isArray(data.participants)) fail("Invalid aic Markdown metadata.");
      let pos = 7;
      data.log = data.log.map((e) => {
        if (!object(e) || !Number.isSafeInteger(e.textLines) || e.textLines < 1 || !lines[pos]?.startsWith("## ") || lines[pos + 1] !== "") fail("Markdown messages do not match their metadata.");
        const start = pos + 2, end = start + e.textLines;
        if (end >= lines.length || lines[end] !== "") fail("Incomplete Markdown message.");
        pos = end + 1;
        const { textLines, ...entry } = e;
        return { ...entry, text: lines.slice(start, end).join("\n") };
      });
      if (lines.slice(pos).some((line) => line !== "")) fail("Unexpected content after Markdown messages.");
      return data;
    }
    // Legacy exports have a title and a Participants line. Ordinary Markdown
    // remains an unsent draft, even if it happens to contain level-two headings.
    if (!lines[0]?.startsWith("# ") || !lines[2]?.startsWith("Participants: ")) return null;
    const participants = [], log = [];
    const add = (name, agent, model) => {
      const old = participants.find((p) => p.name === name);
      if (old && (old.agent !== agent || old.model !== (model || ""))) fail("Conflicting Markdown participant: " + name);
      if (!old) participants.push({ name, agent, model: model || "" });
    };
    const pattern = /([\w-]+) \(([^,()]+)(?:, ([^()]+))?\)/g;
    for (const m of lines[2].slice(14).matchAll(pattern)) add(m[1], m[2], m[3]);
    let entry = null, body = [], fence = null;
    const finish = () => { if (entry) { entry.text = body.join("\n").replace(/^\n/, "").replace(/\n$/, ""); log.push(entry); } };
    for (const line of lines.slice(4)) {
      const f = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
      if (f) {
        if (!fence) fence = f[1];
        else if (f[1][0] === fence[0] && f[1].length >= fence.length && !f[2].trim()) fence = null;
      }
      const m = !fence && line.match(/^## (you)(?: → (.*))?$/);
      const p = !fence && line.match(/^## ([\w-]+) \(([^,()]+)(?:, ([^()]+))?\)$/);
      if (m || p) {
        finish(); body = [];
        if (p) { add(p[1], p[2], p[3]); entry = { from: p[1] }; }
        else entry = { from: "you", ...(m[2] ? { to: m[2].split(", ") } : {}) };
      } else if (entry) body.push(line);
      else if (line.trim()) fail("Unrecognized content before Markdown messages.");
    }
    finish();
    return { title: lines[0].slice(2), participants, log };
  }
  function parse(text, extension, agents, maxParticipants) {
    text = text.replace(/^\uFEFF/, "").replace(/\r\n/g, "\n");
    let data;
    if (extension === "json") {
      try { data = JSON.parse(text); } catch { fail("Invalid JSON. Check the file syntax."); }
    } else if (extension === "md") {
      data = markdown(text);
      if (!data) return { draft: text };
    } else fail("Choose a .json or .md file.");
    return { conversation: normalize(data, agents, maxParticipants) };
  }
  return { MAX_BYTES, parse, exportText };
})();
if (typeof module !== "undefined") module.exports = ChatImport;
