// Floating, desktop-style windows: drag the title bar to move, drag any edge or
// corner to resize, and drop on a screen edge to tile (KDE-style quick tiling:
// top = maximize, left/right = half, corners = quarter, with a preview).
// Minimizing animates a window into its button in the shared dock at the bottom
// right ("magic lamp"); windows with `onClose` also get a ✕. Geometry, tiling and
// minimized state are remembered per browser.

type Snap = null | 'max' | 'left' | 'right' | 'tl' | 'tr' | 'bl' | 'br';
interface Geo { x: number; y: number; w: number; h: number }
interface Saved { geo: Geo; snap: Snap; min: boolean }

export interface FloatWin {
  root: HTMLElement;
  /** Bar under the title for chips / summary. */
  tools: HTMLElement;
  body: HTMLElement;
  dockBtn: HTMLButtonElement;
  setTitle(sub: string): void;
  setBadge(text: string): void;
  open(): void;
  minimize(): void;
  isMinimized(): boolean;
  /** Continue a pointer drag that started elsewhere (tear-off) as a window move. */
  startMove(ev: PointerEvent, offX: number, offY: number): void;
  /** Remove the window and its dock button (contents are the caller's). */
  destroy(): void;
}

export interface FloatWinOpts {
  id: string;
  title: string;
  dockLabel: string;
  storageKey: string;
  host?: HTMLElement;
  /** Initial geometry when nothing is saved. */
  geo?: Geo;
  /** Initial state when nothing is saved (default: minimized). */
  startOpen?: boolean;
  /** Adds a ✕ button; the caller decides what closing means. */
  onClose?: () => void;
}

const EDGE = 6;        // px from the screen edge that triggers tiling
const CORNER = 80;     // px along an edge that counts as a corner zone
const MAGNET = 14;     // edges stick to the screen border within this distance
const MIN_W = 340, MIN_H = 200;
const DOCK_GAP = 16;
let zTop = 60;         // raise-on-click stacking

const vw = () => document.documentElement.clientWidth;
const vh = () => window.innerHeight;
const reduceMotion = () => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;

function snapGeo(s: Exclude<Snap, null>): Geo {
  const W = vw(), H = vh(), hw = Math.round(W / 2), hh = Math.round(H / 2);
  switch (s) {
    case 'max': return { x: 0, y: 0, w: W, h: H };
    case 'left': return { x: 0, y: 0, w: hw, h: H };
    case 'right': return { x: hw, y: 0, w: W - hw, h: H };
    case 'tl': return { x: 0, y: 0, w: hw, h: hh };
    case 'tr': return { x: hw, y: 0, w: W - hw, h: hh };
    case 'bl': return { x: 0, y: hh, w: hw, h: H - hh };
    case 'br': return { x: hw, y: hh, w: W - hw, h: H - hh };
  }
}

/** Which tile a pointer at (x, y) asks for while a window is being dragged. */
function zoneAt(x: number, y: number): Snap {
  const W = vw(), H = vh();
  const L = x <= EDGE, R = x >= W - 1 - EDGE, T = y <= EDGE, B = y >= H - 1 - EDGE;
  if ((L && y < CORNER) || (T && x < CORNER)) return 'tl';
  if ((R && y < CORNER) || (T && x > W - CORNER)) return 'tr';
  if ((L && y > H - CORNER) || (B && x < CORNER)) return 'bl';
  if ((R && y > H - CORNER) || (B && x > W - CORNER)) return 'br';
  if (T) return 'max';
  if (L) return 'left';
  if (R) return 'right';
  return null;
}

function defaultGeo(): Geo {
  const w = Math.min(860, vw() - 32), h = Math.min(520, Math.round(vh() * 0.62));
  return { x: vw() - w - DOCK_GAP, y: vh() - h - 72, w, h };
}

/** Keep a window reachable: on screen, title bar visible, not below the minimum. */
function clampGeo(g: Geo): Geo {
  const W = vw(), H = vh();
  const w = Math.max(Math.min(MIN_W, W), Math.min(g.w, W));
  const h = Math.max(Math.min(MIN_H, H), Math.min(g.h, H));
  const x = Math.min(Math.max(g.x, 0), W - w);
  const y = Math.min(Math.max(g.y, 0), H - Math.min(h, 40));
  return { x, y, w, h };
}

/** The shared dock (bottom right); buttons are added right-to-left. */
function dockBar(host: HTMLElement): HTMLElement {
  let bar = document.getElementById('fw-dockbar');
  if (!bar) {
    bar = document.createElement('div');
    bar.id = 'fw-dockbar';
    bar.className = 'fw-dockbar';
    host.append(bar);
  }
  return bar;
}

export function createFloatWin(opts: FloatWinOpts): FloatWin {
  const host = opts.host || document.body;
  let saved: Saved = { geo: opts.geo ?? defaultGeo(), snap: null, min: !opts.startOpen };
  try {
    const v = JSON.parse(localStorage.getItem(opts.storageKey) || 'null');
    if (v && v.geo && ['x', 'y', 'w', 'h'].every((k) => Number.isFinite(v.geo[k]))) {
      saved = { geo: v.geo, snap: v.snap ?? null, min: v.min !== false };
    }
  } catch { /* storage unavailable: defaults */ }
  let geo: Geo = clampGeo(saved.geo); // the floating (un-tiled) geometry
  let snap: Snap = saved.snap;
  let minimized = saved.min;
  let alive = true;
  const persist = () => {
    if (!alive) return;
    try { localStorage.setItem(opts.storageKey, JSON.stringify({ geo, snap, min: minimized })); } catch { /* ignore */ }
  };

  const root = document.createElement('div');
  root.className = 'fw';
  root.id = opts.id;
  root.setAttribute('role', 'dialog');
  root.setAttribute('aria-label', opts.title);
  root.innerHTML = `
    <div class="fw-title" title="Drag to move · drop on a screen edge to tile · double-click to maximize">
      <span class="fw-name"></span><span class="fw-sub muted"></span>
      <span class="fw-ctrls">
        <button type="button" class="fw-btn fw-min" title="Minimize" aria-label="Minimize">–</button>
        <button type="button" class="fw-btn fw-max" title="Maximize / restore" aria-label="Maximize">□</button>
        ${opts.onClose ? '<button type="button" class="fw-btn fw-close" title="Put back on the page" aria-label="Close">✕</button>' : ''}
      </span>
    </div>
    <div class="fw-tools"></div>
    <div class="fw-body"></div>
    ${['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'].map((d) => `<div class="fw-h fw-h-${d}" data-dir="${d}"></div>`).join('')}`;
  (root.querySelector('.fw-name') as HTMLElement).textContent = opts.title;
  const title = root.querySelector('.fw-title') as HTMLElement;
  const sub = root.querySelector('.fw-sub') as HTMLElement;
  const tools = root.querySelector('.fw-tools') as HTMLElement;
  const body = root.querySelector('.fw-body') as HTMLElement;

  const dock = document.createElement('button');
  dock.type = 'button';
  dock.className = 'fw-dock';
  dock.id = `${opts.id}-dock`;
  dock.title = opts.title;
  dock.innerHTML = `<span class="fw-dock-ico">☰</span> <span class="fw-dock-lbl"></span> <span class="fw-badge"></span>`;
  (dock.querySelector('.fw-dock-lbl') as HTMLElement).textContent = opts.dockLabel;
  const badge = dock.querySelector('.fw-badge') as HTMLElement;

  const preview = document.createElement('div');
  preview.className = 'fw-snap';
  host.append(root, preview);
  dockBar(host).append(dock);

  const raise = () => { if (+root.style.zIndex !== zTop) root.style.zIndex = String(++zTop); };
  root.addEventListener('pointerdown', raise, true);
  raise();

  const current = (): Geo => (snap ? snapGeo(snap) : geo);
  function place(animate = false) {
    const g = current();
    root.classList.toggle('fw-anim', animate && !reduceMotion());
    root.classList.toggle('fw-tiled', !!snap);
    root.dataset.snap = snap || '';
    Object.assign(root.style, { left: `${g.x}px`, top: `${g.y}px`, width: `${g.w}px`, height: `${g.h}px` });
    if (animate) setTimeout(() => root.classList.remove('fw-anim'), 220);
  }
  function showPreview(z: Snap) {
    if (!z) { preview.classList.remove('on'); return; }
    const g = snapGeo(z);
    Object.assign(preview.style, { left: `${g.x}px`, top: `${g.y}px`, width: `${g.w}px`, height: `${g.h}px` });
    preview.classList.add('on');
  }

  // ---- move: title bar, or a drag handed over by a tear-off ----
  function beginMove(ev: PointerEvent, offX: number, offY: number, alreadyMoving: boolean) {
    let start = current();
    let moved = alreadyMoving, zone: Snap = null;
    if (alreadyMoving) root.classList.add('fw-moving');
    const onMove = (e: PointerEvent) => {
      if (!moved && Math.hypot(e.clientX - ev.clientX, e.clientY - ev.clientY) < 4) return;
      if (!moved && snap) {
        // Dragging a tiled window out restores its floating size under the pointer.
        const frac = offX / start.w;
        snap = null;
        start = { ...geo, x: 0, y: 0 };
        offX = Math.round(frac * geo.w);
      }
      moved = true;
      root.classList.add('fw-moving');
      let x = e.clientX - offX, y = Math.max(0, e.clientY - offY);
      // Magnetic screen borders.
      if (Math.abs(x) < MAGNET) x = 0;
      if (Math.abs(vw() - (x + geo.w)) < MAGNET) x = vw() - geo.w;
      if (Math.abs(y) < MAGNET) y = 0;
      if (Math.abs(vh() - (y + geo.h)) < MAGNET) y = vh() - geo.h;
      geo = { ...geo, x, y };
      place();
      zone = zoneAt(e.clientX, e.clientY);
      showPreview(zone);
    };
    const onUp = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onUp);
      root.classList.remove('fw-moving');
      showPreview(null);
      if (!moved) return;
      if (zone) snap = zone;
      geo = clampGeo(geo);
      place(true);
      persist();
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onUp);
  }
  title.addEventListener('pointerdown', (ev) => {
    if (ev.button !== 0 || (ev.target as HTMLElement).closest('.fw-btn')) return;
    ev.preventDefault();
    const g = current();
    beginMove(ev, ev.clientX - g.x, ev.clientY - g.y, false);
  });
  title.addEventListener('dblclick', (ev) => {
    if ((ev.target as HTMLElement).closest('.fw-btn')) return;
    toggleMax();
  });

  // ---- resize (edges / corners) ----
  root.querySelectorAll<HTMLElement>('.fw-h').forEach((h) => {
    h.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return;
      ev.preventDefault();
      h.setPointerCapture(ev.pointerId);
      const dir = h.dataset.dir || '';
      const g0 = current();
      snap = null; // resizing un-tiles, keeping the tiled size as the start
      geo = g0;
      root.classList.add('fw-moving');
      const onMove = (e: PointerEvent) => {
        const dx = e.clientX - ev.clientX, dy = e.clientY - ev.clientY;
        let { x, y, w, h: hh } = g0;
        if (dir.includes('e')) {
          w = g0.w + dx;
          if (Math.abs(vw() - (x + w)) < MAGNET) w = vw() - x;
        }
        if (dir.includes('s')) {
          hh = g0.h + dy;
          if (Math.abs(vh() - (y + hh)) < MAGNET) hh = vh() - y;
        }
        if (dir.includes('w')) {
          let nx = g0.x + dx;
          if (Math.abs(nx) < MAGNET) nx = 0;
          nx = Math.min(nx, g0.x + g0.w - MIN_W);
          w = g0.x + g0.w - nx; x = nx;
        }
        if (dir.includes('n')) {
          let ny = g0.y + dy;
          if (Math.abs(ny) < MAGNET) ny = 0;
          ny = Math.max(0, Math.min(ny, g0.y + g0.h - MIN_H));
          hh = g0.y + g0.h - ny; y = ny;
        }
        geo = { x, y, w: Math.max(MIN_W, Math.min(w, vw() - x)), h: Math.max(MIN_H, Math.min(hh, vh() - y)) };
        place();
      };
      const onUp = () => {
        h.removeEventListener('pointermove', onMove);
        h.removeEventListener('pointerup', onUp);
        h.removeEventListener('pointercancel', onUp);
        root.classList.remove('fw-moving');
        geo = clampGeo(geo);
        place();
        persist();
      };
      h.addEventListener('pointermove', onMove);
      h.addEventListener('pointerup', onUp);
      h.addEventListener('pointercancel', onUp);
    });
  });

  function toggleMax() {
    snap = snap === 'max' ? null : 'max';
    place(true);
    persist();
  }
  (root.querySelector('.fw-max') as HTMLElement).addEventListener('click', toggleMax);
  (root.querySelector('.fw-min') as HTMLElement).addEventListener('click', () => minimize());
  root.querySelector('.fw-close')?.addEventListener('click', () => opts.onClose?.());
  dock.addEventListener('click', () => {
    if (minimized) open();
    else if (+root.style.zIndex !== zTop) raise(); // behind another window: bring it up first
    else minimize();
  });

  // ---- minimize / restore ("magic lamp" into the dock button) ----
  let anim: Animation | null = null;
  function lampFrames(): { frames: Keyframe[]; origin: string } {
    const r = current(), d = dock.getBoundingClientRect();
    const ox = d.left + d.width / 2 - r.x, oy = d.top + d.height / 2 - r.y;
    const W = r.w, H = r.h;
    const full = `polygon(0px 0px, ${W}px 0px, ${W}px ${H}px, 0px ${H}px)`;
    // The bottom edge pinches toward the dock first, then the whole window is
    // sucked into it: two affine-free phases that read as a genie effect.
    const pinch = `polygon(0px 0px, ${W}px 0px, ${ox + 40}px ${H}px, ${ox - 40}px ${H}px)`;
    const thin = `polygon(${ox - 14}px ${H * 0.15}px, ${ox + 14}px ${H * 0.15}px, ${ox + 6}px ${H}px, ${ox - 6}px ${H}px)`;
    return {
      origin: `${ox}px ${oy}px`,
      frames: [
        { offset: 0, clipPath: full, transform: 'translate(0,0) scale(1)', opacity: 1 },
        { offset: 0.45, clipPath: pinch, transform: 'translate(0,0) scale(1,.92)', opacity: 1 },
        { offset: 1, clipPath: thin, transform: 'scale(.06,.04)', opacity: 0.15 },
      ],
    };
  }
  function bump() {
    if (reduceMotion()) return;
    dock.animate([{ transform: 'scale(1)' }, { transform: 'scale(1.12)' }, { transform: 'scale(1)' }],
      { duration: 260, easing: 'ease-out' });
  }
  function minimize() {
    if (minimized) return;
    minimized = true;
    persist();
    dock.classList.add('fw-dock-on');
    anim?.cancel();
    if (reduceMotion() || !root.animate) { root.hidden = true; return; }
    const { frames, origin } = lampFrames();
    root.style.transformOrigin = origin;
    anim = root.animate(frames, { duration: 420, easing: 'cubic-bezier(.55,.05,.75,.35)' });
    anim.onfinish = () => { root.hidden = true; anim = null; bump(); };
  }
  function open() {
    raise();
    if (!minimized) return;
    minimized = false;
    persist();
    dock.classList.remove('fw-dock-on');
    anim?.cancel();
    root.hidden = false;
    place();
    if (reduceMotion() || !root.animate) return;
    const { frames, origin } = lampFrames();
    root.style.transformOrigin = origin;
    anim = root.animate(frames.slice().reverse().map((f) => ({ ...f, offset: 1 - (f.offset as number) })),
      { duration: 380, easing: 'cubic-bezier(.25,.65,.45,.95)' });
    anim.onfinish = () => { anim = null; };
  }

  const onResize = () => { geo = clampGeo(geo); place(); };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape' && !minimized && root.contains(document.activeElement)) minimize();
  };
  window.addEventListener('resize', onResize);
  window.addEventListener('keydown', onKey);

  // Content that sizes itself from its container (charts, the transfer graph)
  // listens to window 'resize'; tell it when this window's body changes size.
  let lastW = 0, lastH = 0, roTimer = 0;
  const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(() => {
    const w = body.clientWidth, h = body.clientHeight;
    if (!w || (w === lastW && h === lastH)) return;
    lastW = w; lastH = h;
    clearTimeout(roTimer);
    roTimer = window.setTimeout(() => window.dispatchEvent(new Event('resize')), 120);
  }) : null;
  ro?.observe(body);

  root.hidden = minimized;
  dock.classList.toggle('fw-dock-on', minimized);
  place();

  return {
    root, tools, body, dockBtn: dock,
    setTitle: (s) => { sub.textContent = s; },
    setBadge: (s) => { badge.textContent = s; },
    open, minimize,
    isMinimized: () => minimized,
    startMove: (ev, offX, offY) => beginMove(ev, offX, offY, true),
    destroy: () => {
      alive = false;
      anim?.cancel();
      ro?.disconnect();
      window.removeEventListener('resize', onResize);
      window.removeEventListener('keydown', onKey);
      root.remove(); preview.remove(); dock.remove();
      try { localStorage.removeItem(opts.storageKey); } catch { /* ignore */ }
    },
  };
}
