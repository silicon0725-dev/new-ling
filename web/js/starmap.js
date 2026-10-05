/* ling · 成长星图（Canvas 力导向）
   与 ling-render 同源视觉：近黑底、白/银灰节点发光、流光连线、背景星尘；
   缩放/平移在显示层完成（同 Slint 版的显示层契约思路） */
class Starmap {
  constructor(canvas, onPick) {
    this.cv = canvas; this.ctx = canvas.getContext('2d');
    this.onPick = onPick;
    this.editMode = false;          /* 编辑模式: 拖节点/右键菜单/长按菜单 */
    this.onEditAdd = null;
    this.onEditMenu = null;         /* ({x,y(client), node, world}) 右键/长按菜单 */
    this.nodes = []; this.edges = [];
    this.view = { x: 0, y: 0, k: 1 };
    this.particles = [];
    this.running = false; this.t = 0;
    this.selected = null;
    /* 底色与页面 --bg 同源：星图视口不再是纯黑, 与顶栏背景物理同色(消除交界灰条) */
    this.bg = getComputedStyle(document.body).backgroundColor || '#060708';
    this._bind();
  }

  /* nodes: [{id, kind:'trait'|'memory', label, weight, state}], edges: [[idA,idB]] */
  setData(nodes, edges) {
    const W = this.cv.clientWidth || 800, H = this.cv.clientHeight || 600;
    this.nodes = nodes.map((n, i) => {
      const old = this.nodes.find(o => o.id === n.id);
      const ang = (i / nodes.length) * Math.PI * 2;
      return { ...n, ...old, x: old?.x ?? W / 2 + Math.cos(ang) * 140, y: old?.y ?? H / 2 + Math.sin(ang) * 140, vx: 0, vy: 0 };
    });
    this.edges = edges
      .map(([a, b]) => [this.nodes.find(n => n.id === a), this.nodes.find(n => n.id === b)])
      .filter(([a, b]) => a && b);
    if (!this.running) { this.running = true; requestAnimationFrame(() => this._tick()); }
  }

  _bind() {
    const cv = this.cv;
    new ResizeObserver(() => this._resize()).observe(cv.parentElement);
    this._resize();
    cv.addEventListener('wheel', (e) => {
      e.preventDefault();
      const k = Math.min(3, Math.max(0.4, this.view.k * (e.deltaY < 0 ? 1.12 : 0.89)));
      this.view.k = k;                       /* 以指针为锚的缩放（简化：中心锚） */
    }, { passive: false });
    let drag = null, nodeDrag = null, lp = null, lpStart = null;
    const ptrs = new Map();                                 /* 多指追踪: 双指捏合缩放/平移 */
    let pinch = null;
    cv.addEventListener('pointerdown', (e) => {
      if (e.button === 2) return;                                /* 右键交 contextmenu 事件 */
      ptrs.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (ptrs.size === 2) {                                     /* 第二指落下: 进入捏合, 清除单指状态 */
        if (lp) { clearTimeout(lp); lp = null; lpStart = null; }
        drag = null; nodeDrag = null;
        pinch = null;                                            /* 首次 move 建立基线 */
        return;
      }
      if (ptrs.size > 2) return;
      const p = this._toWorld(e);
      const hit = this._pick(p);
      if (hit) {
        this.selected = hit.id;
        if (!this.editMode) this.onPick?.(hit);                  /* 编辑模式: 左键节点=拖动, 编辑走右键菜单 */
        if (this.editMode) nodeDrag = { n: hit, ox: hit.x - p.x, oy: hit.y - p.y, moved: false };
      } else {
        drag = { x: e.clientX, y: e.clientY, vx: this.view.x, vy: this.view.y };   /* 左键/单指空白=平移(两模式同) */
        if (!this.editMode) { this.selected = null; this.onPick?.(null); }
      }
      /* 触屏/触控笔长按 = 右键菜单(移动 <6px 且未抬起) */
      if (this.editMode && (e.pointerType === 'touch' || e.pointerType === 'pen')) {
        const sx = e.clientX, sy = e.clientY;
        lpStart = { sx, sy };
        lp = setTimeout(() => {
          lp = null; lpStart = null; drag = null; nodeDrag = null;
          const wp = this._toWorld({ clientX: sx, clientY: sy });
          this.onEditMenu?.({ x: sx, y: sy, node: this._pick(wp), world: wp });
        }, 480);
      }
      cv.setPointerCapture(e.pointerId);
    });
    cv.addEventListener('pointermove', (e) => {
      if (ptrs.has(e.pointerId)) ptrs.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (ptrs.size >= 2) {                                      /* 双指: 捏合缩放 + 中点平移 */
        const [a, b] = [...ptrs.values()];
        const dist = Math.hypot(a.x - b.x, a.y - b.y);
        const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
        if (!pinch) { pinch = { dist, mid, k: this.view.k }; return; }
        this.view.k = Math.min(3, Math.max(0.4, pinch.k * dist / Math.max(pinch.dist, 1)));
        this.view.x += mid.x - pinch.mid.x;
        this.view.y += mid.y - pinch.mid.y;
        pinch.mid = mid;
        return;
      }
      if (lp && lpStart && Math.hypot(e.clientX - lpStart.sx, e.clientY - lpStart.sy) > 6) {
        clearTimeout(lp); lp = null; lpStart = null;              /* 移动即取消长按 */
      }
      if (nodeDrag) {
        const p = this._toWorld(e);
        nodeDrag.n.x = p.x + nodeDrag.ox; nodeDrag.n.y = p.y + nodeDrag.oy;
        nodeDrag.n.pinned = true; nodeDrag.moved = true;
        return;
      }
      if (!drag) return;
      this.view.x = drag.vx + (e.clientX - drag.x);
      this.view.y = drag.vy + (e.clientY - drag.y);
    });
    const endPtr = (e) => {
      ptrs.delete(e.pointerId);
      if (ptrs.size < 2) pinch = null;
      if (lp) { clearTimeout(lp); lp = null; }
      lpStart = null; drag = null; nodeDrag = null;
    };
    cv.addEventListener('pointerup', endPtr);
    cv.addEventListener('pointercancel', endPtr);
    /* 右键菜单(编辑模式) */
    cv.addEventListener('contextmenu', (e) => {
      if (!this.editMode) return;
      e.preventDefault();
      const p = this._toWorld(e);
      this.onEditMenu?.({ x: e.clientX, y: e.clientY, node: this._pick(p), world: p });
    });
  }

  setEditMode(m) { this.editMode = !!m; }
  relax() {
    const rnd = (n) => crypto.getRandomValues(new Int8Array(1))[0] / 128 * n;   /* 随机踢一脚重排 */
    for (const n of this.nodes) { n.pinned = false; n.vx = rnd(8); n.vy = rnd(8); }
  }

  _resize() {
    const r = this.cv.parentElement.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return;                          /* 隐藏/塌缩: 忽略, 勿清画布 */
    if (Math.abs(r.width - this.W) < 1 && Math.abs(r.height - this.H) < 1) return;   /* 亚像素抖动: 不重建背板 */
    const dpr = Math.min(2, devicePixelRatio || 1);
    if (this.particles.length) {                                      /* 星尘等比重铺: 覆盖新区无重置感 */
      const kx = r.width / (this.W || r.width), ky = r.height / (this.H || r.height);
      for (const p of this.particles) { p.x *= kx; p.y *= ky; }
    }
    this.cv.width = r.width * dpr; this.cv.height = r.height * dpr;
    this.W = r.width; this.H = r.height;
    if (!this.particles.length) {
      const rnd = (n) => crypto.getRandomValues(new Uint32Array(1))[0] / 4294967296 * n;
      this.particles = Array.from({ length: 90 }, () => ({
        x: rnd(r.width), y: rnd(r.height), r: rnd(1.4) + 0.5, a: rnd(0.5) + 0.2, ph: rnd(6.28),
      }));
    }
  }

  _toWorld(e) {
    const r = this.cv.getBoundingClientRect();
    return { x: (e.clientX - r.left - this.W / 2 - this.view.x) / this.view.k + this.W / 2,
             y: (e.clientY - r.top - this.H / 2 - this.view.y) / this.view.k + this.H / 2 };
  }
  _pick(p) {
    let best = null, bd = 26 / this.view.k;
    for (const n of this.nodes) {
      const d = Math.hypot(n.x - p.x, n.y - p.y);
      if (d < bd + n.weight * 3) { bd = d; best = n; }
    }
    return best;
  }

  _tick() {
    if (!this.running) return;
    this.t += 0.016;
    this._physics();
    this._draw();
    requestAnimationFrame(() => this._tick());
  }

  _physics() {
    const N = this.nodes, W = this.W, H = this.H;
    for (let i = 0; i < N.length; i++) {
      for (let j = i + 1; j < N.length; j++) {
        const a = N[i], b = N[j];
        let dx = b.x - a.x, dy = b.y - a.y;
        let d2 = Math.max(dx * dx + dy * dy, 400);
        const f = 2400 / d2;
        const d = Math.sqrt(d2);
        a.vx -= (dx / d) * f; a.vy -= (dy / d) * f;
        b.vx += (dx / d) * f; b.vy += (dy / d) * f;
      }
    }
    for (const [a, b] of this.edges) {
      const dx = b.x - a.x, dy = b.y - a.y, d = Math.max(Math.hypot(dx, dy), 1);
      const f = (d - 150) * 0.008;
      a.vx += (dx / d) * f; a.vy += (dy / d) * f;
      b.vx -= (dx / d) * f; b.vy -= (dy / d) * f;
    }
    for (const n of N) {
      if (n.pinned) continue;                                        /* 编辑模式钉住的节点不参与物理 */
      n.vx += (W / 2 - n.x) * 0.0015; n.vy += (H / 2 - n.y) * 0.0015;   // 向心
      n.vx *= 0.85; n.vy *= 0.85;
      n.x += n.vx; n.y += n.vy;
    }
  }

  _draw() {
    const x = this.ctx, dpr = Math.min(2, devicePixelRatio || 1);
    x.setTransform(dpr, 0, 0, dpr, 0, 0);
    x.clearRect(0, 0, this.W, this.H);
    x.fillStyle = this.bg; x.fillRect(0, 0, this.W, this.H);

    /* 背景星尘 —— 屏幕空间绘制(缩放/平移变换之外): 视差背景层,
       不然 zoom-in 时一窄条源粒子被拉伸铺满边缘, 密集重叠成灰带 */
    for (const p of this.particles) {
      const tw = 0.55 + 0.45 * Math.sin(this.t * 1.2 + p.ph);
      x.fillStyle = `rgba(255,255,255,${(p.a * tw * 0.5).toFixed(3)})`;
      x.beginPath(); x.arc(p.x, p.y, p.r, 0, 7); x.fill();
    }

    x.save();
    x.translate(this.W / 2 + this.view.x, this.H / 2 + this.view.y);
    x.scale(this.view.k, this.view.k);
    x.translate(-this.W / 2, -this.H / 2);

    /* 连线: 与选中节点相连的边随其亮度渐亮 */
    for (const [a, b] of this.edges) {
      const glow = 1 + Math.max(a.sel ?? 0, b.sel ?? 0) * 1.4;
      const g = x.createLinearGradient(a.x, a.y, b.x, b.y);
      g.addColorStop(0, `rgba(255,255,255,${Math.min(0.85, 0.30 * glow).toFixed(3)})`);
      g.addColorStop(1, `rgba(230,230,234,${Math.min(0.60, 0.14 * glow).toFixed(3)})`);
      x.strokeStyle = g; x.lineWidth = 0.8;
      x.beginPath(); x.moveTo(a.x, a.y); x.lineTo(b.x, b.y); x.stroke();
    }

    /* 节点: 选中不画框 —— 亮度 sel 每帧向目标缓动, 星体缓慢亮起/熄灭 */
    for (const n of this.nodes) {
      const sel = n.sel = (n.sel ?? 0) + ((this.selected === n.id ? 1 : 0) - (n.sel ?? 0)) * 0.055;
      const r = (3 + n.weight * 1.4) * (1 + 0.35 * sel);
      const dormant = n.state === 'dormant';
      const col = n.kind === 'trait' ? '255,255,255' : '214,214,218';
      x.shadowColor = `rgba(${col},${Math.min(1, (dormant ? 0.25 : 0.85) * (1 + sel)).toFixed(3)})`;
      x.shadowBlur = (12 + 26 * sel) * (this.view.k > 0.7 ? 1 : 0.7);
      x.fillStyle = `rgba(${col},${Math.min(1, (dormant ? 0.32 : 1) + 0.2 * sel).toFixed(3)})`;
      x.beginPath(); x.arc(n.x, n.y, r, 0, 7); x.fill();
      x.shadowBlur = 0;
      x.fillStyle = `rgba(244,244,245,${Math.min(1, (dormant ? 0.4 : 0.75) + 0.25 * sel).toFixed(3)})`;
      x.font = '500 10.5px Inter, "PingFang SC", sans-serif';
      x.textAlign = 'center';
      x.fillText(n.label, n.x, n.y - r - 6);
    }
    x.restore();
  }
}
