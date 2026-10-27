"use strict";
const $ = id => document.getElementById(id);
const shareId = location.pathname.split('/').filter(Boolean)[1] || '';
let snapshot = null, current = null, timer = null;
let viewSettings = { layout: 'columns', chatWidth: 1 };
try { const theme=localStorage.getItem('aic-chat-theme'); if(theme==='light'||theme==='dark')document.documentElement.dataset.theme=theme; } catch {}
try {
  const saved=JSON.parse(localStorage.getItem('aic-share-view')||'null');
  if(saved?.layout==='columns'||saved?.layout==='stack')viewSettings.layout=saved.layout;
  if(Number.isInteger(saved?.chatWidth)&&saved.chatWidth>=1&&saved.chatWidth<=3)viewSettings.chatWidth=saved.chatWidth;
} catch {}

function node(tag, cls, text) {
  const value = document.createElement(tag);
  if (cls) value.className = cls;
  if (text !== undefined) value.textContent = text;
  return value;
}
function icon(paths) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('aria-hidden', 'true'); svg.classList.add('i');
  svg.innerHTML = paths;
  return svg;
}
function hashHue(name) { let hue=0; for(const char of name) hue=(hue*31+char.charCodeAt(0))%360; return hue; }
function colour(value, name) { value.classList.add(`share-hue-${hashHue(name)%12}`); return value; }
function stamp(value) { return value ? new Date(value).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'}) : ''; }
function saveViewSettings() { try { localStorage.setItem('aic-share-view',JSON.stringify(viewSettings)); } catch {} }

async function request(path, options) {
  const response = await fetch(`/api/public/shares/${encodeURIComponent(shareId)}${path}`, options);
  if (!response.ok) throw Object.assign(new Error(response.status === 401 ? 'Invalid password or expired session.' : 'Could not load this sharing.'), {status:response.status});
  return response.status === 204 ? null : response.json();
}
function showUnlock(message='') {
  clearInterval(timer); timer=null; $('app').hidden=true;
  $('error').textContent=message; $('error').hidden=!message;
  if (!$('unlock').open) $('unlock').showModal();
  requestAnimationFrame(()=>$('password').focus());
}
function showApp() {
  if ($('unlock').open) $('unlock').close();
  $('app').hidden=false;
}
function renderSidebar(conversations) {
  $('share-title').textContent=snapshot.name||'Shared conversations';
  $('conversations').replaceChildren(...conversations.map(conversation=>{
    const button=node('button',conversation.id===current?'active':''); button.type='button';
    button.append(node('span','',conversation.title||'Conversation'));
    button.onclick=()=>{current=conversation.id;render();if(matchMedia('(max-width:700px)').matches)document.body.classList.remove('drawer');};
    return button;
  }));
}
function renderBar(conversation) {
  const chips=(conversation?.participants||[]).map(participant=>{
    const chip=colour(node('span','chip'),participant.name); chip.title=participant.role||participant.name;
    const identity=node('span','chipidentity'), line=node('span','chipnameline');
    line.append(node('span','dot'),node('span','chipname',participant.name));
    identity.append(line,node('small','',participant.agent||'Participant')); chip.append(identity); return chip;
  });
  $('chips').replaceChildren(...chips);
}
function renderLayout(conversation) {
  const controls=$('layout'); controls.replaceChildren();
  const messages=$('msgs');
  document.documentElement.dataset.chatWidth=String(viewSettings.chatWidth);
  messages.classList.toggle('cols',viewSettings.layout==='columns');
  messages.classList.toggle('stack',viewSettings.layout==='stack');
  if(!conversation?.participants?.length)return;
  const add=(mode,label,paths)=>{
    const button=node('button','icon'); button.type='button'; button.title=label; button.setAttribute('aria-label',label);
    button.setAttribute('aria-pressed',String(viewSettings.layout===mode)); button.append(icon(paths));
    button.onclick=()=>{viewSettings.layout=mode;saveViewSettings();renderLayout(conversation);}; controls.append(button);
  };
  add('columns','Replies side by side','<rect x="3" y="5" width="7.5" height="14" rx="1.5"/><rect x="13.5" y="5" width="7.5" height="14" rx="1.5"/>');
  add('stack','Replies stacked','<rect x="4" y="3.5" width="16" height="7.5" rx="1.5"/><rect x="4" y="13" width="16" height="7.5" rx="1.5"/>');
  const next=viewSettings.chatWidth===3?1:viewSettings.chatWidth+1;
  const width=node('button','icon chatwidth'); width.type='button'; width.dataset.level=String(viewSettings.chatWidth);
  width.title=`Chat area width: level ${viewSettings.chatWidth} of 3. Change to level ${next}`; width.setAttribute('aria-label',width.title);
  width.append(icon('<path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"/>'));
  width.onclick=()=>{viewSettings.chatWidth=next;saveViewSettings();renderLayout(conversation);}; controls.append(width);
}
function renderMessages(conversation) {
  const messages=$('msgs');
  if (!conversation?.log?.length) { messages.replaceChildren(node('div','empty','No messages are currently shared.')); return; }
  const wrap=node('div','wrap'), rows=new Map();
  for(const entry of conversation.log) {
    if(entry.from==='you') {
      const user=node('div','user'), to=entry.to?.length?entry.to:(conversation.participants||[]).map(p=>p.name);
      user.append(node('span','to','TO: '+(to.length?to.join(', '):'No participants')),node('div','text',entry.text||''));
      if(entry.edited) user.append(node('span','edited','edited'));
      wrap.append(user); continue;
    }
    const round=entry.round??`entry-${rows.size}`, key=String(round), grouped=rows.get(key)||[];
    let group=grouped.find(item=>!item.names.has(entry.from));
    if(!group) { group={node:node('div','row solo'),names:new Set()}; grouped.push(group); rows.set(key,grouped); wrap.append(group.node); }
    const row=group.node; group.names.add(entry.from);
    const card=colour(node('article','card'+(entry.error?' error':'')),entry.from||'Participant');
    const head=node('header'), identity=node('span','cardidentity');
    identity.append(node('span','dot'),node('span','name',entry.from||'Participant')); head.append(identity);
    const body=node('div','body md'), paragraph=node('p','',entry.text||''); body.append(paragraph);
    if(entry.error) paragraph.classList.add('err');
    const footer=node('footer'), metrics=node('span','metrics',stamp(entry.t0)+(entry.stopped?' · stopped':'')); footer.append(metrics);
    card.append(head,body,footer);
    const order=name=>{const index=(conversation.participants||[]).findIndex(participant=>participant.name===name);return index<0?Infinity:index;};
    const before=[...row.children].find(item=>order(item.dataset.participant)>order(entry.from)); card.dataset.participant=entry.from||'';
    row.insertBefore(card,before||null);
    row.classList.remove(...[...row.classList].filter(name=>name.startsWith('share-cols-')));
    row.classList.add(`share-cols-${Math.min(row.childElementCount,8)}`); row.classList.toggle('solo',row.childElementCount<2);
  }
  messages.replaceChildren(wrap);
}
function render() {
  const conversations=snapshot?.conversations||[];
  if(!conversations.some(conversation=>conversation.id===current)) current=conversations[0]?.id||null;
  const conversation=conversations.find(item=>item.id===current);
  renderSidebar(conversations); renderBar(conversation); renderLayout(conversation);
  $('conversation-title').textContent=conversation?.title||'No conversations';
  $('updated').textContent=snapshot.updated_at?`Updated ${new Date(snapshot.updated_at).toLocaleString()}`:'';
  renderMessages(conversation);
}
async function load() {
  try { snapshot=await request(''); showApp(); render(); return true; }
  catch(error) { if(error.status===401) showUnlock(); return false; }
}

$('unlock-form').onsubmit=async event=>{
  event.preventDefault(); $('error').hidden=true; $('unlock-button').disabled=true;
  try {
    await request('/unlock',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({password:$('password').value})});
    $('password').value=''; await load(); if(!timer)timer=setInterval(load,3000);
  } catch(error) { $('error').textContent=error.message; $('error').hidden=false; }
  finally { $('unlock-button').disabled=false; }
};
$('unlock').oncancel=event=>event.preventDefault();
$('logout').onclick=async()=>{try{await request('/logout',{method:'POST'});}catch{}showUnlock();};
$('menu').append(icon('<path d="M4 6h16M4 12h16M4 18h16"/>'));
$('menu').onclick=()=>document.body.classList.toggle('drawer');
$('scrim').onclick=()=>document.body.classList.remove('drawer');
function paintTheme() {
  const dark=document.documentElement.dataset.theme==='dark'; $('themebtn').replaceChildren(icon(dark?'<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>':'<path d="M21 12.8A9 9 0 1 1 11.2 3 7 7 0 0 0 21 12.8z"/>'));
  $('themebtn').title=$('themebtn').ariaLabel=dark?'Use light theme':'Use dark theme';
}
$('themebtn').onclick=()=>{const next=document.documentElement.dataset.theme==='dark'?'light':'dark';document.documentElement.dataset.theme=next;localStorage.setItem('aic-chat-theme',next);paintTheme();};
paintTheme();
load().then(ok=>{if(ok)timer=setInterval(load,3000)});
