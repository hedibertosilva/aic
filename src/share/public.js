"use strict";
const $ = id => document.getElementById(id);
const shareId = location.pathname.split('/').filter(Boolean)[1] || '';
let snapshot = null, current = null, timer = null;
async function request(path, options) {
  const response = await fetch(`/api/public/shares/${encodeURIComponent(shareId)}${path}`, options);
  if (!response.ok) throw Object.assign(new Error(response.status === 401 ? 'Invalid password or expired session.' : 'Could not load this share.'), {status:response.status});
  return response.status === 204 ? null : response.json();
}
function node(tag, cls, text) { const n=document.createElement(tag); if(cls)n.className=cls; if(text!==undefined)n.textContent=text; return n; }
function render() {
  $('share-title').textContent = snapshot.name || 'Shared conversations';
  const conversations = snapshot.conversations || [];
  if (!conversations.some(c => c.id === current)) current = conversations[0]?.id || null;
  $('conversations').replaceChildren(...conversations.map(c => { const b=node('button',c.id===current?'active':'',c.title||'Conversation'); b.onclick=()=>{current=c.id;render()}; return b; }));
  const conversation = conversations.find(c => c.id === current);
  $('conversation-title').textContent = conversation?.title || 'No conversations';
  $('updated').textContent = snapshot.updated_at ? `Updated ${new Date(snapshot.updated_at).toLocaleString()}` : '';
  if (!conversation) { $('messages').replaceChildren(node('div','empty','No conversations are currently shared.')); return; }
  $('messages').replaceChildren(...(conversation.log||[]).map(entry => {
    const card=node('article','message'+(entry.from==='you'?' user':''));
    const head=node('header'); head.append(node('strong','',entry.from==='you'?'User':entry.from||'Participant'));
    if(entry.t0) head.append(node('span','',new Date(entry.t0).toLocaleString()));
    card.append(head,node('pre','',entry.text||'')); return card;
  }));
}
async function load() {
  try { snapshot=await request(''); $('unlock').hidden=true; $('app').hidden=false; render(); }
  catch(error) { if(error.status===401){clearInterval(timer);timer=null;$('app').hidden=true;$('unlock').hidden=false;} }
}
$('unlock-form').onsubmit=async event=>{event.preventDefault();$('error').textContent='';try{await request('/unlock',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({password:$('password').value})});$('password').value='';await load();if(!timer)timer=setInterval(load,3000);}catch(error){$('error').textContent=error.message;}};
$('logout').onclick=async()=>{try{await request('/logout',{method:'POST'});}catch{}clearInterval(timer);timer=null;$('app').hidden=true;$('unlock').hidden=false;$('password').focus();};
load().then(()=>{if(!$('app').hidden)timer=setInterval(load,3000)});
