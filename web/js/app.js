/* ling · 应用逻辑：路由 + 五页面 + 流式对话 + 铸造 + 领地 + 星图 + 设置
   UX 审计 P0 修复模式：贴底跟随、流式中切会话不丢、横幅行动接线、键盘可用 */
const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];
const esc = (s) => { const d = document.createElement('div'); d.textContent = String(s ?? ''); return d.innerHTML; };
const fmtTs = (ts) => new Date(ts).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });

const state = { charId: null, sessionId: null, streaming: null, starmap: null, worldMap: null, tlFocus: null, worldView: null, kbTab: 'char' };

/* ── 图标注入：所有 [data-icon] 元素插入内联 SVG（动态渲染后需再调用） ── */
function injectIcons(root = document) {
  $$('[data-icon]', root).forEach(el => {
    if (el.dataset.iconDone) return;
    el.insertAdjacentHTML('afterbegin', icon(el.dataset.icon, +(el.dataset.iconSize || 15)));
    el.dataset.iconDone = '1';
  });
}

/* ── Toast（普通 4s，错误 6s；入场/退场统一走 Motion 引擎；带语义图标） ── */
function toast(msg, isError = false) {
  const el = document.createElement('div');
  el.className = 'toast' + (isError ? ' error' : '');
  el.innerHTML = icon(isError ? 'x' : 'check', 14) + '<span style="margin-left:8px">' + esc(msg) + '</span>';
  el.style.display = 'flex'; el.style.alignItems = 'center';
  $('#toasts').appendChild(el);
  Motion.motion(el, { y: 0, opacity: 1, speed: 'fast' }, { from: { y: 8, opacity: 0 } });
  setTimeout(() => {
    Motion.motion(el, { y: 8, opacity: 0, speed: 'fast' }).finished.then(() => el.remove());
  }, isError ? 6000 : 4000);
}

/* ── 统一入场（Motion 引擎，js/motion.js）：替代全部 CSS rise/pop 关键帧 ──
   级联间隔 = --stagger 40ms；超过 12 项封顶, 长列表不拖沓;
   页面滑动过渡期间直接就位(页面本身在动, 抑制列表双重动画) */
const rise = (el, i = 0) => {
  if (pageSliding) return;
  Motion.motion(el, { y: 0, opacity: 1, speed: 'med' }, { from: { y: 6, opacity: 0 }, delay: Math.min(i, 12) * 40 });
};
const requireApi = () => {
  if (Store.apiReady()) { LLM.guard(Store.settings.baseUrl); return true; }
  toast('尚未配置模型 API，请先到设置页完成配置', true); location.hash = '#/settings'; return false;
};
const llmOpts = () => ({ ...Store.settings });

/* ── 路由 ── */
const routes = ['chat', 'kb', 'territory', 'starmap', 'settings'];   /* 顺序即过渡方向轴 */
let booted = false;
let pageSliding = false;     /* 页面滑动中抑制列表级联入场, 避免双重动画 */

/* 旧页冻结为像素级覆盖层: 按当前流式位置绝对定位, 保持视觉不动、脱离布局 */
function ghost(el) {
  const m = document.querySelector('main').getBoundingClientRect();
  const r = el.getBoundingClientRect();
  Object.assign(el.style, {
    position: 'absolute', zIndex: 2,
    left: (r.left - m.left) + 'px', top: (r.top - m.top) + 'px',
    width: r.width + 'px', height: r.height + 'px',
  });
  el.classList.add('page-ghost');
}
/* 幽灵还原为干净隐藏态(退场完成, 或被快速切回) */
function unghost(el) {
  el.getAnimations().forEach(a => a.cancel());
  el.classList.remove('page-ghost');
  el.style.position = el.style.left = el.style.top = el.style.width =
    el.style.height = el.style.zIndex = el.style.transform = el.style.opacity = '';
}

function route() {
  const parts = (location.hash.replace('#/', '') || 'chat').split('?')[0].split('/');
  let page = parts[0];
  if (page === 'characters') { page = 'kb'; parts[1] = parts[1] || 'char'; }   /* 旧链接兼容 */
  if (page === 'worlds') { page = 'kb'; parts[1] = parts[1] || 'world'; }
  if (!routes.includes(page)) page = 'chat';
  if (page === 'kb') state.kbTab = parts[1] === 'world' ? 'world' : 'char';
  const prev = document.querySelector('.page.active');
  const next = document.querySelector(`.page[data-page="${page}"]`);
  const sliding = booted && prev && prev !== next;      /* 首次进入/同页刷新: 不滑 */
  if (next.classList.contains('page-ghost')) unghost(next);   /* 快速来回切: 新页可能是退场中的幽灵 */
  pageSliding = sliding;

  $$('.nav-item:not(.sub)').forEach(n => n.classList.toggle('active', n.dataset.page === page));
  document.querySelector('.nav').classList.toggle('kb-open', page === 'kb');
  if (booted) Sfx.play('select');
  if (sliding) ghost(prev);                             /* 先冻结旧页(还在流内), 再切 active */

  $$('.page').forEach(p => p.classList.toggle('active', p.dataset.page === page));
  ({ chat: renderChat, kb: renderKB, territory: renderTerritory, starmap: renderStarmap, settings: renderSettings })[page]();
  EdgeGlow.collect();                 /* 页面重渲染后重新收集发光目标 */

  if (sliding) {
    const dir = routes.indexOf(page) > routes.indexOf(prev.dataset.page) ? 1 : -1;
    Motion.motion(prev, { x: -28 * dir, opacity: 0, speed: 'fast' }).finished.then(() => unghost(prev));
    Motion.motion(next, { x: 0, opacity: 1, speed: 'med' }, { from: { x: 28 * dir, opacity: 0 } })
      .finished.then(() => { pageSliding = false; });
  } else {
    pageSliding = false;
    if (booted && prev === next) { /* 同页重复路由, 已重渲染 */ }
    else Motion.motion(next, { y: 0, opacity: 1, speed: 'med' }, { from: { y: 6, opacity: 0 } });
  }
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
  if (!c) { $('#char-list-empty').innerHTML = '<div class="glyph">🎭</div>还没有角色。<a href="#/kb/char">去铸造 →</a>'; return; }
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
  document.querySelectorAll('#sessions .session-item, #chat-chars .session-item').forEach(rise);
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
  [...wrap.children].forEach(el => rise(el));   /* 消息体量不定, 不级联 */
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
  sizeComposer(input);                              /* 清空后动画回收高度 */
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
  $('#char-grid').innerHTML = chars.map(c => `
    <div class="card char-card clickable ${c.id === state.charId ? 'current' : ''}" data-open="${c.id}">
      <div class="avatar">${esc(c.emoji)}</div>
      <div class="name">${esc(c.name)}</div>
      <div class="persona">${esc(c.persona)}</div>
      <div class="traits">${c.traits.map(t => `<span class="trait ${t.state}">${esc(t.name)}</span>`).join('')}</div>
      <div class="row" style="margin-top:auto; padding-top:8px">
        <button class="btn sm" data-edit="${c.id}" data-icon="pencil" title="编辑"><span class="bl">编辑</span></button>
        <button class="btn sm danger" data-delchar="${c.id}" data-icon="trash" title="删除"><span class="bl">删除</span></button>
        <span style="margin-left:auto; font-size:var(--fs-xs); color:var(--text-3)">${c.arcs.length} 段记忆</span>
      </div>
    </div>`).join('') ||
    `<div class="empty" style="grid-column:1/-1"><span data-icon="sparkles" data-icon-size="34"></span>还没有角色——铸造第一颗星。</div>`;
  injectIcons($('#char-grid'));
  document.querySelectorAll('#char-grid .char-card').forEach(rise);
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
  Motion.motion($('#char-modal .modal'), { size: 1, opacity: 1, speed: 'slow' }, { from: { size: 0.96, opacity: 0 } });
  Sfx.play('open');
  $('#f-name').focus();
}
/* ═─ 铸造核心（角色/世界 · 双模式共用）══ */
const pickedFiles = { c: [], w: [] };           /* 各模态已选文件 */
async function collectFiles(k) {
  if (!pickedFiles[k].length) return '';
  const docs = await FileKit.readSettingFiles(pickedFiles[k]);
  const errs = docs.filter(d => d.error);
  if (errs.length) toast(errs.map(e => e.name).join('、') + ' 不支持，已跳过', true);
  return docs.filter(d => d.text).map(d => `【${d.name}】\n${d.text.slice(0, 12000)}`).join('\n\n').slice(0, 50000);
}
async function forgeCharFrom(src, btn) {
  if (!requireApi()) return;
  const prev = btn?.textContent; if (btn) { btn.disabled = true; btn.textContent = '铸造中……'; }
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
  finally { if (btn) { btn.disabled = false; btn.textContent = prev; } }
}
async function forge() {                         /* 从已有设定：文件 + 粘贴 */
  const files = await collectFiles('c').catch(e => { toast('读取文件失败：' + e.message, true); return ''; });
  const src = [files, $('#forge-src').value.trim()].filter(Boolean).join('\n\n');
  if (!src) { toast('先选择文件或粘贴素材', true); return; }
  await forgeCharFrom(src, $('#forge-btn'));
}
async function worldForgeFrom(material, btn, isSeed) {
  if (!requireApi()) return;
  const prev = btn?.textContent; if (btn) { btn.disabled = true; btn.textContent = '铸造中……'; }
  try {
    LLM.guard(Store.settings.baseUrl);
    const out = await LLM.complete({
      ...llmOpts(),
      messages: [{ role: 'user', content:
        (isSeed ? '这是一句种子描述，请据此创作一个基本世界观（实体 6~10 个）。\n' : '') +
        '从下面的世界观设定中提取世界星图实体。只输出 JSON，不要多余文字。格式：\n' +
        '{"name":"世界名","intro":"50字内简介","nodes":[{"label":"实体名","kind":"大陆|组织|概念|事件|人物","weight":1-5,"desc":"20字内描述"}],"edges":[["实体A","实体B"]]}\n' +
        '提取 12~22 个最重要的实体（大陆/核心组织/关键概念/重大事件/主角），关系边 15~30 条。\n\n' + material.slice(0, 60000) }],
    });
    const parsed = LLM.extractJson(out);
    const built = applyParsedWorld(parsed,
      $('#w-name').value.trim() || parsed.name || '新世界',
      $('#w-emoji').value.trim() || '🌐',
      $('#w-intro').value.trim() || parsed.intro || '');
    $('#w-name').value = built.name; $('#w-emoji').value = built.emoji; $('#w-intro').value = built.intro;
    window.__parsedWorld = built;                 /* 保存按钮随取 */
    Sfx.play('achievement');
    toast(`铸造出 ${built.nodes.length} 实体 · ${built.edges.length} 关系`);
  } catch (e) { toast('铸造失败：' + e.message, true); }
  finally { if (btn) { btn.disabled = false; btn.textContent = prev; } }
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
  document.querySelectorAll('#timeline .tl-entry').forEach(rise);
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
let ndSeq = 0;                                 /* 节点面板开/关时序令牌 */
function pickNodeDetail(node) {
  const el = $('#node-detail');
  if (!node) {
    if (!el.classList.contains('show')) return;
    const my = ++ndSeq;                       /* 快速重开保护: 迟到的隐藏回调不再生效 */
    Motion.motion(el, { y: 8, opacity: 0, speed: 'fast' }).finished.then(() => {
      if (my !== ndSeq) return;
      el.classList.remove('show');
    });
    return;
  }
  ++ndSeq;                                    /* 使在飞的隐藏回调失效 */
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
  /* 统一变换引擎入场: 落定 y8→0 + 淡入(替代 CSS rise 关键帧, 曲线与全应用同源) */
  Motion.motion(el, { y: 0, opacity: 1, speed: 'fast' }, { from: { y: 8, opacity: 0 } });
}
const showNodeDetail = pickNodeDetail;

/* ── 轻量 Markdown 渲染(标题/粗斜/代码/列表/引用/链接/分隔线) ── */
const mdInline = (t) => esc(t)
  .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
  .replace(/\*([^*\n]+)\*/g, '<i>$1</i>')
  .replace(/`([^`\n]+)`/g, '<code>$1</code>')
  .replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
function md(src) {
  const lines = String(src || '').split('\n');
  const out = []; let list = null, quote = false, code = false, buf = [];
  const close = () => { if (list) { out.push(`</${list}>`); list = null; } if (quote) { out.push('</blockquote>'); quote = false; } };
  for (const l of lines) {
    if (/^```/.test(l)) {
      if (code) { out.push(`<pre><code>${esc(buf.join('\n'))}</code></pre>`); buf = []; code = false; }
      else { close(); code = true; }
      continue;
    }
    if (code) { buf.push(l); continue; }
    const h = l.match(/^(#{1,4})\s+(.*)/);
    if (h) { close(); out.push(`<h${h[1].length + 2}>${mdInline(h[2])}</h${h[1].length + 2}>`); continue; }
    if (/^[-*]\s+/.test(l)) { if (list !== 'ul') { close(); out.push('<ul>'); list = 'ul'; } out.push(`<li>${mdInline(l.replace(/^[-*]\s+/, ''))}</li>`); continue; }
    if (/^\d+\.\s+/.test(l)) { if (list !== 'ol') { close(); out.push('<ol>'); list = 'ol'; } out.push(`<li>${mdInline(l.replace(/^\d+\.\s+/, ''))}</li>`); continue; }
    if (/^>\s?/.test(l)) { if (!quote) { close(); out.push('<blockquote>'); quote = true; } out.push(mdInline(l.replace(/^>\s?/, '')) + '<br>'); continue; }
    if (/^(---|\*\*\*)\s*$/.test(l)) { close(); out.push('<hr>'); continue; }
    if (!l.trim()) { close(); continue; }
    close(); out.push(`<p>${mdInline(l)}</p>`);
  }
  if (code) out.push(`<pre><code>${esc(buf.join('\n'))}</code></pre>`);
  close();
  return sanitizeRich(out.join(''));
}

/* ═══ 世界页：列表 ⇄ 世界星图详情 ═══ */
let wdSeq = 0;                                    /* 世界实体面板开合时序令牌 */
function pickWorldDetail(node) {
  const el = $('#world-detail');
  if (state.worldEdit) {                             /* 编辑模式: 点节点=进编辑器页面 */
    if (node) openWorldEditor(node);
    return;
  }
  if (!node) {
    if (!el.classList.contains('show')) return;
    const my = ++wdSeq;
    Motion.motion(el, { y: 8, opacity: 0, speed: 'fast' }).finished.then(() => {
      if (my !== wdSeq) return;
      el.classList.remove('show');
    });
    return;
  }
  ++wdSeq;
  el.innerHTML = `
    <div class="kind">${esc(node.kind === 'trait' ? '大陆' : node.kind)}节点</div>
    <div style="font-size:var(--fs-lg); margin:6px 0">${esc(node.label)}</div>
    <div class="md-body" style="font-size:var(--fs-sm); color:var(--text-2); line-height:1.7">${node.desc ? md(node.desc) : '<p>（暂无描述）</p>'}</div>`;
  el.classList.add('show');
  Motion.motion(el, { y: 0, opacity: 1, speed: 'fast' }, { from: { y: 8, opacity: 0 } });
}
function renderKB() {
  const kb = state.kbTab === 'world' ? 'world' : 'char';
  state.kbTab = kb;
  $$('.nav-item.sub').forEach(s => s.classList.toggle('active', s.dataset.kb === kb));
  const wExists = kb === 'world' && state.worldView && !!Store.world(state.worldView);
  const inEditor = wExists && state.editNodeId && !!worldNow()?.nodes.find(n => n.id === state.editNodeId);
  if (!inEditor) state.editNodeId = null;
  $('#char-pane').style.display = (!wExists && kb === 'char') ? '' : 'none';
  $('#world-pane').style.display = (!wExists && kb === 'world') ? '' : 'none';
  $('#world-view').style.display = (wExists && !inEditor) ? '' : 'none';
  $('#we-page').style.display = inEditor ? '' : 'none';
  if (kb === 'char') renderChars();
  else if (inEditor) { fillEditorFields(editNode()); }
  else renderWorlds();
  document.body.classList.toggle('editor-focus', !!inEditor);   /* 沉浸编辑: 收起顶栏 */
  if (inEditor) $('.page.active').style.transform = '';         /* 清入场残留 transform, 顶栏方可 fixed 到屏幕顶 */
  if (!inEditor) { $('#we-info')?.classList.remove('on'); $('#we-ai')?.classList.remove('on'); }
}
function renderWorlds() {
  const view = $('#world-view');
  if (!state.worldView || !Store.world(state.worldView)) {
    state.worldView = null;
    view.style.display = 'none';
    const ws = Store.worlds;
    $('#world-grid').innerHTML = ws.map(w => `
      <div class="card world-card clickable" data-openw="${w.id}">
        <div class="avatar">${esc(w.emoji)}</div>
        <div class="name">${esc(w.name)}</div>
        <div class="persona">${esc((w.intro || '').slice(0, 64))}${(w.intro || '').length > 64 ? '…' : ''}</div>
        <div class="row" style="margin-top:auto; padding-top:8px">
          <span style="font-size:var(--fs-xs); color:var(--text-3)">${w.nodes.length} 个实体 · ${w.edges.length} 条关系</span>
          <button class="btn sm danger" style="margin-left:auto" data-delw="${w.id}" data-icon="trash" title="删除世界"><span class="bl">删除</span></button>
        </div>
      </div>`).join('');
    injectIcons($('#world-grid'));
    document.querySelectorAll('#world-grid .world-card:not(.world-new)').forEach(rise);
    return;
  }
  /* 详情：世界星图 */
  view.style.display = '';
  const w = Store.world(state.worldView);
  if (!state.worldMap) state.worldMap = new Starmap($('#world-map'), pickWorldDetail);
  state.worldMap.setData(w.nodes, w.edges);
  /* 编辑模式接线: 右键/长按菜单 */
  state.worldMap.setEditMode(!!state.worldEdit);
  state.worldMap.onEditMenu = ({ x, y, node, world }) => {
    if (!state.worldEdit) return;
    showCtxMenu(x, y, node, world);
  };
  document.querySelectorAll('#edit-toolbar .seg-btn').forEach(b =>
    b.classList.toggle('active', (b.dataset.mode === 'edit') === !!state.worldEdit));
  const intro = $('#world-intro');
  intro.classList.remove('collapsed');
  intro.style.width = ''; intro.style.borderRadius = '';      /* 清折叠胶囊内联, 还原面板形态 */
  intro.innerHTML = `
    <div class="wi-head" role="button" tabindex="0" title="收起 / 展开">
      <div class="kind">世界</div>
      <div class="wi-name"><span class="wi-nt">${esc(w.emoji)} ${esc(w.name)}</span></div>
      ${icon('chevron-down', 16)}
    </div>
    <div class="wi-body"><div style="font-size:var(--fs-sm); color:var(--text-2); line-height:1.7">${esc(w.intro || '（暂无简介）')}</div></div>`;
  pickWorldDetail(null);
}

/* 设置解析结果 → 世界实体 */
function applyParsedWorld(parsed, name, emoji, intro) {
  const nodes = (parsed.nodes ?? []).map((n, i) => ({
    id: 'p' + i, kind: n.kind || '概念', label: String(n.label || '未命名'),
    weight: Math.min(5, Math.max(1, +n.weight || 2)), state: 'active', desc: String(n.desc || ''),
  }));
  const byLabel = Object.fromEntries(nodes.map(n => [n.label, n.id]));
  const edges = (parsed.edges ?? [])
    .map(([a, b]) => [byLabel[a], byLabel[b]]).filter(([a, b]) => a && b);
  return { name, emoji, intro, nodes, edges };
}

/* ═══ 世界星图编辑器（查看/编辑 · 富文本 · AI · 文件导入）═══ */
const RICH_ALLOW = new Set(['B','STRONG','I','EM','U','S','H1','H2','H3','H4','H5','UL','OL','LI','P','BR','BLOCKQUOTE','CODE','PRE','HR','A','DIV']);
function sanitizeRich(html) {                       /* 富文本白名单消毒: 剥脚本/属性, 非白名单标签解包 */
  const d = document.createElement('div');
  d.innerHTML = String(html);
  d.querySelectorAll('script,style,iframe,object,embed,link,meta,img').forEach(e => e.remove());
  d.querySelectorAll('*').forEach(el => {
    if (!RICH_ALLOW.has(el.tagName)) el.replaceWith(...el.childNodes);
    else {
      if (el.tagName === 'A') {                    /* 仅保留 http(s) href */
        const href = el.getAttribute('href') || '';
        [...el.attributes].forEach(a => el.removeAttribute(a.name));
        if (/^https?:\/\//i.test(href)) { el.setAttribute('href', href); el.target = '_blank'; el.rel = 'noopener'; }
      }
      else [...el.attributes].forEach(a => el.removeAttribute(a.name));
    }
  });
  return d.innerHTML;
}
const worldNow = () => Store.world(state.worldView);
function saveWorldGraph() {
  const w = worldNow(); if (!w) return;
  Store.updateWorld(w.id, { nodes: w.nodes, edges: w.edges });
  state.worldMap.setData(w.nodes, w.edges);         /* setData 按 id 保位置/钉住态 */
}
function editNode() { return worldNow()?.nodes.find(x => x.id === state.editNodeId); }
function openWorldEditor(n) { state.editNodeId = n.id; renderKB(); }   /* 编辑器=独立页面 */
function fillEditorFields(n) {
  if (!n) return;
  $('#we-name').value = n.label;
  $('#we-kind').value = ['大陆','组织','概念','事件','人物'].includes(n.kind) ? n.kind : (n.kind === 'trait' ? '大陆' : '概念');
  $('#we-weight').value = n.weight; $('#we-wv').textContent = n.weight;
  $('#we-desc').value = n.desc || '';                /* markdown 源码 */
  updateMdPreview();
  renderWeEdges();
}
function updateMdPreview() {
  const pv = $('#we-preview');
  if (pv) pv.innerHTML = md($('#we-desc').value);
}
function renderWeEdges() {
  const w = worldNow(), n = editNode(); if (!w || !n) return;
  const linked = new Set(); const rows = [];
  for (const [a, b] of w.edges) {
    const other = a === n.id ? b : (b === n.id ? a : null);
    if (!other || linked.has(other)) continue;
    linked.add(other);
    rows.push(`<span class="chip">${esc(w.nodes.find(x => x.id === other)?.label ?? other)} <b data-unlink="${other}" title="断开">×</b></span>`);
  }
  $('#we-edges').innerHTML = rows.join('') || '<span class="we-hint">暂无连线</span>';
  $('#we-linkto').innerHTML = w.nodes.filter(x => x.id !== n.id && !linked.has(x.id))
    .map(x => `<option value="${x.id}">${esc(x.label)}</option>`).join('') || '<option value="">（无可用节点）</option>';
}
function closeWorldEditor() { state.editNodeId = null; renderKB(); }   /* 返回星图视图 */
async function aiJson(prompt, btn) {
  if (!requireApi()) return null;
  LLM.guard(Store.settings.baseUrl);
  const prev = btn?.textContent; if (btn) { btn.disabled = true; btn.textContent = '思考中…'; }
  try {
    const out = await LLM.complete({ ...llmOpts(), messages: [{ role: 'user', content: prompt }] });
    return LLM.extractJson(out);
  } catch (e) { Sfx.play('error'); toast('AI 失败：' + e.message, true); return null; }
  finally { if (btn) { btn.disabled = false; btn.textContent = prev; } }
}
/* 合并 AI 结果({nodes,edges} 按标签) 到当前世界, 返回新增计数 */
function mergeAiGraph(parsed) {
  const w = worldNow(); if (!w || !parsed) return { added: 0, edges: 0 };
  const byLabel = Object.fromEntries(w.nodes.map(n => [n.label, n.id]));
  let added = 0;
  for (const n of parsed.nodes ?? []) {
    const label = String(n.label ?? '').trim(); if (!label || byLabel[label]) continue;
    const id = 'e' + Date.now().toString(36) + (added++).toString(36);
    byLabel[label] = id;
    w.nodes.push({ id, kind: n.kind || '概念', label, weight: Math.min(5, Math.max(1, +n.weight || 2)), state: 'active',
      desc: String(n.desc ?? '') });                     /* 存 markdown 源码, 渲染时经 md() 消毒 */
  }
  let ec = 0;
  const seen = new Set(w.edges.map(([a, b]) => a < b ? a + '|' + b : b + '|' + a));
  for (let [a, b] of parsed.edges ?? []) {
    const ia = byLabel[String(a)], ib = byLabel[String(b)];
    if (!ia || !ib || ia === ib) continue;
    const key = ia < ib ? ia + '|' + ib : ib + '|' + ia;
    if (seen.has(key)) continue;
    seen.add(key); w.edges.push([ia, ib]); ec++;
  }
  saveWorldGraph();
  return { added, edges: ec };
}
const worldCtx = () => {
  const w = worldNow();
  return `世界「${w.name}」简介：${(w.intro || '').slice(0, 120)}\n现有节点：${w.nodes.map(n => n.label).join('、')}`;
};
/* ── 编辑模式: 节点增删公共函数 + 右键/长按上下文菜单 ── */
function addNodeAt(worldPos) {
  const w = worldNow(); if (!w) return null;
  const id = 'n' + Date.now().toString(36);
  w.nodes.push({ id, kind: '概念', label: '新节点', weight: 2, state: 'active', desc: '',
    x: worldPos.x, y: worldPos.y, pinned: true });
  saveWorldGraph();
  state.worldMap.selected = id;
  return w.nodes.at(-1);
}
function deleteNodeById(id) {
  const w = worldNow(), n = w?.nodes.find(x => x.id === id);
  if (!w || !n || !confirm(`删除节点「${n.label}」及其连线？`)) return false;
  w.nodes = w.nodes.filter(x => x.id !== id);
  w.edges = w.edges.filter(([a, b]) => a !== id && b !== id);
  saveWorldGraph();
  if (state.worldMap) state.worldMap.selected = null;
  Sfx.play('delete'); toast('节点已删除');
  return true;
}
let ctxTarget = null;
function showCtxMenu(x, y, node, world) {
  const m = $('#ctx-menu');
  ctxTarget = { node, world };
  m.innerHTML = node
    ? `<button data-act="edit">${icon('pencil', 14)} 编辑节点</button><button data-act="del" class="danger">${icon('trash', 14)} 删除节点</button>`
    : `<button data-act="new">${icon('square-plus', 14)} 新建空白节点</button><button data-act="relax">${icon('rotate-cw', 14)} 刷新星图</button>`;
  m.classList.add('on');
  const vw = $('#world-view').getBoundingClientRect();
  const mw = m.offsetWidth || 160, mh = m.offsetHeight || 80;
  m.style.left = Math.min(x - vw.left, vw.width - mw - 8) + 'px';
  m.style.top = Math.min(y - vw.top, vw.height - mh - 8) + 'px';
  Sfx.play('select');
}
function hideCtxMenu() { $('#ctx-menu').classList.remove('on'); ctxTarget = null; }
async function autoConnectNode(id) {                /* 新建节点后 AI 自动判断连线 */
  const n = worldNow()?.nodes.find(x => x.id === id); if (!n) return;
  const parsed = await aiJson(
    worldCtx() + `\n\n新节点「${n.label}」。判断它应与哪些现有节点连线。只输出 JSON：{"edges":[["${n.label}","现有节点名"],…]}，无合适连线输出 {"edges":[]}`);
  if (!parsed) return;
  const r = mergeAiGraph({ nodes: [], edges: parsed.edges });
  if (r.edges) toast(`AI 自动连接 ${r.edges} 条`);
}

/* 星图编辑器接线: 模式/标签页/富文本/字段/连线/AI/导入 */
function wireWorldEditor() {
  /* 模式切换 */
  $('#edit-toolbar').addEventListener('click', (e) => {
    const b = e.target.closest('.seg-btn'); if (!b) return;
    state.worldEdit = b.dataset.mode === 'edit';
    document.querySelectorAll('#edit-toolbar .seg-btn').forEach(x => x.classList.toggle('active', x === b));
    state.worldMap?.setEditMode(state.worldEdit);
    if (state.worldEdit) toast('编辑模式：点空白新建 · 拖节点定位 · 点节点编辑');
    else { closeWorldEditor(); if (state.worldMap) state.worldMap.selected = null; pickWorldDetail(null); }
    Sfx.play('select');
  });
  /* 右键/长按菜单动作 + 点击外部关闭 */
  $('#ctx-menu').addEventListener('click', (e) => {
    const b = e.target.closest('[data-act]'); if (!b || !ctxTarget) return;
    const { node, world } = ctxTarget;
    hideCtxMenu();
    if (b.dataset.act === 'new') {
      const n = addNodeAt(world);                                /* 就地创建, 不进编辑器(右键节点可编辑) */
      if (n) { toast('已新建节点，右键它可编辑'); if (Store.apiReady()) autoConnectNode(n.id); }
    }
    else if (b.dataset.act === 'relax') {
      state.worldMap.relax();                       /* 视觉侧解钉+踢一脚 */
      const wn = worldNow(); wn?.nodes.forEach(n => { n.pinned = false; });   /* store 侧持久化 */
      saveWorldGraph();
      toast('星图已刷新');
    }
    else if (b.dataset.act === 'edit' && node) openWorldEditor(node);
    else if (b.dataset.act === 'del' && node) deleteNodeById(node.id);
  });
  document.addEventListener('pointerdown', (e) => {
    if (!$('#ctx-menu').contains(e.target)) hideCtxMenu();
  }, true);

  /* 基础信息 / AI 助手 面板开合 */
  const panel = (btnSel, panelSel) => {
    $(btnSel).addEventListener('click', () => {
      const p = $(panelSel), other = panelSel === '#we-info' ? $('#we-ai') : $('#we-info');
      other.classList.remove('on');
      p.classList.toggle('on');
      Sfx.play('select');
    });
  };
  panel('#we-info-btn', '#we-info');
  panel('#we-ai-btn', '#we-ai');
  document.querySelectorAll('[data-we-close]').forEach(b => b.addEventListener('click', () => b.closest('.we-panel').classList.remove('on')));
  $('#we-back').addEventListener('click', closeWorldEditor);
  /* 富文本工具栏 */
  /* Markdown 工具栏: 包裹/前缀插入 */
  $('#we-page .we-rtbar').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-pre]'); if (!b) return;
    const ta = $('#we-desc');
    const s = ta.selectionStart, en = ta.selectionEnd;
    const sel = ta.value.slice(s, en) || b.dataset.ph || '';
    ta.setRangeText(b.dataset.pre + sel + (b.dataset.post || ''), s, en, 'end');
    ta.focus();
    ta.dispatchEvent(new Event('input', { bubbles: true }));
  });
  /* 编辑/预览切换(手机; 桌面双栏自动隐藏按钮) */
  $('#we-md-edit').addEventListener('click', () => {
    $('#we-desc').classList.remove('hide'); $('#we-preview').classList.remove('show');
    $('#we-md-edit').classList.add('on'); $('#we-md-prev').classList.remove('on');
  });
  $('#we-md-prev').addEventListener('click', () => {
    updateMdPreview();
    $('#we-desc').classList.add('hide'); $('#we-preview').classList.add('show');
    $('#we-md-prev').classList.add('on'); $('#we-md-edit').classList.remove('on');
  });
  /* 字段即时保存 */
  $('#we-name').addEventListener('input', () => { const n = editNode(); if (n) { n.label = $('#we-name').value || '未命名'; saveWorldGraph(); renderWeEdges(); } });
  $('#we-kind').addEventListener('change', () => { const n = editNode(); if (n) { n.kind = $('#we-kind').value; saveWorldGraph(); } });
  $('#we-weight').addEventListener('input', () => { const n = editNode(); if (n) { n.weight = +$('#we-weight').value; $('#we-wv').textContent = n.weight; saveWorldGraph(); } });
  let mdSaveT = 0;
  $('#we-desc').addEventListener('input', () => {           /* markdown 源码即时保存(去抖) + 预览 */
    const n = editNode(); if (!n) return;
    clearTimeout(mdSaveT);
    mdSaveT = setTimeout(() => { n.desc = $('#we-desc').value; saveWorldGraph(); }, 400);
    updateMdPreview();
  });
  /* 连线: 手动加/断 */
  $('#we-link-add').addEventListener('click', () => {
    const n = editNode(), to = $('#we-linkto').value;
    if (!n || !to || to === n.id) return;
    worldNow().edges.push([n.id, to]);
    saveWorldGraph(); renderWeEdges(); Sfx.play('check');
  });
  $('#we-edges').addEventListener('click', (e) => {
    const x = e.target.closest('[data-unlink]'); if (!x) return;
    const w = worldNow(), id = x.dataset.unlink;
    w.edges = w.edges.filter(([a, b]) => !((a === state.editNodeId && b === id) || (b === state.editNodeId && a === id)));
    saveWorldGraph(); renderWeEdges();
  });
  $('#we-delete').addEventListener('click', () => {
    const w = worldNow(), n = editNode(); if (!w || !n || !confirm(`删除节点「${n.label}」及其连线？`)) return;
    w.nodes = w.nodes.filter(x => x.id !== n.id);
    w.edges = w.edges.filter(([a, b]) => a !== n.id && b !== n.id);
    saveWorldGraph(); closeWorldEditor();
    if (state.worldMap) state.worldMap.selected = null;
    Sfx.play('delete'); toast('节点已删除');
  });
  /* AI: 润色 / 生成关联 / 自由指令 */
  $('#we-polish').addEventListener('click', async () => {
    const n = editNode(); if (!n) return;
    const plain = n.desc || '';
    const parsed = await aiJson(`润色下面这个世界设定节点的描述为流畅的中文 Markdown 简介(≤80字, 保留要点)。只输出 JSON {"desc":"..."}。\n节点：${n.label}\n描述：${plain}`,
      $('#we-polish'));
    if (parsed?.desc != null) {
      n.desc = String(parsed.desc);
      $('#we-desc').value = n.desc;
      updateMdPreview();
      saveWorldGraph(); toast('已润色');
    }
  });
  $('#we-suggest').addEventListener('click', async () => {
    const n = editNode(); if (!n) return;
    const parsed = await aiJson(
      worldCtx() + `\n\n围绕节点「${n.label}」创造 2~4 个密切相关的新节点, 并给出它们之间/与「${n.label}」及现有节点的连线。只输出 JSON：{"nodes":[{"label":"","kind":"大陆|组织|概念|事件|人物","weight":1-5,"desc":"≤20字"}],"edges":[["A","B"]]}`,
      $('#we-suggest'));
    if (parsed) { const r = mergeAiGraph(parsed); toast(`AI 生成 ${r.added} 节点 · ${r.edges} 连线`); Sfx.play('achievement'); }
  });
  $('#we-run').addEventListener('click', async () => {
    const cmd = $('#we-prompt').value.trim();
    if (!cmd) { toast('先输入指令', true); return; }
    const parsed = await aiJson(
      worldCtx() + `\n\n根据指令修改世界星图。只输出 JSON：{"nodes":[{"label":"","kind":"大陆|组织|概念|事件|人物","weight":1-5,"desc":""}],"edges":[["A","B"]],"remove":["要删除的节点名"],"disconnect":[["A","B"]]}\n（不需要的项给空数组）\n\n指令：${cmd}`,
      $('#we-run'));
    if (!parsed) return;
    const w = worldNow();
    for (const label of parsed.remove ?? []) w.nodes = w.nodes.filter(n => n.label !== label);
    for (const [a, b] of parsed.disconnect ?? [])
      w.edges = w.edges.filter(([x, y]) => !((x === a && y === b) || (x === b && y === a)));
    const r = mergeAiGraph(parsed);
    $('#we-prompt').value = '';
    toast(`已应用：+${r.added} 节点 +${r.edges} 连线${(parsed.remove ?? []).length ? ` -${parsed.remove.length} 节点` : ''}`);
    Sfx.play('achievement');
  });
  /* 导入: 文件选择/chips/分析 + 画布拖放 */
  const files = [];
  const chips = $('#we-file-chips');
  const renderChips = () => {
    chips.innerHTML = files.map((f, i) => `<span class="chip">${esc(f.name)} <b data-x="${i}" title="移除">×</b></span>`).join('');
    $('#we-import').disabled = !files.length;
  };
  const addFiles = list => { files.push(...list); renderChips(); };
  $('#we-file-btn').addEventListener('click', () => $('#we-files').click());
  $('#we-files').addEventListener('change', () => { addFiles([...$('#we-files').files]); $('#we-files').value = ''; });
  chips.addEventListener('click', (e) => { const x = e.target.closest('[data-x]'); if (x) { files.splice(+x.dataset.x, 1); renderChips(); } });
  $('#we-import').addEventListener('click', async () => {
    if (!files.length) return;
    const btn = $('#we-import'); btn.disabled = true; btn.textContent = '分析中…';
    try {
      const docs = await FileKit.readSettingFiles(files);
      const text = docs.filter(d => d.text).map(d => `【${d.name}】\n${d.text.slice(0, 12000)}`).join('\n\n').slice(0, 50000);
      if (!text) { toast('没有可读内容', true); return; }
      const parsed = await aiJson(
        worldCtx() + '\n\n分析设定文本, 提取新节点(勿与现有重复), 并给出它们之间及与现有节点的连线。只输出 JSON：{"nodes":[{"label":"","kind":"大陆|组织|概念|事件|人物","weight":1-5,"desc":"≤20字"}],"edges":[["A","B"]]}.\n\n' + text);
      if (parsed) { const r = mergeAiGraph(parsed); files.length = 0; renderChips(); toast(`导入 ${r.added} 节点 · ${r.edges} 连线`); Sfx.play('achievement'); }
    } catch (e) { toast('读取失败：' + e.message, true); }
    finally { btn.disabled = false; btn.textContent = '分析并导入节点'; }
  });
  /* 画布拖放(编辑模式): 显示遮罩, 松手直接分析导入 */
  const view = $('#world-view'), veil = $('#drop-veil');
  let dragDepth = 0;
  view.addEventListener('dragover', (e) => { if (state.worldEdit && e.dataTransfer?.types?.includes('Files')) e.preventDefault(); });
  view.addEventListener('dragenter', (e) => {
    if (!state.worldEdit || ![...(e.dataTransfer?.types ?? [])].includes('Files')) return;
    e.preventDefault(); dragDepth++; veil.classList.add('on');
  });
  view.addEventListener('dragleave', () => { if (--dragDepth <= 0) { dragDepth = 0; veil.classList.remove('on'); } });
  view.addEventListener('drop', async (e) => {
    if (!state.worldEdit) return;
    e.preventDefault(); dragDepth = 0; veil.classList.remove('on');
    addFiles([...(e.dataTransfer?.files ?? [])]);
    if (files.length) toast('已加入导入队列：打开任意节点的编辑器 →「导入」页分析');
  });
}

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
  $$('.nav-item:not(.sub)').forEach(n => n.addEventListener('click', () => {
    const target = '#/' + n.dataset.page;
    if (location.hash === target) route();   /* 同页重点 = 强制刷新 */
    else location.hash = target;
  }));
  /* 知识库二级胶囊: 角色 / 世界 */
  $$('.nav-item.sub').forEach(n => n.addEventListener('click', () => {
    const target = '#/kb/' + n.dataset.kb;
    if (location.hash === target) route();
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
  /* 自动增高（统一走 Motion）: auto 态测量解锁收缩, 归位后动画到目标高 */
  const sizeComposer = (t) => {
    const prev = t.style.height;
    t.style.height = 'auto';
    const target = Math.min(t.scrollHeight, 160);
    t.style.height = prev;
    Motion.motion(t, { height: target, speed: 'fast' });
  };
  $('#composer').addEventListener('input', (e) => sizeComposer(e.target));
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
    if (open) {                                            /* 点卡片: 已是当前角色→查看星图; 其他→仅切换不跳转 */
      const id = open.dataset.open;
      if (state.charId === id) { location.hash = '#/starmap'; }
      else {
        state.charId = id;
        document.querySelectorAll('#char-grid .char-card').forEach(c => c.classList.toggle('current', c.dataset.open === id));
        toast('当前角色：' + (Store.char(id)?.name ?? id));
        Sfx.play('select');
      }
    }
  });
  $('#forge-btn').addEventListener('click', forge);
  $('#char-save').addEventListener('click', saveChar);
  $('#char-cancel').addEventListener('click', () => $('#char-modal').classList.remove('open'));

  /* 世界 */
  $('#add-world').addEventListener('click', () => { $('#world-modal').classList.add('open'); Sfx.play('open'); });
  $('#world-grid').addEventListener('click', (e) => {
    const dw = e.target.closest('[data-delw]');
    if (dw) {
      if (confirm('删除该世界？')) { Store.removeWorld(dw.dataset.delw); Sfx.play('delete'); renderKB(); toast('世界已删除'); }
      return;
    }
    const ow = e.target.closest('[data-openw]');
    if (ow) { state.worldView = ow.dataset.openw; renderKB(); Sfx.play('select'); }
  });
  $('#world-back').addEventListener('click', () => { state.worldView = null; pickWorldDetail(null); renderKB(); });
  $('#world-intro').addEventListener('click', (e) => {          /* 简介面板: 头部点击折叠(胶囊化)/展开 */
    if (!e.target.closest('.wi-head')) return;
    const el = $('#world-intro');
    const goingCollapsed = !el.classList.contains('collapsed');
    el.classList.toggle('collapsed', goingCollapsed);
    if (goingCollapsed) {
      const head = el.querySelector('.wi-head');
      /* 真内容宽: 量不拉伸的文本 span(wi-name 有 flex:1 被拉满, 不可直接量) */
      const kids = [...head.children].map(c => c.classList.contains('wi-name') ? c.querySelector('.wi-nt') : c);
      const gap = kids.length > 1 ? parseFloat(getComputedStyle(head).columnGap || getComputedStyle(head).gap) || 8 : 0;
      const w = Math.ceil(kids.reduce((s, c) => s + c.getBoundingClientRect().width, 0) + gap * (kids.length - 1)) + 36 + 2;
      Motion.motion(el, { width: w, radius: 999, speed: 'fast' });
    } else {
      Motion.motion(el, { width: Math.min(340, innerWidth - 32), radius: 24, speed: 'fast' });
    }
  });
  $('#world-cancel').addEventListener('click', () => $('#world-modal').classList.remove('open'));
  $('#world-save').addEventListener('click', () => {
    const name = $('#w-name').value.trim();
    if (!name) { toast('世界需要一个名字', true); return; }
    const parsed = window.__parsedWorld;
    const w = Store.addWorld({
      name, emoji: $('#w-emoji').value.trim() || '🌐', intro: $('#w-intro').value.trim(),
      nodes: parsed?.nodes ?? [{ id: 'p0', kind: '概念', label: name, weight: 3, state: 'active', desc: '世界的中心概念' }],
      edges: parsed?.edges ?? [],
    });
    window.__parsedWorld = null;
    $('#world-modal').classList.remove('open');
    $('#w-name').value = ''; $('#w-emoji').value = '🌐'; $('#w-intro').value = ''; $('#w-lore').value = '';
    renderKB(); toast('世界已铸造');
    state.worldView = w.id; renderKB();
  });
  $('#w-parse-btn').addEventListener('click', async () => {          /* 从已有设定: 文件 + 粘贴 */
    const files = await collectFiles('w').catch(e => { toast('读取文件失败：' + e.message, true); return ''; });
    const material = [files, $('#w-lore').value.trim()].filter(Boolean).join('\n\n');
    if (!material) { toast('先选择文件或粘贴素材', true); return; }
    await worldForgeFrom(material, $('#w-parse-btn'));
  });
  $('#w-seed-btn').addEventListener('click', async () => {           /* 从零开始: 一句话种子 */
    const seed = $('#w-seed').value.trim();
    if (!seed) { toast('先用一句话描述世界', true); return; }
    await worldForgeFrom(seed, $('#w-seed-btn'), true);
  });
  $('#c-seed-btn').addEventListener('click', async () => {           /* 角色从零开始 */
    const seed = $('#c-seed').value.trim();
    if (!seed) { toast('先用一句话描述 TA', true); return; }
    await forgeCharFrom('根据这句话创作角色：' + seed, $('#c-seed-btn'));
  });

  /* 铸造双模式: 模式切换 + 文件选择(可移除 chip) */
  const wireForgeModal = (k, modesSel, rootSel) => {
    $(modesSel).addEventListener('click', (e) => {
      const m = e.target.closest('.forge-mode'); if (!m) return;
      $(modesSel).querySelectorAll('.forge-mode').forEach(b => b.classList.toggle('active', b === m));
      $(rootSel).querySelectorAll('.forge-panel').forEach(p => p.hidden = p.dataset.panel !== m.dataset.mode);
    });
    const input = $('#' + k + '-files'), chips = $('#' + k + '-file-chips');
    const renderChips = () => {
      chips.innerHTML = pickedFiles[k].map((f, i) =>
        `<span class="chip">${esc(f.name)} <b data-x="${i}" title="移除">×</b></span>`).join('');
    };
    $('#' + k + '-files-btn').addEventListener('click', () => input.click());
    input.addEventListener('change', () => { pickedFiles[k].push(...input.files); input.value = ''; renderChips(); });
    chips.addEventListener('click', (e) => {
      const x = e.target.closest('[data-x]'); if (!x) return;
      pickedFiles[k].splice(+x.dataset.x, 1); renderChips();
    });
  };
  wireForgeModal('c', '#c-modes', '#char-modal');
  wireForgeModal('w', '#w-modes', '#world-modal');

  /* ── 星图编辑器 ── */
  wireWorldEditor();

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
    + '.btn, .iconbtn, .bubble, .node-detail, .banner, .api-chip, .side-foot .nav-item, .nav, '
    + '.world-card, .world-detail, .world-intro, .we-md, .we-preview, .we-panel, .seg-btn, .we-rtbar button';
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

/* ── 角色卡网格: 布局 1:1 跟手 + 位置偏移普通缓动 ──
   布局永远即时跟手(无动画/无内联宽); 列数变化瞬间只记
   "视觉连续性偏移"(旧视觉 - 新布局, 含在飞偏移当前值), 之后
   320ms 固定时长 easeOutCubic 缓到位 —— 只平移不缩放
   (尺寸跟布局即时变, 拉伸形变=弹跳感来源, 已去除) */
(() => {
  const grid = $('#char-grid');
  if (!grid) return;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const cards = () => [...grid.querySelectorAll('.char-card')];
  const cols = () => {
    try { return getComputedStyle(grid).gridTemplateColumns.trim().split(/\s+/).map(parseFloat).filter(n => n > 0).length; }
    catch { return 0; }
  };
  let nCols = 0, layout = [], raf = 0, t0 = 0;
  const off = new Map();     /* el → 起始偏移 {dx,dy} 只平移 */
  const DUR = 320;           /* 固定时长 · 普通缓出(非弹簧) */
  const easeOut = p => 1 - Math.pow(1 - p, 3);
  const curOff = o => {      /* 在飞偏移的当前值(改道连续性用) */
    if (!o) return { dx: 0, dy: 0 };
    const r = 1 - easeOut(Math.min(1, (performance.now() - t0) / DUR));
    return { dx: o.dx * r, dy: o.dy * r };
  };
  function frame(t) {
    raf = 0;
    const p = Math.min(1, (t - t0) / DUR), r = 1 - easeOut(p);
    for (const [el, o] of off) {
      if (!el.isConnected) { off.delete(el); continue; }
      if (p >= 1) { el.style.transform = ''; off.delete(el); continue; }
      el.style.transform = `translate(${(o.dx * r).toFixed(2)}px, ${(o.dy * r).toFixed(2)}px)`;
    }
    if (off.size) raf = requestAnimationFrame(frame);
  }
  const stop = () => { if (raf) cancelAnimationFrame(raf); raf = 0; for (const [el] of off) el.style.transform = ''; off.clear(); };
  function onResize() {
    const els = cards();
    if (!els.length || !grid.offsetWidth) { nCols = 0; layout = []; stop(); return; }
    const n = cols();
    if (!n) return;
    if (nCols && n !== nCols && layout.length === els.length && !reduced.matches) {
      els.forEach((el, i) => {
        const old = layout[i];
        if (!old || el.getAnimations().length) return;
        const c = curOff(off.get(el));            /* 在飞偏移当前值: 改道零跳变 */
        const dx = (old.l + c.dx) - el.offsetLeft, dy = (old.t + c.dy) - el.offsetTop;
        if (Math.abs(dx) < .5 && Math.abs(dy) < .5) { off.delete(el); el.style.transform = ''; return; }
        off.set(el, { dx, dy });
      });
      if (off.size) { t0 = performance.now(); if (!raf) raf = requestAnimationFrame(frame); }
    }
    nCols = n;
    layout = els.map(el => ({ l: el.offsetLeft, t: el.offsetTop, w: el.offsetWidth, h: el.offsetHeight }));
  }
  new ResizeObserver(() => setTimeout(onResize, 0)).observe(grid);   /* 勿用 rAF: 面板隐藏时 rAF 挂起 */
})();

/* ── 卡片按下 3D 倾斜: 中心锚点跷跷板 —— 被按处下沉, 对角上浮 ──
   事件委托(重渲染安全); transform-origin 保持默认卡片正中心;
   按下捕获当前内联 transform 为基线(与网格列变偏移动画正交, 平移不受 origin 影响);
   reduced-motion 直接跳过 */
(() => {
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;
  const MAX = 7;                     /* 最大倾角 deg */
  let cur = null;
  const reset = (a) => { a.el.style.transform = a.base; };
  const loop = () => {
    if (!cur) return;
    const { el, base } = cur;
    cur.rx += (cur.trx - cur.rx) * cur.k;
    cur.ry += (cur.try_ - cur.ry) * cur.k;
    if (!cur.hold && Math.abs(cur.rx) < 0.05 && Math.abs(cur.ry) < 0.05) {
      reset(cur);                    /* 完全回正: 还原基线 */
      cur = null;
      return;
    }
    el.style.transform = `${base ? base + ' ' : ''}perspective(760px) rotateX(${cur.rx.toFixed(2)}deg) rotateY(${cur.ry.toFixed(2)}deg)`;
    requestAnimationFrame(loop);
  };
  document.addEventListener('pointerdown', (e) => {
    const el = e.target.closest?.('.char-card, .world-card, .node-detail, .world-detail');
    if (!el) return;
    if (cur && cur.el !== el) reset(cur);                             /* 换卡: 上一张立即复位 */
    const r = el.getBoundingClientRect();                             /* 倾斜前的未形变矩形(滑动映射的稳定基准) */
    cur = { el, base: el.style.transform || '', r,
      rx: 0, ry: 0, k: 0.25, hold: true,
      trx: (0.5 - (e.clientY - r.top) / r.height) * 2 * MAX,
      try_: ((e.clientX - r.left) / r.width - 0.5) * 2 * MAX,
    };
    requestAnimationFrame(loop);
  });
  /* 按住滑动: 目标角随指针实时更新(出界钳制到边缘), lerp 平滑追逐 */
  document.addEventListener('pointermove', (e) => {
    if (!cur || !cur.hold) return;
    const { r } = cur;
    const nx = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    const ny = Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
    cur.trx = (0.5 - ny) * 2 * MAX;
    cur.try_ = (nx - 0.5) * 2 * MAX;
  }, { passive: true });
  const release = () => {
    if (!cur) return;
    cur.hold = false; cur.trx = 0; cur.try_ = 0; cur.k = 0.16;       /* 松手: 缓速回正 */
  };
  document.addEventListener('pointerup', release);
  document.addEventListener('pointercancel', release);
})();

bind();
bindDrawer();
route();
