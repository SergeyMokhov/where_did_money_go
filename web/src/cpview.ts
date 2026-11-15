// Change-point ("level") view of the cash-flow chart: three stacked charts —
// Net, In, Out — each showing the raw buckets as grey dots, the level the Rust
// engine detected as a stepped line with its 95% band, a +N / −N mark at every
// level change (green = good for your money, red = bad), a red ring around any
// bucket that is unusually high/low for its level, and the shared life-event /
// unusual-activity overlay. All statistics come from the WebAssembly engine.
import uPlot from 'uplot';
import { changepoints, type CpResult, type Gran, type Metric, type Sensitivity } from './engine';

const DAY_MS = 86_400_000;
/** Nominal bucket length, for expressing a per-day level "per bucket". */
const UNIT_DAYS: Record<Gran, number> = { day: 1, week: 7, biweek: 14, month: 365.25 / 12 };
const UNIT_LABEL: Record<Gran, string> = { day: '/day', week: '/wk', biweek: '/2wk', month: '/mo' };
const METRICS: { id: Metric; title: string; hint: string }[] = [
  { id: 'net', title: 'Net (in − out)', hint: 'Higher is better.' },
  { id: 'in', title: 'Money in', hint: 'Higher is better.' },
  { id: 'out', title: 'Money out', hint: 'Lower is better — a rise is marked red.' },
];

export interface CpCtx {
  rows: any[];
  gran: Gran;
  dateMs: (iso: string) => number | null;
  isFlow: (t: any) => boolean;
  flowParts: (t: any) => [number, number];
  drawOverlays: (u: uPlot) => void;
  fmtAxisNum: (v: number) => string;
  fmtMoney: (cents: number) => string;
  dark: boolean;
  onDragRange: (aSec: number, bSec: number) => void;
  /** A click (not a drag) on a bucket dot: list that bucket's transactions. */
  onPickBucket?: (metric: Metric, startIso: string, days: number) => void;
  /** Plot height per chart (px); 220 on the page, larger to fill a window. */
  chartHeight?: number;
}

// ---------- settings (per project, per browser) ----------
interface CpSettings { sens: Sensitivity; minDays: number }
let settings: CpSettings = { sens: 'standard', minDays: 14 };
let settingsKey = 'wdmg.cp.v1';

function loadSettings(projectId: string) {
  settingsKey = `wdmg.cp.v1.${projectId}`;
  try {
    const v = JSON.parse(localStorage.getItem(settingsKey) || 'null');
    if (v && (v.sens === 'standard' || v.sens === 'sensitive')) settings.sens = v.sens;
    if (v && Number.isFinite(v.minDays)) settings.minDays = clampDays(v.minDays);
  } catch { /* storage unavailable: defaults */ }
}
function saveSettings() {
  try { localStorage.setItem(settingsKey, JSON.stringify(settings)); } catch { /* ignore */ }
}
const clampDays = (n: number) => Math.min(365, Math.max(7, Math.round(n)));

export function initCpControls(projectId: string, redraw: () => void) {
  loadSettings(projectId);
  const box = document.getElementById('cp-controls');
  const input = document.getElementById('cp-min-days') as HTMLInputElement | null;
  if (!box || !input) return;
  const sync = () => {
    box.querySelectorAll<HTMLButtonElement>('button[data-sens]').forEach((b) =>
      b.classList.toggle('active', b.dataset.sens === settings.sens));
    input.value = String(settings.minDays);
  };
  sync();
  box.addEventListener('click', (ev) => {
    const b = (ev.target as HTMLElement).closest<HTMLButtonElement>('button[data-sens]');
    if (!b) return;
    settings.sens = b.dataset.sens as Sensitivity;
    saveSettings(); sync(); redraw();
  });
  input.addEventListener('change', () => {
    const n = Number(input.value);
    settings.minDays = Number.isFinite(n) ? clampDays(n) : 14;
    saveSettings(); sync(); redraw();
  });
}

// ---------- series ----------
const isoDay = (ms: number) => new Date(ms).toISOString().slice(0, 10);
const isoMs = (iso: string) => Date.parse(`${iso}T00:00:00Z`);

interface Daily { start: string; net: number[]; in: number[]; out: number[] }

/** Zero-filled daily in/out/net cents over the rows' date span (same flow rules as the raw chart). */
function dailySeries(ctx: CpCtx): Daily | null {
  const hasFlow = ctx.rows.some(ctx.isFlow);
  let lo = Infinity, hi = -Infinity;
  const items: [number, number, number][] = [];
  // Day range spans ALL rows (incl. transfers), like the engine's own series,
  // so the chart and the change_point anomaly rows line up.
  for (const t of ctx.rows) {
    const ms = ctx.dateMs(t.date);
    if (ms == null) continue;
    const day = Math.floor(ms / DAY_MS) * DAY_MS;
    if (day < lo) lo = day;
    if (day > hi) hi = day;
  }
  for (const t of ctx.rows) {
    const include = hasFlow ? ctx.isFlow(t) : t.kind === 'transfer' || t.kind === 'unknown';
    if (!include) continue;
    const ms = ctx.dateMs(t.date);
    if (ms == null) continue;
    const day = Math.floor(ms / DAY_MS) * DAY_MS;
    const [i, o] = ctx.flowParts(t);
    items.push([day, i, o]);
  }
  if (!items.length) return null;
  const n = Math.round((hi - lo) / DAY_MS) + 1;
  const d: Daily = { start: isoDay(lo), net: new Array(n).fill(0), in: new Array(n).fill(0), out: new Array(n).fill(0) };
  for (const [day, i, o] of items) {
    const k = Math.round((day - lo) / DAY_MS);
    d.in[k] += i; d.out[k] += o; d.net[k] += i - o;
  }
  return d;
}

// Memo so resize/theme redraws don't recompute the statistics.
let memo: { rows: any[]; key: string; res: Record<Metric, CpResult | Error> } | null = null;

function compute(ctx: CpCtx, daily: Daily): Record<Metric, CpResult | Error> {
  const key = `${ctx.gran}|${settings.sens}|${settings.minDays}`;
  if (memo && memo.rows === ctx.rows && memo.key === key) return memo.res;
  const res = {} as Record<Metric, CpResult | Error>;
  for (const m of METRICS) {
    try {
      res[m.id] = changepoints({
        start: daily.start, daily: daily[m.id], metric: m.id,
        sensitivity: settings.sens, min_days: settings.minDays, bucket: ctx.gran,
      });
    } catch (e) {
      res[m.id] = e instanceof Error ? e : new Error(String(e));
    }
  }
  memo = { rows: ctx.rows, key, res };
  return res;
}

// ---------- formatting ----------
function compactMoney(ctx: CpCtx, cents: number, signed = false): string {
  const d = cents / 100;
  const sign = d < 0 ? '−' : signed ? '+' : '';
  const a = Math.abs(d);
  const body = a >= 1000 ? ctx.fmtAxisNum(Math.round(a / 10) * 10) : a >= 100 ? a.toFixed(0) : a.toFixed(a >= 10 ? 0 : 2);
  return `${sign}$${body}`;
}
const pct = (x: number) => `${Math.round(x * 100)}%`;
const wilson = (w: { k: number; n: number; lo: number; hi: number } | null | undefined) =>
  w && w.n ? `${w.k}/${w.n} (95%: ${pct(w.lo)}–${pct(w.hi)})` : '—';

// ---------- drawing ----------
const plots: uPlot[] = [];
let dragging: { u: uPlot; onDrag: CpCtx['onDragRange'] } | null = null;
let mouseupBound = false;

function palette(dark: boolean) {
  return dark
    ? { axis: '#9aa3b2', dot: 'rgba(154,163,178,.55)', dotHollow: 'rgba(154,163,178,.35)', level: '#e7ebf2',
        band: 'rgba(231,235,242,.10)', normal: 'rgba(110,168,254,.07)', normalEdge: 'rgba(110,168,254,.22)', tent: '#d9a441', pos: '#3fc98a', neg: '#ff6b81', ring: '#ff4d5e', pillBg: '#171c25' }
    : { axis: '#6b7280', dot: 'rgba(107,114,128,.5)', dotHollow: 'rgba(107,114,128,.3)', level: '#1c2230',
        band: 'rgba(28,34,48,.08)', normal: 'rgba(37,99,235,.06)', normalEdge: 'rgba(37,99,235,.20)', tent: '#b07a14', pos: '#10915a', neg: '#c2384b', ring: '#d0263a', pillBg: '#ffffff' };
}

export function drawChangePoints(host: HTMLElement, ctx: CpCtx) {
  plots.splice(0).forEach((p) => p.destroy());
  host.replaceChildren();
  const daily = dailySeries(ctx);
  if (!daily) return;
  const results = compute(ctx, daily);
  const unit = UNIT_DAYS[ctx.gran];
  const pal = palette(ctx.dark);
  const width = host.clientWidth || host.parentElement?.clientWidth || 640;

  if (!mouseupBound) {
    mouseupBound = true;
    document.addEventListener('mouseup', () => {
      if (!dragging) return;
      const { u, onDrag } = dragging;
      dragging = null;
      if (u.select && u.select.width > 8) {
        onDrag(u.posToVal(u.select.left, 'x'), u.posToVal(u.select.left + u.select.width, 'x'));
      }
    });
  }

  for (const m of METRICS) {
    const section = document.createElement('div');
    section.className = 'cp-section';
    const head = document.createElement('div');
    head.className = 'cp-head';
    const chartBox = document.createElement('div');
    section.append(head, chartBox);
    host.append(section);

    const r = results[m.id];
    if (r instanceof Error) {
      head.innerHTML = `<b>${m.title}</b>`;
      const err = document.createElement('div');
      err.className = 'cp-err';
      err.textContent = `Couldn't compute levels: ${r.message}`;
      section.append(err);
      continue;
    }
    renderHead(head, m, r, ctx, unit);

    const xs: number[] = [];
    const ys: number[] = [];
    for (const b of r.buckets) {
      xs.push((isoMs(b.start) + (b.days * DAY_MS) / 2) / 1000);
      ys.push(b.value / 100);
    }
    // y range covers dots, levels and their bands.
    let ymin = Math.min(0, ...ys), ymax = Math.max(0, ...ys);
    for (const s of r.segments) {
      const [a, b] = s.ci95 ?? [s.level_per_day, s.level_per_day];
      ymin = Math.min(ymin, (a * unit) / 100);
      ymax = Math.max(ymax, (b * unit) / 100);
    }
    // ...and the normal-range corridor, so its edges are never clipped.
    for (const b of r.buckets) {
      if (b.lo != null) ymin = Math.min(ymin, b.lo / 100);
      if (b.hi != null) ymax = Math.max(ymax, b.hi / 100);
    }
    const pad = (ymax - ymin) * 0.08 || 1;

    const tip = makeTooltip(chartBox, r, ctx, unit);
    const opts: uPlot.Options = {
      width, height: ctx.chartHeight ?? 220,
      legend: { show: false },
      cursor: { sync: { key: 'wdmg-cp' }, drag: { x: true, y: false, setScale: false }, points: { size: 7 } },
      scales: { x: { time: true }, y: { range: () => [ymin - pad, ymax + pad] } },
      axes: [
        { stroke: pal.axis, grid: { stroke: ctx.dark ? '#262d39' : '#e6e8ec' } },
        { stroke: pal.axis, size: 58, grid: { stroke: ctx.dark ? '#262d39' : '#e6e8ec' },
          values: (_u, vals) => vals.map(ctx.fmtAxisNum) },
      ],
      series: [
        {},
        {
          label: 'Bucket', stroke: pal.dot, width: 0,
          paths: () => null,
          points: { show: true, size: 6, stroke: pal.dot, fill: pal.dot, width: 1 },
        },
      ],
      hooks: {
        ready: [(u) => {
          u.over.addEventListener('mousedown', () => { dragging = { u, onDrag: ctx.onDragRange }; });
          u.over.addEventListener('mouseleave', () => { tip.el.hidden = true; });
          u.over.addEventListener('click', () => {
            if (u.select && u.select.width > 8) return;
            const i = u.cursor.idx;
            const b = i == null ? null : r.buckets[i];
            if (b) ctx.onPickBucket?.(m.id, b.start, b.days);
          });
        }],
        setCursor: [tip.hook],
        draw: [(u) => { drawLevels(u, r, unit, pal, ctx); ctx.drawOverlays(u); }],
      },
    };
    plots.push(new uPlot(opts, [xs, ys], chartBox));
  }

  const key = document.createElement('div');
  key.className = 'cp-key muted';
  key.innerHTML = `<span class="cp-key-dot"></span> bucket
    <span class="cp-key-level"></span> level (shade = 95% band)
    <span class="cp-key-normal"></span> normal range for that level
    <span class="cp-key-ring"></span> outside it`;
  host.append(key);
}

function renderHead(head: HTMLElement, m: (typeof METRICS)[number], r: CpResult, ctx: CpCtx, unit: number) {
  const u = UNIT_LABEL[ctx.gran];
  const last = r.segments[r.segments.length - 1];
  const parts: string[] = [`<b>${m.title}</b> <span class="muted">${m.hint}</span>`];
  if (last) {
    parts.push(`<span class="cp-stat">Current level <b>${compactMoney(ctx, last.level_per_day * unit)}${u}</b>
      <span class="muted">since ${last.start}${last.ci95 ? ` (95%: ${compactMoney(ctx, last.ci95[0] * unit)}–${compactMoney(ctx, last.ci95[1] * unit)})` : ''}</span></span>`);
  }
  if (r.trend) {
    // slope is cents/day per day → $ per bucket, per 30 days.
    const perMonth = r.trend.slope * unit * 30.44;
    const dir = r.trend.direction;
    const good = m.id === 'out' ? perMonth < 0 : perMonth > 0;
    const cls = dir === 'flat' ? 'muted' : good ? 'pos' : 'neg';
    parts.push(`<span class="cp-stat">Trend within level: <b class="${cls}">${dir}${dir === 'flat' ? '' : ` ${compactMoney(ctx, perMonth, true)}${u} per month`}</b></span>`);
  }
  if (r.tentative) {
    const t = r.tentative;
    parts.push(`<span class="cp-stat cp-tent">Possible new level forming since ${t.start} (${t.days}d of ${settings.minDays}):
      <b class="${t.good ? 'pos' : 'neg'}">${compactMoney(ctx, t.delta_per_day * unit, true)}${u}</b></span>`);
  }
  const effDays = Math.max(settings.minDays, 2 * r.block_days);
  if (effDays > settings.minDays) {
    parts.push(`<span class="cp-stat muted" title="This series is analysed in ${r.block_kind === 'calendar_month' ? 'calendar-month' : `${r.block_days}-day`} blocks (chosen because paydays/bills make shorter blocks too noisy), and a level needs at least 2 blocks.">Effective minimum level: ~${effDays} days ⓘ</span>`);
  }
  head.innerHTML = parts.join('');

  // Per-level table: dates, level, 95% band, and Wilson-95 rates.
  const det = document.createElement('details');
  det.className = 'cp-levels';
  const sum = document.createElement('summary');
  sum.textContent = `${r.segments.length} level${r.segments.length === 1 ? '' : 's'} · ${r.changes.length} change${r.changes.length === 1 ? '' : 's'}`;
  det.append(sum);
  const tbl = document.createElement('table');
  const negCol = m.id === 'net';
  tbl.innerHTML = `<thead><tr><th>From</th><th>To</th><th class="num">Level${u}</th><th class="num">95% band</th>
    <th class="num">Change</th><th>Unusual buckets</th>${negCol ? '<th>Weeks net-negative</th>' : ''}</tr></thead>`;
  const tb = document.createElement('tbody');
  r.segments.forEach((s, i) => {
    const ch = i > 0 ? r.changes[i - 1] : null;
    const tr = document.createElement('tr');
    const cells = [
      s.start, s.end,
      compactMoney(ctx, s.level_per_day * unit),
      s.ci95 ? `${compactMoney(ctx, s.ci95[0] * unit)}–${compactMoney(ctx, s.ci95[1] * unit)}` : '—',
      ch ? compactMoney(ctx, ch.delta_per_day * unit, true) : '—',
      wilson(s.unusual_buckets),
      ...(negCol ? [wilson(s.neg_weeks)] : []),
    ];
    cells.forEach((c, j) => {
      const td = document.createElement('td');
      td.textContent = c;
      if (j >= 2 && j <= 4) td.className = 'num';
      if (j === 4 && ch) td.classList.add(ch.good ? 'pos' : 'neg');
      tr.append(td);
    });
    tb.append(tr);
  });
  tbl.append(tb);
  det.append(tbl);
  head.append(det);
}

type Pal = ReturnType<typeof palette>;

function drawLevels(u: uPlot, r: CpResult, unit: number, pal: Pal, ctx: CpCtx) {
  const c = u.ctx;
  const dpr = uPlot.pxRatio || window.devicePixelRatio || 1;
  const { left, top, width, height } = u.bbox;
  const right = left + width, bot = top + height;
  const xmin = u.scales.x.min ?? 0, xmax = u.scales.x.max ?? 0;
  const X = (iso: string, plusDays = 0) => u.valToPos(isoMs(iso) / 1000 + plusDays * 86400, 'x', true);
  const Y = (cents: number) => u.valToPos((cents * unit) / 100, 'y', true);
  const clampX = (px: number) => Math.min(right, Math.max(left, px));

  c.save();
  c.beginPath(); c.rect(left, top, width, height); c.clip();

  // Normal range (XmR natural limits) as a faint tinted corridor behind
  // everything. It is per bucket, so it steps with each level and scales with
  // the bucket length; red rings mark dots outside it.
  const Yc = (cents: number) => u.valToPos(cents / 100, 'y', true);
  c.fillStyle = pal.normal;
  c.strokeStyle = pal.normalEdge; c.lineWidth = 1 * dpr; c.setLineDash([2 * dpr, 3 * dpr]);
  let edgeOpen = false;
  let prevSeg = -1;
  const edges: [number, number, number, number][] = [];
  for (const b of r.buckets) {
    if (b.lo == null || b.hi == null) { edgeOpen = false; continue; }
    const x1 = clampX(X(b.start)), x2 = clampX(X(b.start, b.days));
    if (x2 <= left || x1 >= right) continue;
    const yt = Yc(b.hi), yb = Yc(b.lo);
    c.fillRect(x1, yt, Math.max(1, x2 - x1), yb - yt);
    const joined = edgeOpen && prevSeg === b.segment;
    edges.push([joined ? 1 : 0, x1, x2, yt], [joined ? 1 : 0, x1, x2, yb]);
    edgeOpen = true; prevSeg = b.segment;
  }
  // Dotted edges, drawn as two stepped polylines (top, bottom) per segment run.
  for (const off of [0, 1]) {
    c.beginPath();
    for (let k = off; k < edges.length; k += 2) {
      const [joined, x1, x2, y] = edges[k];
      if (joined) c.lineTo(x1, y); else c.moveTo(x1, y);
      c.lineTo(x2, y);
    }
    c.stroke();
  }
  c.setLineDash([]);

  // 95% bands + level steps.
  r.segments.forEach((s, i) => {
    const x1 = clampX(X(s.start)), x2 = clampX(X(s.end, 1));
    if (x2 <= left || x1 >= right) return;
    if (s.ci95) {
      c.fillStyle = pal.band;
      const yb1 = Y(s.ci95[1]), yb2 = Y(s.ci95[0]);
      c.fillRect(x1, Math.min(yb1, yb2), Math.max(1, x2 - x1), Math.abs(yb2 - yb1));
    }
    c.strokeStyle = pal.level; c.lineWidth = 2 * dpr; c.setLineDash([]);
    const y = Y(s.level_per_day);
    c.beginPath(); c.moveTo(x1, y); c.lineTo(x2, y);
    // Vertical riser into the next level so it reads as one stepped line.
    const nx = r.segments[i + 1];
    if (nx) c.lineTo(x2, Y(nx.level_per_day));
    c.stroke();
  });

  // Tentative level still forming: dashed.
  if (r.tentative) {
    const t = r.tentative;
    const x1 = clampX(X(t.start)), x2 = clampX(X(t.start, t.days));
    const y = Y(t.level_per_day);
    c.strokeStyle = pal.tent; c.lineWidth = 2 * dpr; c.setLineDash([5 * dpr, 4 * dpr]);
    c.beginPath(); c.moveTo(x1, y); c.lineTo(Math.max(x2, x1 + 6 * dpr), y); c.stroke();
    c.setLineDash([]);
  }

  // Red rings around unusual buckets (drawn over the dots).
  const xs = u.data[0] as number[];
  const ys = u.data[1] as number[];
  c.strokeStyle = pal.ring; c.lineWidth = 2 * dpr;
  r.buckets.forEach((b, i) => {
    if (!b.unusual) return;
    const xv = xs[i];
    if (xv < xmin || xv > xmax) return;
    const px = u.valToPos(xv, 'x', true), py = u.valToPos(ys[i], 'y', true);
    c.beginPath(); c.arc(px, py, 7 * dpr, 0, Math.PI * 2); c.stroke();
  });

  // +N / −N change marks: a pill next to the new level, stacked to avoid overlap.
  c.font = `${Math.round(11 * dpr)}px system-ui, -apple-system, sans-serif`;
  c.textBaseline = 'middle';
  const placed: { x1: number; x2: number; y: number }[] = [];
  r.changes.forEach((ch, i) => {
    const seg = r.segments[i + 1];
    if (!seg) return;
    const ts = isoMs(ch.date) / 1000;
    if (ts < xmin || ts > xmax) return;
    const x = X(ch.date);
    const label = `${compactMoney(ctx, ch.delta_per_day * unit, true)}${UNIT_LABEL[ctx.gran]}`;
    const w = c.measureText(label).width + 10 * dpr, h = 16 * dpr;
    const up = ch.delta_per_day > 0;
    let y = Y(seg.level_per_day) + (up ? -14 : 14) * dpr;
    let x1 = Math.min(right - w, Math.max(left, x + 4 * dpr));
    // Nudge vertically away from previously placed pills.
    for (let tries = 0; tries < 6; tries++) {
      const hit = placed.find((p) => x1 < p.x2 && x1 + w > p.x1 && Math.abs(p.y - y) < h + 2 * dpr);
      if (!hit) break;
      y += (up ? -1 : 1) * (h + 3 * dpr);
    }
    y = Math.min(bot - h / 2, Math.max(top + h / 2, y));
    placed.push({ x1, x2: x1 + w, y });
    const col = ch.good ? pal.pos : pal.neg;
    c.fillStyle = pal.pillBg; c.strokeStyle = col; c.lineWidth = 1.5 * dpr;
    roundRect(c, x1, y - h / 2, w, h, 6 * dpr);
    c.fill(); c.stroke();
    c.fillStyle = col; c.textAlign = 'left';
    c.fillText(label, x1 + 5 * dpr, y + 0.5 * dpr);
  });
  c.restore();
}

function roundRect(c: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  c.beginPath();
  c.moveTo(x + r, y);
  c.arcTo(x + w, y, x + w, y + h, r);
  c.arcTo(x + w, y + h, x, y + h, r);
  c.arcTo(x, y + h, x, y, r);
  c.arcTo(x, y, x + w, y, r);
  c.closePath();
}

/** Small hover readout: bucket value vs its level's expected range. */
function makeTooltip(box: HTMLElement, r: CpResult, ctx: CpCtx, unit: number) {
  const tip = document.createElement('div');
  tip.className = 'cp-tip';
  tip.hidden = true;
  box.style.position = 'relative';
  box.append(tip);
  const hook = (uu: uPlot) => {
    const i = uu.cursor.idx;
    if (i == null || !r.buckets[i]) { tip.hidden = true; return; }
    const b = r.buckets[i];
    const seg = r.segments[b.segment];
    const lines = [
      `<b>${b.start}</b>${b.partial ? ' <span class="muted">(partial)</span>' : ''}`,
      `Bucket: <b>${compactMoney(ctx, b.value)}</b>`,
      seg ? `Level: ${compactMoney(ctx, seg.level_per_day * unit)}${UNIT_LABEL[ctx.gran]}` : '',
      b.lo != null && b.hi != null ? `Normal range: ${compactMoney(ctx, b.lo)} – ${compactMoney(ctx, b.hi)}` : '',
      b.unusual ? `<span class="neg">Unusually ${b.unusual}</span>` : '',
    ].filter(Boolean);
    tip.innerHTML = lines.join('<br>');
    tip.hidden = false;
    // Never cover the cursor: sit right of it, or flip to its left when the
    // right side lacks room (measured after the content is set).
    const x = (uu.cursor.left ?? 0) + uu.bbox.left / (uPlot.pxRatio || 1);
    const w = tip.offsetWidth;
    const gap = 14;
    const right = x + gap;
    const left = right + w <= box.clientWidth ? right : Math.max(0, x - gap - w);
    tip.style.left = `${left}px`;
    tip.style.top = '8px';
  };
  return { el: tip, hook };
}
