// "Yank" a report section off the page into a floating window (txwin.ts): click
// its ⇱, or drag it away. Two kinds of section:
//  - an `h2` followed by a `.panel`: the heading is the handle, the panel moves;
//  - an element with `data-yank-title` (e.g. the filter bar): the element itself
//    moves, and dragging its empty background tears it off.
// The node is moved — not copied — so it keeps updating with the filters. A
// placeholder holds its spot; ✕ on the window flies it back there. Popped-out
// sections are remembered per browser and re-opened on load.
import { createFloatWin, type FloatWin } from './txwin';

const LIST_KEY = 'wdmg.yank.list.v1';
const geoKey = (slug: string) => `wdmg.yank.v1.${slug}`;
const TEAR_PX = 28; // drag distance that tears a section off the page
// Inside a self-contained section, pressing these starts no tear-off.
const INTERACTIVE = 'button,input,select,textarea,label,a,.facet,.pill,.help,.yank-btn,[data-pick]';

interface Section { slug: string; title: string; head: HTMLElement; panel: HTMLElement; self: boolean }
interface Popped { win: FloatWin; ph: HTMLElement }
const popped = new Map<string, Popped>();

const slugify = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40) || 'section';
const vh = () => window.innerHeight, vw = () => document.documentElement.clientWidth;
const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
const reduceMotion = () => !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;

function titleOf(h: HTMLElement): string {
  const c = h.cloneNode(true) as HTMLElement;
  c.querySelectorAll('.caret,.help,.yank-btn').forEach((e) => e.remove());
  return (c.textContent || '').replace(/\s+/g, ' ').trim();
}

function saveList() {
  try { localStorage.setItem(LIST_KEY, JSON.stringify([...popped.keys()])); } catch { /* ignore */ }
}

export function initYank(root: HTMLElement) {
  const host = root;
  const sections: Section[] = [
    ...[...root.querySelectorAll<HTMLElement>('h2')]
      .filter((h) => h.nextElementSibling?.classList.contains('panel'))
      .map((h) => {
        const title = titleOf(h);
        return { slug: slugify(title), title, head: h, panel: h.nextElementSibling as HTMLElement, self: false };
      }),
    ...[...root.querySelectorAll<HTMLElement>('[data-yank-title]')].map((el) => {
      const title = el.dataset.yankTitle || 'Section';
      return { slug: slugify(title), title, head: el, panel: el, self: true };
    }),
  ];

  for (const s of sections) {
    s.head.classList.add('yankable');
    s.head.dataset.yank = s.slug;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'yank-btn';
    btn.title = s.self ? 'Pop out into a window (or drag its background off the page)'
      : 'Pop out into a window (or drag this heading off the page)';
    btn.setAttribute('aria-label', `Pop out “${s.title}”`);
    btn.textContent = '⇱';
    s.head.append(btn);
    btn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      const p = popped.get(s.slug);
      if (p) p.win.open(); else yank(s, null);
    });
    // A popped-out section's heading on the page just brings its window up.
    if (!s.self) s.head.addEventListener('click', () => { popped.get(s.slug)?.win.open(); });

    // Tear-off: drag far enough and the section becomes a window under the pointer.
    s.head.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0 || ev.pointerType === 'touch' || popped.has(s.slug)) return;
      const t = ev.target as HTMLElement;
      if (s.self ? t.closest(INTERACTIVE) : t.closest('.help,.yank-btn')) return;
      const onMove = (e: PointerEvent) => {
        if (Math.hypot(e.clientX - ev.clientX, e.clientY - ev.clientY) < TEAR_PX) return;
        stop();
        const win = yank(s, { x: e.clientX, y: e.clientY });
        win.startMove(e, 90, 16);
      };
      const stop = () => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', stop);
        window.removeEventListener('pointercancel', stop);
      };
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', stop);
      window.addEventListener('pointercancel', stop);
    });
  }

  function yank(s: Section, at: { x: number; y: number } | null, restoring = false): FloatWin {
    const { panel, slug, title } = s;
    const r = panel.getBoundingClientRect();
    const w = clamp(Math.max(r.width, 420) + 2, 360, vw() - 16);
    const hh = clamp((panel.scrollHeight || r.height) + 48, s.self ? 160 : 220, Math.round(vh() * 0.8));
    const geo = at
      ? { x: at.x - 90, y: at.y - 16, w, h: hh }
      : { x: clamp(r.left, 0, vw() - w), y: clamp(r.top, 0, vh() - hh), w, h: hh };
    if (!restoring) { try { localStorage.removeItem(geoKey(slug)); } catch { /* ignore */ } }

    const ph = document.createElement('div');
    ph.className = 'yank-ph';
    ph.innerHTML = `<span class="muted">${s.self ? `${title} are` : 'Shown'} in a window.</span> `;
    const show = Object.assign(document.createElement('button'), { type: 'button', className: 'btn small', textContent: 'Show' });
    const back = Object.assign(document.createElement('button'), { type: 'button', className: 'btn small', textContent: 'Put back here' });
    ph.append(show, ' ', back);

    const win = createFloatWin({
      id: `yank-${slug}`, title, dockLabel: title.length > 22 ? title.slice(0, 21) + '…' : title,
      storageKey: geoKey(slug), host, geo, startOpen: true, onClose: () => putBack(s),
    });
    win.root.classList.add('fw-section', `fw-sec-${slug}`);
    show.addEventListener('click', () => win.open());
    back.addEventListener('click', () => putBack(s));

    if (!s.self) s.head.classList.add('yanked');
    panel.classList.remove('sec-closed');
    panel.before(ph);
    win.body.append(panel);
    win.tools.remove();
    popped.set(slug, { win, ph });
    saveList();
    return win;
  }

  function putBack(s: Section) {
    const p = popped.get(s.slug);
    if (!p) return;
    popped.delete(s.slug);
    saveList();
    const finish = () => {
      p.ph.replaceWith(s.panel);
      if (!s.self) s.head.classList.remove('yanked', 'closed');
      p.win.destroy();
      window.dispatchEvent(new Event('resize')); // charts re-measure their page width
      if (!reduceMotion()) {
        s.panel.animate([{ boxShadow: '0 0 0 3px var(--accent)' }, { boxShadow: '0 0 0 0 transparent' }], { duration: 900, easing: 'ease-out' });
      }
    };
    if (p.win.isMinimized() || reduceMotion() || !p.win.root.animate) { finish(); return; }
    // Fly the window back into its spot on the page.
    p.ph.scrollIntoView({ block: 'nearest' });
    const a = p.win.root.getBoundingClientRect(), b = p.ph.getBoundingClientRect();
    p.win.root.style.transformOrigin = '0 0';
    const anim = p.win.root.animate([
      { transform: 'translate(0,0) scale(1)', opacity: 1 },
      { transform: `translate(${b.left - a.left}px,${b.top - a.top}px) scale(${b.width / a.width},${Math.max(b.height, 24) / a.height})`, opacity: 0.2 },
    ], { duration: 320, easing: 'cubic-bezier(.4,0,.2,1)' });
    anim.onfinish = finish;
  }

  // Re-open what was popped out last time.
  let list: string[] = [];
  try { list = JSON.parse(localStorage.getItem(LIST_KEY) || '[]'); } catch { /* ignore */ }
  for (const slug of Array.isArray(list) ? list : []) {
    const s = sections.find((x) => x.slug === slug);
    if (s && !popped.has(slug)) yank(s, null, true);
  }
}
