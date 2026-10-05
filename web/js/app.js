/* ling · 应用逻辑：路由 + 五页面 + 流式对话 + 铸造 + 领地 + 星图 + 设置
   UX 审计 P0 修复模式：贴底跟随、流式中切会话不丢、横幅行动接线、键盘可用 */
const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];
const esc = (s) => { const d = document.createElement('div'); d.textContent = String(s ?? ''); return d.innerHTML; };
const fmtTs = (ts) => new Date(ts).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });

const state = { charId: null, sessionId: null, streaming: null, starmap: null, tlFocus: null };

/* ── 图标注入：所有 [data-icon] 元素插入内联 SVG（动态渲染后需再调用） ── */
function injectIcons(root = document) {
  $$('[data-icon]', root).forEach(el => {
    if (el.dataset.iconDone) return;
    el.insertAdjacentHTML('afterbegin', icon(el.dataset.icon, +(el.dataset.iconSize || 15)));
    el.dataset.iconDone = '1';
  });
}

/* ── Toast（普通 4s，错误 6s；exit 快于 enter；带语义图标） ── */
function toast(msg, isError = false) {
  const el = document.createElement('div');
  el.className = 'toast enter' + (isError ? ' error' : '');
  el.innerHTML = icon(isError ? 'x' : 'check', 14) + '<span style="margin-left:8px">' + esc(msg) + '</span>';
  el.style.display = 'flex'; el.style.alignItems = 'center';
  $('#toasts').appendChild(el);
  requestAnimationFrame(() => el.classList.remove('enter'));
  setTimeout(() => { el.style.opacity = '0'; el.style.transform = 'translateY(8px)';
    setTimeout(() => el.remove(), 260); }, isError ? 6000 : 4000);
}
const requireApi = () => {
  if (Store.apiReady()) { LLM.guard(Store.settings.baseUrl); return true; }
  toast('尚未配置模型 API，请先到设置页完成配置', true); location.hash = '#/settings'; return false;
};
const llmOpts = () => ({ ...Store.settings });

/* ── 路由 ── */
const routes = ['chat', 'characters', 'territory', 'starmap', 'settings'];
let booted = false;
function route() {
  const name = (location.hash.replace('#/', '') || 'chat').split('?')[0];
  const page = routes.includes(name) ? name : 'chat';
  $$('.page').forEach(p => p.classList.toggle('active', p.dataset.page === page));
  $$('.nav-item').forEach(n => n.classList.toggle('active', n.dataset.page === page));
  if (booted) Sfx.play('select');
  ({ chat: renderChat, characters: renderChars, territory: renderTerritory, starmap: renderStarmap, settings: renderSettings })[page]();
  EdgeGlow.collect();                 /* 页面重渲染后重新收集发光目标 */
  booted = true;
}
window.addEventListener('hashchange', route);

/* 音效开关（顶栏图标按钮） */
function renderSfxToggle() {
  const b = $('#sfx-toggle');
  b.innerHTML = icon(Sfx.muted() ? 'volume-x' : 'volume-2', 16);
  b.setAttribute('aria-label', Sfx.muted() ? '开启音效' : '关闭音效');
  b.title = Sfx.muted() ? '开启音效' : '关闭音效';
}

/* ══ 对话页 ══ */
function renderChat() {
  const chars = Store.characters;
  if (!state.charId || !Store.char(state.charId)) state.charId = chars[0]?.id ?? null;
  const c = Store.char(state.charId);
  $('#chat-empty').style.display = c ? 'none' : 'flex';
  $('#chat-body').style.display = c ? 'flex' : 'none';
  if (!c) { $('#char-list-empty').innerHTML = '<div class="glyph">🎭</div>还没有角色。<a href="#/characters">去角色页铸造 →</a>'; return; }
  if (!state.sessionId || !Store.session(c.id, state.sessionId)) state.sessionId = c.sessions[0]?.id ?? null;
  if (!state.sessionId) { const s = Store.newSession(c.id); state.sessionId = s.id; }

  /* 会话列表 */
  $('#sessions').innerHTML = c.sessions.map(s => `
    <div class="session-item ${s.id === state.sessionId ? 'active' : ''}" data-sid="${s.id}">
      <span>${esc(s.title)}</span><span class="del" data-del="${s.id}" title="删除会话">${icon('trash', 12)}</span>
    </div>`).join('');
  /* 角色切换条 */
  $('#chat-chars').innerHTML = chars.map(x => `
    <div class="session-item ${x.id === c.id ? 'active' : ''}" data-cid="${x.id}">
      <span>${esc(x.emoji)} ${esc(x.name)}</span>
    </div>`).join('');
  renderMessages();
}

function renderMessages() {
  const s = Store.session(state.charId, state.sessionId);
  const wrap = $('#messages');
  wrap.innerHTML = (s?.messages ?? []).map(m => {
    if (m.role === 'error') return `<div class="bubble error">${esc(m.content)}${m.retryable ? ` · <a href="#" data-retry="${m.ts}" style="color:inherit">${icon('corner-down-left', 12)} 重试</a>` : ''}</div>`;
    if (m.role === 'assistant' && m.streaming) return `<div class="bubble ai streaming"><span class="dots"><i></i><i></i><i></i></span></div>`;
    return `<div class="bubble ${m.role === 'user' ? 'user' : 'ai'}">${esc(m.content)}</div>`;
  }).join('');
  wrap.scrollTop = wrap.scrollHeight;
  stick.check(wrap);
}

/* 贴底跟随（P0-1）：上滚越 56px 解除，出现「回到底部」 */
const stick = {
  up: false,
  check(scroll) {
    const bottom = scroll.scrollTop + scroll.clientHeight >= scroll.scrollHeight - 56;
    this.up = !bottom;
    $('#float-bottom').classList.toggle('show', this.up);
  },
  follow(scroll) { if (!this.up) scroll.scrollTop = scroll.scrollHeight; },
};

async function send() {
  const input = $('#composer');
  const text = input.value.trim();
  if (!text || state.streaming) return;
  if (!requireApi()) return;
  input.value = '';
  const charId = state.charId, sessionId = state.sessionId;
  const c = Store.char(charId), s = Store.session(charId, sessionId);
  Store.pushMessage(charId, sessionId, { role: 'user', content: text });
  const aiMsg = Store.pushMessage(charId, sessionId, { role: 'assistant', content: '' });
  renderMessages();

  const ctrl = new AbortController();
  state.streaming = { charId, sessionId, ctrl };
  setComposerBusy(true);
  Sfx.play('send');
  const scroll = $('#chat-scroll');
  let firstDelta = true;
  try {
    const history = s.messages
      .filter(m => m.role !== 'error' && !(m.role === 'assistant' && !m.content))
      .slice(-16).map(m => ({ role: m.role, content: m.content }));
    const sys = `你是「${c.name}」。人设：${c.persona}\n说话风格：${c.style || '自然'}\n` +
      `当前性格特质（活跃）：${c.traits.filter(t => t.state === 'active').map(t => t.name).join('、') || '（尚未形成）'}\n保持角色一致性。`;
    aiMsg.streaming = true; renderMessages();
    await LLM.chatStream({
      ...llmOpts(), messages: [{ role: 'system', content: sys }, ...history],
      signal: ctrl.signal,
      onDelta: (d) => {
        if (firstDelta) { aiMsg.streaming = false; firstDelta = false; renderMessages(); Sfx.play('typing'); }
        aiMsg.content += d;
        if (state.streaming?.sessionId === sessionId) {
          const last = wrap_lastBubble();
          if (last) { last.textContent = aiMsg.content; stick.follow(scroll); }
        }
      },
    });
    if (!aiMsg.content) aiMsg.content = '（空回复）';
    Sfx.play('receive');
  } catch (err) {
    aiMsg.streaming = false;
    if (err.name === 'AbortError') { aiMsg.content = aiMsg.content ? aiMsg.content + '\n（已停止）' : '（已停止）'; Sfx.play('stop'); }
    else {
      aiMsg.role = 'error'; aiMsg.content = `请求失败：${err.message}`; aiMsg.retryable = true;
      Sfx.play('error');
    }
  } finally {
    aiMsg.streaming = false;            /* 标志不落盘：中断后重开不应残留点点 */
    Store.save(); state.streaming = null; setComposerBusy(false);
    renderMessages();
    if (aiMsg.role === 'assistant' && aiMsg.content) distillAsync(charId, text, aiMsg.content);
  }
}
function wrap_lastBubble() { const all = $$('#messages .bubble.ai'); return all[all.length - 1]; }
function setComposerBusy(b) {
  $('#send').style.display = b ? 'none' : '';
  $('#stop').style.display = b ? '' : 'none';
}

/* 对话后异步提炼：特质演化 + 叙事弧（失败静默降级为本地弧） */
async function distillAsync(charId, userText, aiText) {
  const c = Store.char(charId);
  if (!(Store.settings.autoDistill && Store.apiReady())) {
    Store.addArc(charId, { title: userText.slice(0, 18) || '一段对话', summary: `${userText.slice(0, 40)}…`, traits: [] });
    return;
  }
  LLM.guard(Store.settings.baseUrl);
  try {
    const out = await LLM.complete({ ...llmOpts(), messages: [
      { role: 'system', content: '从对话中提炼特质与叙事弧。只输出 JSON：{"traits":[{"name":"..."}],"arc":{"title":"≤12字","summary":"≤60字"}}' },
      { role: 'user', content: `用户：${userText}\n角色（${c.name}）回复：${aiText}` },
    ]});
    const parsed = LLM.extractJson(out);
    Store.applyTraits(charId, parsed.traits);
    Store.addArc(charId, { title: parsed.arc?.title ?? '一段对话', summary: parsed.arc?.summary ?? '', traits: (parsed.traits ?? []).map(t => t.name) });
  } catch {
    Store.addArc(charId, { title: userText.slice(0, 18) || '一段对话', summary: `${userText.slice(0, 40)}…`, traits: [] });
  }
}

/* ═══ 角色页 ═══ */
function renderChars() {
  const chars = Store.characters;
  $('#char-grid').innerHTML = chars.map((c, i) => `
    <div class="card char-card clickable" style="animation-delay:${i * 40}ms" data-open="${c.id}">
      <div class="avatar">${esc(c.emoji)}</div>
      <div class="name">${esc(c.name)}</div>
      <div class="persona">${esc(c.persona)}</div>
      <div class="traits">${c.traits.map(t => `<span class="trait ${t.state}">${esc(t.name)}</span>`).join('')}</div>
      <div class="row" style="margin-top:auto; padding-top:8px">
        <button class="btn sm" data-edit="${c.id}" data-icon="pencil">编辑</button>
        <button class="btn sm danger" data-delchar="${c.id}" data-icon="trash">删除</button>
        <span style="margin-left:auto; font-size:var(--fs-xs); color:var(--text-3)">${c.arcs.length} 段记忆</span>
      </div>
    </div>`).join('') ||
    `<div class="empty" style="grid-column:1/-1"><span data-icon="sparkles" data-icon-size="34"></span>还没有角色——铸造第一颗星。</div>`;
  injectIcons($('#char-grid'));
}

/* 铸造 / 编辑（同一模态） */
function openCharModal(char) {
  $('#modal-title').textContent = char ? '编辑角色' : '铸造角色';
  $('#f-id').value = char?.id ?? '';
  $('#f-emoji').value = char?.emoji ?? '✦';
  $('#f-name').value = char?.name ?? '';
  $('#f-persona').value = char?.persona ?? '';
  $('#f-style').value = char?.style ?? '';
  $('#f-greeting').value = char?.greeting ?? '';
  $('#forge-src').value = '';
  $('#forge-area').style.display = char ? 'none' : 'block';
  $('#char-modal').classList.add('open');
  Sfx.play('open');
  $('#f-name').focus();
}
async function forge() {
  const src = $('#forge-src').value.trim();
  if (!src) return;
  if (!requireApi()) return;
  const btn = $('#forge-btn'); btn.disabled = true; btn.textContent = '铸造中……';
  try {
    const out = await LLM.complete({ ...llmOpts(), messages: [
      { role: 'system', content: '把素材铸成角色卡。只输出 JSON：{"emoji":"一个emoji","name":"名字","persona":"人设(≤120字)","style":"说话风格(≤40字)","greeting":"开场白(≤60字)","traits":["特质",…]}' },
      { role: 'user', content: src },
    ]});
    const j = LLM.extractJson(out);
    $('#f-emoji').value = j.emoji ?? '✦'; $('#f-name').value = j.name ?? '';
    $('#f-persona').value = j.persona ?? ''; $('#f-style').value = j.style ?? '';
    $('#f-greeting').value = j.greeting ?? '';
    $('#f-id').dataset.forgedTraits = JSON.stringify(j.traits ?? []);
    Sfx.play('achievement');
    toast('铸造完成，可继续微调后保存');
  } catch (e) { Sfx.play('error'); toast('铸造失败：' + e.message + '（可手动填写）', true); }
  finally { btn.disabled = false; btn.innerHTML = icon('wand-sparkles', 15) + ' 铸造'; }
}
function saveChar() {
  const fields = {
    emoji: $('#f-emoji').value.trim() || '✦', name: $('#f-name').value.trim(),
    persona: $('#f-persona').value.trim(), style: $('#f-style').value.trim(), greeting: $('#f-greeting').value.trim(),
  };
  if (!fields.name) { toast('名字不能为空', true); return; }
  const id = $('#f-id').value;
  if (id) Store.updateCharacter(id, fields);
  else {
    const c = Store.addCharacter(fields);
    state.charId = c.id;
    try { Store.applyTraits(c.id, JSON.parse($('#f-id').dataset.forgedTraits || '[]')); } catch { /* 无铸造特质 */ }
  }
  $('#char-modal').classList.remove('open');
  renderChars(); toast(id ? '已保存' : '角色已铸造');
}

/* ═══ 领地页 ═══ */
function renderTerritory() {
  const chars = Store.characters;
  if (!state.tlFocus || !Store.char(state.tlFocus)) state.tlFocus = chars[0]?.id ?? null;
  $('#residents').innerHTML = chars.map(c => `
    <div class="resident ${c.id === state.tlFocus ? '' : ''}" data-focus="${c.id}"
      style="${c.id === state.tlFocus ? 'border-color:var(--border-strong); box-shadow:var(--glow)' : ''}">
      <span>${esc(c.emoji)}</span><span style="font-size:var(--fs-sm)">${esc(c.name)}</span>
    </div>`).join('') || '<span style="color:var(--text-3); font-size:var(--fs-sm)">领地空空如也。</span>';
  const c = Store.char(state.tlFocus);
  $('#tl-banner').style.display = Store.apiReady() ? 'none' : 'flex';
  $('#generate-tl').disabled = !Store.apiReady() || !c;
  $('#timeline').innerHTML = c ? c.timeline.map(e => `
    <div class="tl-entry"><div class="who">${esc(c.emoji)} ${esc(c.name)} · ${fmtTs(e.ts)}</div>
      <div class="what">${esc(e.text)}</div></div>`).join('') ||
    '<div class="empty"><div class="glyph">🏡</div>还没有生活记录——生成一段近况。</div>' :
    '<div class="empty"></div>';
}
async function generateTimeline() {
  const c = Store.char(state.tlFocus);
  if (!c) return;
  if (!requireApi()) return;
  const btn = $('#generate-tl'); btn.disabled = true; btn.textContent = '生成中……';
  try {
    const recent = (Store.session(c.id, c.sessions[0]?.id)?.messages ?? []).slice(-6)
      .map(m => `${m.role === 'user' ? '主人' : c.name}：${m.content.slice(0, 60)}`).join('\n');
    const out = await LLM.complete({ ...llmOpts(), messages: [
      { role: 'system', content: `你是「${c.name}」（人设：${c.persona}）。写一段主人离开期间的生活近况，60-120 字，具体、有画面、符合人设。` },
      { role: 'user', content: `最近的对话记忆：\n${recent || '（还没有对话）'}\n\n已发生的故事：${c.arcs.slice(0, 3).map(a => a.title).join('、') || '（无）'}` },
    ]});
    Store.addTimeline(c.id, out.trim());
    renderTerritory(); toast('近况已生成');
  } catch (e) { toast('生成失败：' + e.message, true); }
  finally { btn.disabled = false; btn.textContent = '生成近况'; }
}

/* ═══ 星图页 ═══ */
function renderStarmap() {
  if (!state.charId || !Store.char(state.charId)) state.charId = Store.characters[0]?.id ?? null;
  if (!state.starmap) state.starmap = new Starmap($('#starmap'), pickNodeDetail);
  const c = Store.char(state.charId);
  if (!c) { state.starmap.setData([], []); showNodeDetail(null); return; }
  const nodes = [
    ...c.traits.map(t => ({ id: 't:' + t.name, kind: 'trait', label: t.name, weight: t.weight, state: t.state })),
    ...c.arcs.slice(0, 24).map(a => ({ id: 'a:' + a.id, kind: 'memory', label: a.title, weight: 2, state: 'active' })),
  ];
  const edges = c.arcs.slice(0, 24).flatMap(a => (a.traits ?? []).map(t => ['t:' + t, 'a:' + a.id]));
  state.starmap.setData(nodes, edges);
  showNodeDetail(null);
}
function pickNodeDetail(node) {
  const el = $('#node-detail');
  if (!node) { el.classList.remove('show'); return; }
  const c = Store.char(state.charId);
  const arc = node.kind === 'memory' ? c.arcs.find(a => 'a:' + a.id === node.id) : null;
  const trait = node.kind === 'trait' ? c.traits.find(t => 't:' + t.name === node.id) : null;
  el.innerHTML = `
    <div class="kind">${node.kind === 'trait' ? '特质节点' : '记忆节点（叙事弧）'}</div>
    <div style="font-size:var(--fs-lg); margin:6px 0">${esc(node.label)}</div>
    <div style="font-size:var(--fs-sm); color:var(--text-2); line-height:1.7">
      ${arc ? esc(arc.summary) : ''}
      ${trait ? `权重 ${trait.weight}/5 · ${trait.state === 'dormant' ? '休眠中' : '活跃'} · 最近活跃 ${fmtTs(trait.lastActive)}` : ''}
    </div>`;
  el.classList.add('show');
}
const showNodeDetail = pickNodeDetail;

/* ═══ 设置页 ═══ */
function renderSettings() {
  const s = Store.settings;
  $('#s-base').value = s.baseUrl; $('#s-key').value = s.apiKey; $('#s-model').value = s.model;
  $('#s-distill').checked = !!s.autoDistill;
  $('#conn-result').textContent = '';
}
async function testConn() {
  const el = $('#conn-result'); el.textContent = '测试中……'; el.className = '';
  try {
    LLM.guard($('#s-base').value);
    await LLM.testConnection({ baseUrl: $('#s-base').value, apiKey: $('#s-key').value, model: $('#s-model').value });
    el.textContent = '✓ 连接成功'; el.className = 'conn-ok';
  } catch (e) { el.textContent = '✗ ' + e.message; el.className = 'conn-bad'; }
}

/* ═─ 事件绑定 ─═ */
function bind() {
  injectIcons();
  renderSfxToggle();
  $$('.nav-item').forEach(n => n.addEventListener('click', () => {
    const target = '#/' + n.dataset.page;
    if (location.hash === target) route();   /* 同页重点 = 强制刷新 */
    else location.hash = target;
  }));
  /* 键盘可达：导航支持 Enter/Space */
  $$('.nav-item').forEach(n => n.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); n.click(); }
  }));
  $('#sfx-toggle').addEventListener('click', () => {
    Sfx.setMuted(!Sfx.muted()); renderSfxToggle();
    if (!Sfx.muted()) Sfx.play('check');
  });
  $('#api-chip').addEventListener('click', () => {
    if (!Store.apiReady()) { location.hash = '#/settings'; return; }
    location.hash = '#/settings';   /* 已配置也允许去设置页查看 */
  });
  window.addEventListener('hashchange', route);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      if ($('#char-modal').classList.contains('open')) { $('#char-modal').classList.remove('open'); return; }
      if (state.streaming) state.streaming.ctrl.abort();
    }
    if (e.ctrlKey && e.key >= '1' && e.key <= '5') { location.hash = '#/' + routes[e.key - 1]; }
  });

  /* 对话 */
  $('#chat-scroll').addEventListener('scroll', (e) => stick.check(e.target));
  $('#float-bottom').addEventListener('click', () => { const s = $('#chat-scroll'); stick.up = false; s.scrollTop = s.scrollHeight; stick.check(s); });
  $('#composer').addEventListener('input', (e) => {   /* 自动增高 */
    const t = e.target;
    t.style.height = 'auto';
    t.style.height = Math.min(t.scrollHeight, 160) + 'px';
  });
  $('#composer').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); }
  });
  $('#send').addEventListener('click', send);
  $('#stop').addEventListener('click', () => state.streaming?.ctrl.abort());
  $('#new-session').addEventListener('click', () => {
    if (!Store.char(state.charId)) return;
    state.sessionId = Store.newSession(state.charId).id; renderChat();
  });
  $('#sessions').addEventListener('click', (e) => {
    const del = e.target.closest('[data-del]');
    if (del) {
      const sid = del.dataset.del;
      if (state.streaming?.sessionId === sid) { toast('流式生成中，稍后再删', true); return; }
      const s = Store.session(state.charId, sid);
      if (s.messages.length && !confirm('删除该会话及其全部消息？')) return;
      Store.removeSession(state.charId, sid);
      if (state.sessionId === sid) state.sessionId = null;
      Sfx.play('delete');
      renderChat(); closeDrawer(); return;
    }
    const item = e.target.closest('[data-sid]');
    /* P0-2：流式中也允许切换——流继续写入其会话，回来时从存储重载 */
    if (item) { state.sessionId = item.dataset.sid; renderChat(); closeDrawer(); }
  });
  $('#chat-chars').addEventListener('click', (e) => {
    const item = e.target.closest('[data-cid]');
    if (item) { state.charId = item.dataset.cid; state.sessionId = null; renderChat(); closeDrawer(); }
  });
  $('#messages').addEventListener('click', (e) => {
    const r = e.target.closest('[data-retry]');
    if (!r) return;
    const s = Store.session(state.charId, state.sessionId);
    const idx = s.messages.findIndex(m => m.ts === +r.dataset.retry);
    if (idx > 0 && s.messages[idx - 1].role === 'user') {
      const userText = s.messages[idx - 1].content;
      s.messages.splice(idx - 1, 2); Store.save();   /* 宁少删不误删：按相邻成对删除 */
      renderMessages();
      $('#composer').value = userText; send();
    }
  });

  /* 角色 */
  $('#add-char').addEventListener('click', () => openCharModal(null));
  $('#char-grid').addEventListener('click', (e) => {
    const ed = e.target.closest('[data-edit]'); if (ed) { openCharModal(Store.char(ed.dataset.edit)); return; }
    const dc = e.target.closest('[data-delchar]');
    if (dc) { if (confirm('删除该角色及其全部数据？')) { Store.removeCharacter(dc.dataset.delchar); Sfx.play('delete'); renderChars(); toast('已删除'); } return; }
    const open = e.target.closest('[data-open]');
    if (open) { state.charId = open.dataset.open; location.hash = '#/chat'; }
  });
  $('#forge-btn').addEventListener('click', forge);
  $('#char-save').addEventListener('click', saveChar);
  $('#char-cancel').addEventListener('click', () => $('#char-modal').classList.remove('open'));

  /* 领地 */
  $('#residents').addEventListener('click', (e) => {
    const r = e.target.closest('[data-focus]');
    if (r) { state.tlFocus = r.dataset.focus; renderTerritory(); }
  });
  $('#generate-tl').addEventListener('click', generateTimeline);

  /* 设置 */
  $('#s-save').addEventListener('click', () => {
    Store.saveSettings({ baseUrl: $('#s-base').value, apiKey: $('#s-key').value, model: $('#s-model').value, autoDistill: $('#s-distill').checked });
    toast('已保存'); Sfx.play('check'); renderChat();
  });
  $('#s-test').addEventListener('click', testConn);
  $('#s-export').addEventListener('click', () => {
    const blob = new Blob([Store.exportAll()], { type: 'application/json' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob); a.download = `ling-backup-${Date.now()}.json`; a.click();
    URL.revokeObjectURL(a.href);
  });
  $('#s-import-file').addEventListener('change', (e) => {
    const f = e.target.files[0]; if (!f) return;
    f.text().then(t => { Store.importAll(t); Sfx.play('check'); toast('导入成功'); route(); })
      .catch(err => toast('导入失败：' + err.message, true));
    e.target.value = '';
  });
  $('#s-clear').addEventListener('click', () => {
    if (confirm('清空全部本地数据？此操作不可恢复。')) { Store.clearAll(); Sfx.play('delete'); route(); toast('已清空'); }
  });
}

/* ── 侧栏形变开关（morphicons 式）：唯一按钮附着侧栏右上角,
   收起后滑到屏幕左上; 图标 panel-left ⇄ x 随状态交叉形变 ── */
const isNarrow = () => window.matchMedia('(max-width: 850px)').matches;
function closeDrawer() { sideOpen = false; document.body.classList.remove('side-open'); }
/* 侧栏开合的显式唯一真源(跨模式共享); 类只是按模式重放的结果, 禁止反推 */
let sideOpen = !window.matchMedia('(max-width: 850px)').matches;   /* 桌面默认开/手机默认关 */
function sidebarExpanded() { return sideOpen; }
function updateToggleMode() {
  const b = $('#sidenav-toggle');
  const mode = sidebarExpanded() ? 'collapse' : 'expand';
  b.dataset.mode = mode;
  b.setAttribute('aria-label', mode === 'collapse' ? '收起侧边栏' : '展开侧边栏');
}
function bindDrawer() {
  const b = $('#sidenav-toggle');
  b.innerHTML = `<span class="morph-ic ic-x">${icon('x', 16)}</span>`
              + `<span class="morph-ic ic-panel">${icon('panel-left', 16)}</span>`;
  b.addEventListener('click', () => {
    const wasExpanded = sideOpen;
    sideOpen = !sideOpen;
    applySidebarState();
    updateToggleMode();
    Sfx.play(wasExpanded ? 'close' : 'open');
  });
  $('#scrim').addEventListener('click', closeDrawer);
  /* 跨断点映射: 幂等 apply —— 按当前模式把 sideOpen 重放为对应类。
     mq change(断点翻转精确触发) + resize(自愈: 任意 resize 都重放, 漏掉的翻转
     会被下一次 resize 修正; 类不再被反推, 无 clean 歧义)。 */
  const applySidebarState = () => {
    if (isNarrow()) {
      document.body.classList.toggle('side-open', sideOpen);
      document.body.classList.remove('side-closed');
    } else {
      document.body.classList.toggle('side-closed', !sideOpen);
      document.body.classList.remove('side-open');
    }
  };
  window.applySidebarState = applySidebarState;          /* 调试暴露 */
  const mqNarrow = window.matchMedia('(max-width: 850px)');
  const onBreakpoint = () => { applySidebarState(); updateToggleMode(); };
  if (mqNarrow.addEventListener) mqNarrow.addEventListener('change', onBreakpoint);
  else mqNarrow.addListener(onBreakpoint);
  window.addEventListener('resize', onBreakpoint);       /* 自愈通道 */
  applySidebarState();
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && isNarrow()) closeDrawer(); });
  updateToggleMode();
}

/* ── 边缘近邻发光（仅电脑精确指针; 移动设备零开销跳过） ──
   光标靠近组件边缘 → 最近点处的边缘环带发亮(参考 Windows 图标近邻光带)。
   JS 只算几何并写 --gx/--gy/--glow-o 三个变量, 视觉全部在 CSS ::after。 ── */
const EdgeGlow = (() => {
  const fine = window.matchMedia('(hover: hover) and (pointer: fine)');
  const SELECTOR = '.card, .session-item, .composer-shell, .resident, .float-bottom, '
    + '.btn, .iconbtn, .bubble, .node-detail, .banner, .api-chip, .side-foot .nav-item';
  const REACH = 90;                 // 感应半径(px), 距边缘 90px 内开始渐亮
  let targets = [], raf = 0, mx = -1e4, my = -1e4;

  function collect() {
    targets = fine.matches ? [...document.querySelectorAll(SELECTOR)] : [];
    targets.forEach(el => el.classList.add('glow-edge'));
  }
  function update() {
    raf = 0;
    for (const el of targets) {
      const r = el.getBoundingClientRect();
      if (r.width === 0) continue;
      const nx = Math.max(r.left, Math.min(mx, r.right));   // 光标在矩形上的最近点
      const ny = Math.max(r.top, Math.min(my, r.bottom));
      const d = Math.hypot(mx - nx, my - ny);
      el.style.setProperty('--gx', (nx - r.left).toFixed(1) + 'px');
      el.style.setProperty('--gy', (ny - r.top).toFixed(1) + 'px');
      el.style.setProperty('--glow-o', Math.max(0, 1 - d / REACH).toFixed(3));
    }
  }
  function onMove(e) {
    mx = e.clientX; my = e.clientY;
    if (!raf && targets.length) raf = requestAnimationFrame(update);
  }
  fine.addEventListener('change', collect);
  window.addEventListener('resize', collect);
  document.addEventListener('pointermove', onMove, { passive: true });
  collect();
  return { collect };
})();

bind();
bindDrawer();
route();
