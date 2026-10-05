/* ling · 本地存储层（localStorage，本地优先）
   数据形状与 ling-core 对齐（简化）：角色/特质(活跃-休眠)/会话/叙事弧/领地近况 */
const Store = (() => {
  const KEY = 'ling-web-v1';
  const uid = () =>
    (crypto.randomUUID ? crypto.randomUUID() : Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(36)).join(''))
      .slice(0, 12);
  const DORMANT_DAYS = 7;

  function blank() {
    return { settings: { baseUrl: '', apiKey: '', model: '', autoDistill: true, sound: true }, characters: [] };
  }
  let data = load();

  function load() {
    try { return { ...blank(), ...JSON.parse(localStorage.getItem(KEY) || '{}') }; }
    catch { return blank(); }
  }
  function save() { localStorage.setItem(KEY, JSON.stringify(data)); }

  const api = {
    get settings() { return data.settings; },
    get characters() { return data.characters; },
    char(id) { return data.characters.find(c => c.id === id); },

    apiReady() { return !!(data.settings.baseUrl.trim() && data.settings.model.trim()); },

    saveSettings(patch) { Object.assign(data.settings, patch); save(); },

    addCharacter(fields) {
      const c = { id: uid(), emoji: '✦', greeting: '', style: '', traits: [], sessions: [], arcs: [], timeline: [], ...fields };
      c.sessions.push({ id: uid(), title: '初次对话', messages: c.greeting ? [{ role: 'assistant', content: c.greeting, ts: Date.now() }] : [] });
      data.characters.push(c); save(); return c;
    },
    updateCharacter(id, patch) { Object.assign(this.char(id), patch); save(); },
    removeCharacter(id) {
      data.characters = data.characters.filter(c => c.id !== id); save();
    },

    /* 会话 */
    newSession(charId, title = '新的对话') {
      const s = { id: uid(), title, messages: [] };
      this.char(charId).sessions.unshift(s); save(); return s;
    },
    session(charId, sessionId) { return this.char(charId).sessions.find(s => s.id === sessionId); },
    removeSession(charId, sessionId) {
      const c = this.char(charId);
      c.sessions = c.sessions.filter(s => s.id !== sessionId); save();
    },
    pushMessage(charId, sessionId, msg) {
      const s = this.session(charId, sessionId);
      const m = { ts: Date.now(), ...msg };
      s.messages.push(m); save(); return m;
    },

    /* 特质：活跃/休眠 */
    touchTrait(charId, name) {
      const c = this.char(charId);
      let t = c.traits.find(t => t.name === name);
      if (t) { t.lastActive = Date.now(); t.state = 'active'; t.weight = Math.min(5, t.weight + 1); }
      else c.traits.push({ name, weight: 2, state: 'active', lastActive: Date.now() });
      save();
    },
    applyTraits(charId, traits) {
      (traits || []).forEach(t => this.touchTrait(charId, t.name));
      this.refreshDormancy(charId);
    },
    refreshDormancy(charId) {
      const cut = Date.now() - DORMANT_DAYS * 86400e3;
      this.char(charId).traits.forEach(t => { t.state = t.lastActive < cut ? 'dormant' : 'active'; });
      save();
    },

    /* 叙事弧（记忆的最聚合层） */
    addArc(charId, arc) {
      const c = this.char(charId);
      c.arcs.unshift({ id: uid(), ts: Date.now(), traits: [], ...arc });
      if (c.arcs.length > 80) c.arcs.length = 80;
      save();
    },

    /* 领地近况 */
    addTimeline(charId, text) {
      this.char(charId).timeline.unshift({ id: uid(), ts: Date.now(), text }); save();
    },

    exportAll() { return JSON.stringify(data, null, 2); },
    importAll(json) {
      const parsed = JSON.parse(json);           // 交给调用方 catch
      if (!Array.isArray(parsed.characters)) throw new Error('格式不符：缺少 characters');
      data = { ...blank(), ...parsed }; save();
    },
    clearAll() { data = blank(); save(); },
  };
  return api;
})();
