/* ling · 本地存储层（localStorage，本地优先）
   数据形状与 ling-core 对齐（简化）：角色/特质(活跃-休眠)/会话/叙事弧/领地近况 */
const Store = (() => {
  const KEY = 'ling-web-v1';
  const uid = () =>
    (crypto.randomUUID ? crypto.randomUUID() : Array.from(crypto.getRandomValues(new Uint8Array(8)), b => b.toString(36)).join(''))
      .slice(0, 12);
  const DORMANT_DAYS = 7;

  /* 种子世界：辉纹纪元（整理自《界序·完整设定集 v2.6》正史·世界总览） */
  const SEED_WORLD = {
    id: 'seed-huiwen', emoji: '✦', name: '辉纹纪元',
    intro: '五块大陆环中央海而立，一切力量皆源于「辉能」。辉能被刻印为辉纹，成就了文明，也埋下了灾变。世界辉能协会研究它，星环商盟流通它，灰烬议会追问它的真相——而辉能究竟是什么，无人知晓。',
    nodes: [
      { id: 'd1', kind: 'trait', label: '东溟大陆', weight: 5, state: 'active', desc: '「辉能的摇篮」·辉纹技术起源地。东方文明，国家众多，传承深厚（苍玄联邦/天衡武院）' },
      { id: 'd2', kind: 'trait', label: '阿萨拉大陆', weight: 4, state: 'active', desc: '「辉能的熔炉」·沙漠荒野，辉能矿藏最丰富（阿赫兰帝国/赤砂部族）' },
      { id: 'd3', kind: 'trait', label: '澜海群陆', weight: 4, state: 'active', desc: '「辉能的潮汐」·群岛与海洋，辉能随潮汐星辰而变（潮汐圣庭/星辰观测所）' },
      { id: 'd4', kind: 'trait', label: '埃尔德拉大陆', weight: 5, state: 'active', desc: '「辉能的十字路口」·世界中心，国际组织总部所在地，商业与学术之心' },
      { id: 'd5', kind: 'trait', label: '诺德拉大陆', weight: 3, state: 'active', desc: '「辉能的极境」·极北冰原，保有最古老的辉能形式（极光圣域/冰冠王庭）' },
      { id: 'o1', kind: '组织', label: '世界辉能协会', weight: 4, state: 'active', desc: '研究辉能。掌握「初辉记录」，辉能研究最高机构' },
      { id: 'o2', kind: '组织', label: '星环商盟', weight: 3, state: 'active', desc: '流通辉能。世界经济命脉，总部位于埃尔德拉' },
      { id: 'o3', kind: '组织', label: '苍穹骑士团', weight: 3, state: 'active', desc: '保护世界。国际灾害应对体系' },
      { id: 'o4', kind: '组织', label: '万国仲裁庭', weight: 3, state: 'active', desc: '裁定规则。国际法体系的基石' },
      { id: 'o5', kind: '组织', label: '灰烬议会', weight: 4, state: 'active', desc: '追问真相。掌握古辉文明秘密的隐秘组织' },
      { id: 'o6', kind: '组织', label: '自由猎团', weight: 2, state: 'active', desc: '自由行走。冒险者体系的独立象征' },
      { id: 'o7', kind: '组织', label: '深渊遗民', weight: 4, state: 'active', desc: '守望过去。古辉文明幸存者，千年前遁入地下' },
      { id: 'c1', kind: '概念', label: '辉能', weight: 5, state: 'active', desc: '一切力量之源。可被使用、理解、限制、刻印、传承——但其本质无人知晓' },
      { id: 'c2', kind: '概念', label: '辉纹', weight: 4, state: 'active', desc: '辉能被刻印后的形态。技术与传承的载体' },
      { id: 'c3', kind: '概念', label: '天辉晶', weight: 3, state: 'active', desc: '核心资源，正在枯竭。资源危机是当代最大暗流' },
      { id: 'c4', kind: '概念', label: '第一辉源', weight: 3, state: 'active', desc: '辉能的终极谜题。一切答案加起来意味着什么？' },
      { id: 'e1', kind: '事件', label: '辉烬灾变', weight: 4, state: 'active', desc: '千年前毁灭古辉文明的大灾变。会再次发生吗？' },
      { id: 'e2', kind: '事件', label: '冷辉战', weight: 2, state: 'active', desc: '三千年前天辉/地辉/生辉三派内战，古辉文明由盛转衰' },
      { id: 'f1', kind: '人物', label: '岚珞', weight: 3, state: 'active', desc: '十九岁。故事开始的地方' },
      { id: 'f2', kind: '人物', label: '岚澄', weight: 2, state: 'active', desc: '成长与守护' },
      { id: 'f3', kind: '人物', label: '沈砚川', weight: 2, state: 'active', desc: '陪伴' },
    ],
    edges: [
      ['c1', 'c2'], ['c1', 'c3'], ['c1', 'c4'], ['c1', 'd3'], ['c1', 'd5'],
      ['c2', 'd1'], ['c3', 'd2'], ['c3', 'o2'],
      ['d4', 'd1'], ['d4', 'd2'], ['d4', 'd3'], ['d4', 'd5'],
      ['d4', 'o1'], ['d4', 'o2'], ['d4', 'o3'], ['d4', 'o4'],
      ['o1', 'c1'], ['o5', 'e1'], ['o5', 'c4'], ['e1', 'o7'], ['e1', 'c1'], ['e2', 'c1'],
      ['f1', 'f2'], ['f1', 'f3'], ['f2', 'f3'],
    ],
    createdAt: Date.now(),
  };

  function blank() {
    return { settings: { baseUrl: '', apiKey: '', model: '', autoDistill: true, sound: true }, characters: [], worlds: [] };
  }
  let data = load();

  function load() {
    try {
      const parsed = JSON.parse(localStorage.getItem(KEY) || '{}');
      const d = { ...blank(), ...parsed };
      if (!('worlds' in parsed)) { d.worlds = [structuredClone(SEED_WORLD)]; localStorage.setItem(KEY, JSON.stringify(d)); }
      return d;
    } catch { return blank(); }
  }
  function save() { localStorage.setItem(KEY, JSON.stringify(data)); }

  const api = {
    get settings() { return data.settings; },
    get characters() { return data.characters; },
    char(id) { return data.characters.find(c => c.id === id); },
    get worlds() { return data.worlds; },
    world(id) { return data.worlds.find(w => w.id === id); },
    addWorld(fields) {
      const w = { id: uid(), emoji: '🌐', intro: '', nodes: [], edges: [], createdAt: Date.now(), ...fields };
      data.worlds.push(w); save(); return w;
    },
    updateWorld(id, patch) { Object.assign(this.world(id), patch); save(); },
    removeWorld(id) { data.worlds = data.worlds.filter(w => w.id !== id); save(); },

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
