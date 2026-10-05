/* ling · 音效（uisfx minimal 包, CC0）· 克制原则：低音量、仅关键事件、可静音 */
const Sfx = (() => {
  const cache = {};
  const VOL = { default: 0.3, error: 0.42 };

  function play(name) {
    if (Store.settings.sound === false) return;
    try {
      let a = cache[name];
      if (!a) {
        a = new Audio(`assets/sfx/${name}.ogg`);
        a.volume = VOL[name] ?? VOL.default;
        a.preload = 'auto';
        cache[name] = a;
      }
      a.currentTime = 0;
      a.play().catch(() => { /* 自动播放策略拒绝时静默 */ });
    } catch { /* 音效失败不影响功能 */ }
  }

  function muted() { return Store.settings.sound === false; }
  function setMuted(m) { Store.saveSettings({ sound: !m }); }

  return { play, muted, setMuted };
})();
