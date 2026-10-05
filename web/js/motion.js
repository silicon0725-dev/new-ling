/* ling · 统一组件变换引擎 Motion
   单一函数驱动全部组件动效，替代散落各处的 CSS transition/animation：
     Motion.motion(el, { x, y, angle, size, width, height, radius, opacity, speed, ...材质 })
       几何    x/y 平移 px · angle 旋转 deg · size 缩放倍数 · width/height/radius px
       材质    opacity 透明度 0-1 · blur 自模糊 px · frost 毛玻璃度(背景模糊) px
               grain 颗粒度 0-1(feTurbulence 噪点叠层) · brightness/saturate 滤镜倍数
       速度    speed press|fast|med|slow|毫秒数
   · 起始值一律自动检测（transform 矩阵分解 + 计算样式 + filter/backdrop 解析）
   · 目标值传 'auto'：宽高自动测自然值（自适应场景），其余属性保持现值
   · 缓动统一贝塞尔曲线：预设与 tokens.css --ease 同源；可传 [x1,y1,x2,y2] 自定义
   · 同元素再次调用 = 平滑接管（先读现值再取消旧动画〔含噪点层动画〕，同帧无跳变）
   · 颗粒度: 首次使用时注入 .motion-grain 噪点覆盖层(跟随圆角/指针穿透),
     其动画注册进同元素接管注册表, 与元素本体同曲线同速
   · 结束后 commitStyles 固化为内联样式，不与后续 CSS 过渡打架
   · prefers-reduced-motion 时瞬时到位（令牌动效纪律）
   canvas 侧可用 Motion.bezier(x1,y1,x2,y2)(进度) 取曲线值。 */
const Motion = (() => {
  /* 三次贝塞尔 y(x) 求解器：牛顿迭代 + 二分兜底，精度 1e-6（标准 CSS timing 函数算法） */
  function bezier(x1, y1, x2, y2) {
    const cx = 3 * x1, bx = 3 * (x2 - x1) - cx, ax = 1 - cx - bx;
    const cy = 3 * y1, by = 3 * (y2 - y1) - cy, ay = 1 - cy - by;
    const fx = t => ((ax * t + bx) * t + cx) * t;
    const fy = t => ((ay * t + by) * t + cy) * t;
    const dx = t => (3 * ax * t + 2 * bx) * t + cx;
    return x => {
      if (x <= 0) return 0;
      if (x >= 1) return 1;
      let t = x;
      for (let i = 0; i < 8; i++) {                    /* 牛顿 */
        const e = fx(t) - x;
        if (Math.abs(e) < 1e-6) return fy(t);
        const d = dx(t);
        if (Math.abs(d) < 1e-6) break;
        t -= e / d;
      }
      let lo = 0, hi = 1; t = x;                       /* 二分兜底 */
      while (hi - lo > 1e-6) { fx(t) < x ? (lo = t) : (hi = t); t = (lo + hi) / 2; }
      return fy(t);
    };
  }

  /* 曲线预设（与 tokens.css 令牌同源）· 速度预设（--dur-* 同源） */
  const EASE = {
    ease: [0.2, 0.7, 0.2, 1],       /* = var(--ease) · 前冲快收, 微交互用 */
    out:  [0.16, 1, 0.3, 1],        /* 急出缓停 · 入场/展开 */
    in:   [0.7, 0, 0.84, 0],        /* 缓起急收 · 退场/收起 */
    glide: [0.42, 0, 0.25, 1],      /* 缓起-长滑-稳收 · 大位移飞行(FLIP), 低Q弹感 */
  };
  const SPEED = { press: 120, fast: 160, med: 220, slow: 280 };

  /* filter/backdrop-filter 字符串 → 数值表（未出现的函数取默认） */
  function parseFilter(s) {
    const out = { blur: 0, brightness: 1, saturate: 1 };
    if (!s || s === 'none') return out;
    for (const m of s.matchAll(/([a-z-]+)\(([^)]+)\)/g)) {
      const v = parseFloat(m[2]);
      if (Number.isNaN(v)) continue;
      if (m[1] === 'blur') out.blur = v;
      else if (m[1] === 'brightness') out.brightness = v;
      else if (m[1] === 'saturate') out.saturate = v;
    }
    return out;
  }
  const filtStr = o => {
    const p = [];
    if (o.blur) p.push(`blur(${o.blur}px)`);
    if (o.brightness !== 1) p.push(`brightness(${o.brightness})`);
    if (o.saturate !== 1) p.push(`saturate(${o.saturate})`);
    return p.length ? p.join(' ') : 'none';
  };

  /* 颗粒度: 噪点覆盖层(feTurbulence 灰度噪声 × overlay 混合), opacity 即颗粒量。
     首次注入后常驻(不可见时零成本); 覆盖层动画与元素本体共用接管注册表。 */
  const GRAIN_SVG = "data:image/svg+xml," + encodeURIComponent(
    `<svg xmlns='http://www.w3.org/2000/svg' width='128' height='128'>` +
    `<filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2' stitchTiles='stitch'/>` +
    `<feColorMatrix type='saturate' values='0'/></filter>` +
    `<rect width='128' height='128' filter='url(#n)'/></svg>`);
  /* void/表单元素无法容纳子节点, 跳过颗粒度 */
  const NO_CHILD = new Set(['INPUT', 'TEXTAREA', 'SELECT', 'IMG', 'HR', 'BR', 'CANVAS', 'IFRAME']);
  function ensureGrain(el) {
    if (NO_CHILD.has(el.tagName)) return null;
    let g = el.querySelector(':scope > .motion-grain');
    if (g) return g;
    const pos = getComputedStyle(el).position;
    if (pos === 'static') el.style.position = 'relative';   /* 覆盖层定位基准 */
    g = document.createElement('div');
    g.className = 'motion-grain';
    g.style.cssText =
      `position:absolute;inset:0;pointer-events:none;z-index:3;border-radius:inherit;` +
      `opacity:0;mix-blend-mode:overlay;background-image:url("${GRAIN_SVG}");background-size:128px 128px;`;
    el.appendChild(g);
    return g;
  }

  /* 起始状态自动检测：transform 分解 + 几何 + 材质(filter/backdrop/噪点/透明度) */
  function detect(el) {
    const cs = getComputedStyle(el);
    const m = cs.transform === 'none' ? new DOMMatrix() : new DOMMatrix(cs.transform);
    const g = el.querySelector(':scope > .motion-grain');
    return {
      x: +m.e.toFixed(3), y: +m.f.toFixed(3),
      angle: +(Math.atan2(m.b, m.a) * 180 / Math.PI).toFixed(3),
      size: +Math.hypot(m.a, m.b).toFixed(4),
      sx: +Math.hypot(m.a, m.b).toFixed(4),          /* 非均匀缩放(FLIP 用); 旋转≠0 时为近似 */
      sy: +Math.hypot(m.d, m.c).toFixed(4),
      width: parseFloat(cs.width), height: parseFloat(cs.height),
      radius: parseFloat(cs.borderTopLeftRadius) || 0,
      opacity: +cs.opacity,
      blur: parseFilter(cs.filter).blur,
      brightness: parseFilter(cs.filter).brightness,
      saturate: parseFilter(cs.filter).saturate,
      frost: parseFilter(cs.backdropFilter || cs.webkitBackdropFilter).blur,
      grain: g ? +getComputedStyle(g).opacity : 0,
    };
  }

  /* 'auto' 宽高：同步测自然值（置 auto → 强制回流读 offset → 还原，同帧无闪烁） */
  function naturalPx(el, prop) {
    const prev = el.style[prop];
    el.style[prop] = 'auto';
    const v = prop === 'width' ? el.offsetWidth : el.offsetHeight;
    el.style[prop] = prev;
    return v;
  }

  const KEYS = ['x', 'y', 'angle', 'size', 'sx', 'sy', 'width', 'height', 'radius', 'opacity',
                'blur', 'brightness', 'saturate', 'frost', 'grain'];
  const active = new WeakMap();     /* el → Set<Animation> 接管注册表(含噪点层动画) */

  function motion(el, spec = {}, opts = {}) {
    const speed = spec.speed ?? opts.speed ?? 'med';
    const dur = typeof speed === 'number' ? speed : (SPEED[speed] ?? SPEED.med);
    const easeDef = opts.ease ?? 'ease';
    const curve = Array.isArray(easeDef) ? easeDef : (EASE[easeDef] ?? EASE.ease);
    const D = matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : Math.max(0, dur);

    /* 1) 现值检测（含旧动画 fill 中的值 → 接管无跳变）；from 覆盖用于入场等场景。
       from 显式给 size 时映射到 sx/sy(均匀缩放语义), 免被检测出的非均匀值顶掉 */
    const ov = { ...(opts.from ?? {}) };
    if (ov.size !== undefined && ov.sx === undefined) { ov.sx = ov.size; if (ov.sy === undefined) ov.sy = ov.size; }
    const from = { ...detect(el), ...ov };

    /* 2) 取消旧同源动画（含上一次的噪点层动画；现值已捕获，取消到新动画同帧完成） */
    const old = active.get(el);
    if (old) { for (const a of old) a.cancel(); }
    const mine = new Set();
    active.set(el, mine);

    /* 3) 目标解析：'auto' → 宽高测自然值 / 其余保持现值 */
    const to = {};
    for (const k of KEYS) {
      const v = spec[k];
      if (v === undefined || v === null) continue;
      to[k] = v === 'auto' ? (k === 'width' || k === 'height' ? naturalPx(el, k) : from[k]) : +v;
    }

    const tf = o => `translate(${o.x}px, ${o.y}px) rotate(${o.angle}deg) scale(${o.sx ?? o.size}, ${o.sy ?? o.sx ?? o.size})`;
    const kf0 = { transform: tf(from) };
    const T0 = { x: to.x ?? from.x, y: to.y ?? from.y, angle: to.angle ?? from.angle, size: to.size ?? from.size, sx: to.sx ?? to.size ?? from.sx, sy: to.sy ?? to.sx ?? to.size ?? from.sy };
    const kf1 = { transform: tf(T0) };
    for (const k of ['width', 'height', 'radius', 'opacity']) {
      if (k in to) { kf0[k] = k === 'opacity' ? from[k] : from[k] + 'px'; kf1[k] = k === 'opacity' ? to[k] : to[k] + 'px'; }
    }
    /* 材质: filter 族(未指定的分量沿用现值)与 backdrop 毛玻璃 */
    const useFilter = 'blur' in to || 'brightness' in to || 'saturate' in to;
    if (useFilter) {
      const f0 = { blur: from.blur, brightness: from.brightness, saturate: from.saturate };
      const f1 = { blur: to.blur ?? from.blur, brightness: to.brightness ?? from.brightness, saturate: to.saturate ?? from.saturate };
      kf0.filter = filtStr(f0); kf1.filter = filtStr(f1);
    }
    if ('frost' in to) {
      kf0.backdropFilter = `blur(${from.frost}px)`;
      kf1.backdropFilter = `blur(${to.frost ?? from.frost}px)`;
    }

    /* 4a) 瞬时路径（reduced-motion / speed 0）：直接内联终态（含材质与噪点层） */
    if (D === 0) {
      el.style.transform = kf1.transform;
      for (const k of ['width', 'height', 'radius', 'opacity']) if (k in to) el.style[k] = String(kf1[k]);
      if (useFilter) el.style.filter = kf1.filter;
      if ('frost' in to) el.style.backdropFilter = kf1.backdropFilter;
      if ('grain' in to) { const g = ensureGrain(el); if (g) g.style.opacity = String(to.grain); }
      opts.onDone?.(true);
      return { finished: Promise.resolve(true), cancel() {} };
    }

    /* 4b) WAAPI：元素本体 + (如需)噪点层, 同曲线同时长, 注册进同一接管集合 */
    const A = { duration: D, delay: opts.delay ?? 0, easing: `cubic-bezier(${curve.join(', ')})`, fill: 'forwards' };
    const anims = [el.animate([kf0, kf1], A)];
    if ('grain' in to) {
      const g = ensureGrain(el);
      if (g) anims.push(g.animate([{ opacity: String(from.grain) }, { opacity: String(to.grain) }], A));
    }
    for (const a of anims) mine.add(a);
    const finished = Promise.all(anims.map(a => a.finished.then(() => ({ a, ok: true }), () => ({ a, ok: false }))))
      .then(res => {
        for (const { a, ok } of res) {
          mine.delete(a);
          if (ok) { try { a.commitStyles(); } catch { /* 元素已脱离文档 */ } a.cancel(); }
        }
        const all = res.every(r => r.ok);
        opts.onDone?.(all);
        return all;
      });
    return { finished, cancel: () => { for (const a of anims) a.cancel(); } };
  }

  return { motion, bezier, detect, ensureGrain, EASE, SPEED };
})();
