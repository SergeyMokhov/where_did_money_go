// End-to-end check of the built single-file app (dist/index.html) in headless
// Firefox, opened via file:// exactly as a user would. Usage:
//   yarn build && node scripts/verify-e2e.mjs [outDir]
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer-core';
import { zipSync } from 'fflate';

const here = path.dirname(fileURLToPath(import.meta.url));
const APP = 'file://' + path.resolve(here, '../dist/index.html');
const SAMPLE = path.resolve(here, '../../playground/data/joint_chequing.csv');
const OUT = process.argv[2] || '/tmp/wdmg-e2e';
fs.mkdirSync(OUT, { recursive: true });

let failed = 0;
const step = (ok, what, observed) => {
  if (!ok) failed++;
  console.log(`${ok ? 'OK  ' : 'FAIL'} | ${what} | ${observed}`);
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const browser = await puppeteer.launch({ browser: 'firefox', executablePath: '/usr/bin/firefox', headless: true });
const page = await browser.newPage();
await page.setViewport({ width: 1360, height: 1000 });
const errors = [];
page.on('pageerror', (e) => errors.push(e.message));
page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
page.on('dialog', (d) => d.accept());
const shot = (name, full = false) => page.screenshot({ path: `${OUT}/${name}.png`, fullPage: full });
const visible = (sel) => page.$eval(sel, (el) => !el.hidden && getComputedStyle(el).display !== 'none').catch(() => false);
const clickText = async (sel, text) => {
  const ok = await page.$$eval(sel, (els, t) => {
    const e = els.find((x) => x.textContent.trim().includes(t));
    if (e) e.click();
    return !!e;
  }, text);
  if (!ok) throw new Error(`no ${sel} containing "${text}"`);
};

try {
// 1. Demo report renders from the in-browser Rust engine.
await page.goto(APP);
await page.waitForFunction(() => !document.getElementById('report-root').hidden, { timeout: 30000 });
const cards = await page.$$eval('#cards .card', (c) => c.length);
const proj = await page.$eval('#proj-name', (e) => e.textContent);
const recs = await page.$$eval('#recs-panel .rec', (r) => r.length);
step(cards > 0 && /Demo/.test(proj), 'demo report renders', `cards=${cards} project="${proj}" recs=${recs}`);
await shot('01-raw');

// Transactions window: starts minimized to the dock; picks open it with exactly
// the transactions behind the number.
const winOpen = () => page.$eval('#txwin', (w) => !w.hidden);
const total0 = await page.$eval('#txwin .tw-count', (e) => +e.textContent);
step(!(await winOpen()) && (await visible('#txwin-dock')) && !(await page.$('#all-txns-panel')),
  'transactions live in a minimized floating window (no All-transactions section)', `rows=${total0}`);
for (const [panel, sel, side] of [['spend-bars', '.cathead', 'out'], ['spend-bars', '.barrow.sub', 'out'], ['income-panel', '.cathead', 'in']]) {
  const want = await page.$eval(`#${panel} ${sel}`, (r) => { r.click(); return r.querySelector('.amt span').textContent; });
  await page.waitForFunction(() => !document.getElementById('txwin').hidden);
  const got = await page.$eval('#txwin', (w, side) => ({
    total: w.querySelector(`.tw-${side}`)?.textContent, n: +w.querySelector('.tw-count').textContent,
    rows: w.querySelectorAll('tbody tr').length, chip: w.querySelector('.tw-chip')?.textContent,
  }), side);
  step(got.total === want && got.rows > 0 && got.n < total0 && !!got.chip, `pick shows its transactions (${panel} ${sel})`,
    `bar=${want} window=${got.total} n=${got.n} chip="${got.chip}"`);
}
await sleep(500);
await shot('01b-txwin');
// Other sections are picks too (top merchants, by person).
await page.$eval('#merchants-panel tbody tr', (r) => r.click());
const mer = await page.$eval('#txwin', (w) => ({ chip: w.querySelector('.tw-chip')?.textContent || '',
  mers: [...new Set([...w.querySelectorAll('tbody td.mer')].map((td) => td.textContent))] }));
step(mer.mers.length === 1 && mer.chip.includes(mer.mers[0]), 'top-merchant row lists that merchant only', JSON.stringify(mer));
await page.$eval('#txwin .tw-x', (b) => b.click());
step((await page.$eval('#txwin .tw-count', (e) => +e.textContent)) === total0, 'clearing the pick shows all filtered rows', `${total0}`);

// Lines in "Money between accounts" are picks: their transfer legs.
const edges = await page.$$eval('#flow-graph .edge-g', (gs) => gs.map((g) => /: ([\d,.]+) in/.exec(g.querySelector('title').textContent)[1]));
const flow = [];
for (let i = 0; i < Math.min(edges.length, 4); i++) {
  await page.$$eval('#flow-graph .edge-g', (gs, i) => gs[i].dispatchEvent(new MouseEvent('click', { bubbles: true })), i);
  const w = await page.$eval('#txwin', (w) => ({ out: w.querySelector('.tw-out')?.textContent, chip: w.querySelector('.tw-chip')?.textContent,
    kinds: [...new Set([...w.querySelectorAll('tbody .tag')].map((t) => t.textContent))] }));
  const picked = await page.$$eval('#flow-graph .edge-g', (gs, i) => gs[i].classList.contains('picked'), i);
  const inn = await page.$eval('#txwin .tw-in', (e) => e.textContent).catch(() => null);
  flow.push({ line: edges[i], ...w, picked, ok: (edges[i] === w.out || edges[i] === inn) && w.kinds.join() === 'transfer' && picked });
}
step(flow.length > 0 && flow.every((f) => f.ok), 'clicking a transfer line lists its legs (amount matches)', JSON.stringify(flow.map((f) => [f.line, f.out, f.chip])));
await page.$eval('#txwin .tw-x', (b) => b.click());
// Dots drag within the graph and the ribbons follow; the layout is remembered.
await page.$eval('h2[data-yank="money-between-your-accounts-people"]', (h) => { h.classList.remove('closed'); h.nextElementSibling.classList.remove('sec-closed'); document.getElementById('flow-graph').scrollIntoView({ block: 'center' }); });
await sleep(200);
const nb = await page.$eval('#flow-graph .node-g .node', (c) => { const r = c.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; });
const d0 = await page.$$eval('#flow-graph .ribbon', (ps) => ps.map((p) => p.getAttribute('d')).join());
await page.mouse.move(nb.x, nb.y); await page.mouse.down();
await page.mouse.move(nb.x - 150, nb.y + 120, { steps: 8 }); await page.mouse.up();
const moved = await page.$eval('#flow-graph .node-g .node', (c) => { const r = c.getBoundingClientRect(); return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }; });
const d1 = await page.$$eval('#flow-graph .ribbon', (ps) => ps.map((p) => p.getAttribute('d')).join());
const noPick = !(await page.$eval('#txwin .tw-chip', () => true).catch(() => false));
step(Math.abs(moved.x - (nb.x - 150)) < 3 && Math.abs(moved.y - (nb.y + 120)) < 3 && d0 !== d1 && noPick && await visible('.flow-reset'),
  'transfer-graph dots drag (ribbons follow, no accidental pick)', JSON.stringify({ from: nb, to: moved, ribbonsMoved: d0 !== d1, noPick, reset: await visible('.flow-reset') }));
await page.$eval('.flow-reset', (b) => b.click());
// Every account is on the circle (even with no transfers); unresolved legs are dashed.
const circ = await page.evaluate(() => ({
  names: [...document.querySelectorAll('#flow-graph .node-lbl')].map((t) => t.textContent),
  inferred: document.querySelectorAll('#flow-graph .edge-g.inferred').length,
}));
step(circ.names.includes('Bender Mastercard') && circ.names.includes('BIG BERTHA BOT') && circ.inferred > 0,
  'transfer circle shows lone accounts and inferred parties', JSON.stringify(circ));
// Click a name → focus view; drill category → subcategory → merchants; follow a party; back.
// A REAL mouse click on the name (press → release): a synthetic click on the text
// skips the dot's pointer capture and once hid a bug where the name only picked.
const label = async (txt) => {
  if (!(await page.$eval('#txwin', (w) => w.hidden))) { await page.$eval('#txwin .fw-min', (b) => b.click()); await sleep(600); }
  const pt = await page.$$eval('#flow-graph .node-lbl', (ts, txt) => {
    const t = ts.find((x) => x.textContent === txt); t.scrollIntoView({ block: 'center' });
    const r = t.getBoundingClientRect(); const x = r.left + r.width / 2, y = r.top + r.height / 2;
    return { x, y, hit: document.elementFromPoint(x, y) === t };
  }, txt);
  if (!pt.hit) throw new Error(`name "${txt}" is covered at its position`);
  await page.mouse.click(pt.x, pt.y);
};

const fnode = (pre) => page.$$eval('.focus-svg .fnode', (gs, pre) => {
  const g = gs.find((x) => x.querySelector('.fn-lbl').textContent.startsWith(pre));
  g.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  return /: ([\d,.]+) ·/.exec(g.querySelector('title').textContent)[1];
}, pre);
const crumbs = () => page.$eval('#flow-crumbs', (c) => c.textContent.replace(/\s+/g, ' ').trim());
const selBefore = await page.$eval('#txwin .tw-chip', (c) => c.textContent).catch(() => null);
await label('Bender Ledger'); await sleep(200);
const c1 = await crumbs();
const selAfter = await page.$eval('#txwin .tw-chip', (c) => c.textContent).catch(() => null);
step(/› Bender Ledger/.test(c1) && selAfter === selBefore, 'clicking a NAME (real mouse) opens explore, not the transaction list', JSON.stringify({ c1, selBefore, selAfter }));
const txState = () => page.$eval('#txwin', (w) => ({ hidden: w.hidden, out: w.querySelector('.tw-out')?.textContent,
  chip: w.querySelector('.tw-chip')?.textContent ?? null, pulse: document.getElementById('txwin-dock').classList.contains('pulse') }));
const indul = await fnode('Indulgence'); await sleep(200);
const w0 = await txState();
step(/› Indulgence/.test(await crumbs()) && w0.hidden && w0.chip === selBefore,
  'drilling into a category leaves the Transactions window alone', JSON.stringify(w0));
// A ribbon is a quiet pick: the minimized window stays minimized, its dock button pulses.
await page.$eval('.focus-svg .fband[data-qpick$="|R|cat|Indulgence"]', (g) => g.dispatchEvent(new MouseEvent('click', { bubbles: true })));
await sleep(150);
const w1 = await txState();
step(w1.hidden && w1.pulse && w1.out === indul && /Indulgence/.test(w1.chip), 'ribbon picks exactly its transactions without popping the window up', JSON.stringify({ indul, w1 }));
// "≡ N transactions" opens the window on the breadcrumb's level.
await page.$eval('.fc-list', (b) => b.click()); await sleep(600);
const w1b = await txState();
step(!w1b.hidden && /Indulgence/.test(w1b.chip) && /≡ \d+ transactions?/.test(await crumbs()), 'breadcrumb button opens the window on that level', JSON.stringify(w1b));
await page.$eval('#txwin .fw-min', (b) => b.click()); await sleep(600);
await fnode('Lottery'); await sleep(200);
const mers = await page.$$eval('.focus-svg .fn-mer', (g) => g.length);
const c2 = await crumbs();
step(mers > 0 && /Indulgence › Lottery & Gaming/.test(c2), 'drill: subcategory shows its merchants', JSON.stringify({ mers, c2 }));
const merName = await page.$eval('.focus-svg .fn-mer', (g) => { g.dispatchEvent(new MouseEvent('click', { bubbles: true })); return g.querySelector('title').textContent.split(':')[0]; });
await sleep(150);
const w2 = await txState();
step(w2.hidden && w2.chip && w2.chip.includes(merName), 'a merchant (leaf) box picks its transactions quietly', JSON.stringify({ merName, w2 }));
await fnode('BIG BERTHA'); await sleep(200);
const c3 = await crumbs();
await page.$eval('.fc-back', (b) => b.click()); await sleep(200);
const c4 = await crumbs();
step(/BIG BERTHA BOT/.test(c3) && /Back/.test(c3) && /› Bender Ledger/.test(c4), 'follow a party, then Back', JSON.stringify({ c3, c4 }));
await page.$eval('.fc-crumb[data-lvl="all"]', (a) => a.click()); await sleep(200);
await label('Bender Mastercard'); await sleep(200);
const card = await page.evaluate(() => ({ none: [...document.querySelectorAll('.focus-svg .fc-none')].map((t) => t.textContent),
  out: [...document.querySelectorAll('.focus-svg .fn-cat .fn-lbl')].map((t) => t.textContent) }));
step(card.none.includes('nothing in') && card.out.length > 0, 'an account with no transfers still opens (money out only)', JSON.stringify(card));
await page.$eval('.fc-crumb[data-lvl="all"]', (a) => a.click()); await sleep(200);
await page.$eval('#txwin .tw-x', (b) => b.click()).catch(() => {});
await page.$eval('#transfers-panel', (e) => e.scrollIntoView({ block: 'start' })); await sleep(100);
await shot('01e-flow');

// Minimize (animated into the dock) and restore.
if (await page.$eval('#txwin', (w) => w.hidden)) { await page.$eval('#txwin-dock', (b) => b.click()); await sleep(600); }
await page.click('#txwin .fw-min');
await sleep(700);
step(!(await winOpen()), 'minimize hides the window into the dock', 'hidden');
await page.click('#txwin-dock');
await sleep(600);
step(await winOpen(), 'dock button restores it', 'shown');

// Drag the title bar onto the left screen edge: tiles to the left half.
const vw = await page.evaluate(() => document.documentElement.clientWidth);
const beforeTile = await page.$eval('#txwin', (w) => w.getBoundingClientRect().width);
const tb = await page.$eval('#txwin .fw-title', (t) => { const r = t.getBoundingClientRect(); return { x: r.left + 60, y: r.top + 10 }; });
await page.mouse.move(tb.x, tb.y); await page.mouse.down();
await page.mouse.move(tb.x - 200, tb.y, { steps: 5 }); await page.mouse.move(1, 400, { steps: 8 });
const preview = await page.$eval('.fw-snap', (p) => p.classList.contains('on'));
await page.mouse.up(); await sleep(350);
const tiled = await page.$eval('#txwin', (w) => { const r = w.getBoundingClientRect(); return { l: r.left, w: r.width, h: r.height, snap: w.dataset.snap }; });
step(preview && tiled.snap === 'left' && tiled.l === 0 && Math.abs(tiled.w - vw / 2) <= 1, 'dropping on the left edge tiles to the left half', JSON.stringify({ preview, ...tiled }));
await shot('01c-tiled');
// Dragging it away restores the floating size; then resize from the corner.
await page.mouse.move(100, 10); await page.mouse.down();
await page.mouse.move(400, 300, { steps: 8 }); await page.mouse.up(); await sleep(300);
const floated = await page.$eval('#txwin', (w) => { const r = w.getBoundingClientRect(); return { w: r.width, h: r.height, snap: w.dataset.snap }; });
step(!floated.snap && floated.w === beforeTile, 'dragging a tiled window out restores its size', JSON.stringify(floated));
const se = await page.$eval('#txwin .fw-h-se', (h) => { const r = h.getBoundingClientRect(); return { x: r.left + 5, y: r.top + 5 }; });
await page.mouse.move(se.x, se.y); await page.mouse.down();
await page.mouse.move(se.x - 120, se.y - 60, { steps: 6 }); await page.mouse.up();
const resized = await page.$eval('#txwin', (w) => { const r = w.getBoundingClientRect(); return { w: r.width, h: r.height }; });
step(Math.abs(resized.w - (floated.w - 120)) <= 2 && Math.abs(resized.h - (floated.h - 60)) <= 2, 'corner drag resizes', JSON.stringify(resized));
await page.click('#txwin .fw-min'); await sleep(700);

// Click a dot on the raw chart: that bucket's transactions.
await page.$eval('#cashflow-chart', (e) => e.scrollIntoView({ block: 'center' }));
const ov = await page.$eval('#cashflow-chart .u-over', (o) => { const r = o.getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width, h: r.height }; });
await page.mouse.click(ov.x + ov.w * 0.5, ov.y + ov.h * 0.5);
await page.waitForFunction(() => !document.getElementById('txwin').hidden, { timeout: 5000 });
const dot = await page.$eval('#txwin', (w) => ({ chip: w.querySelector('.tw-chip')?.textContent || '', n: +w.querySelector('.tw-count').textContent }));
step(/^(Money in|Money out|Net) · /.test(dot.chip) && dot.n > 0 && dot.n < total0, 'clicking a chart dot lists that bucket', JSON.stringify(dot));
// Brushing the chart narrows the window too (and drops the stale dot pick).
await page.mouse.move(ov.x + ov.w * 0.2, ov.y + ov.h * 0.5); await page.mouse.down();
await page.mouse.move(ov.x + ov.w * 0.45, ov.y + ov.h * 0.5, { steps: 6 }); await page.mouse.up();
await sleep(300);
const brushed = await page.$eval('#txwin', (w) => ({ chip: !!w.querySelector('.tw-chip'), n: +w.querySelector('.tw-count').textContent,
  sub: w.querySelector('.fw-sub').textContent }));
step(!brushed.chip && brushed.n > 0 && brushed.n < total0 && /filtered/.test(brushed.sub), 'brushing the chart narrows the window', JSON.stringify(brushed));
await clickText('#filterbar button', 'Reset').catch(() => {});
await page.click('#txwin .fw-min'); await sleep(700);

// Any section pops out into a window; it docks next to Transactions; ✕ puts it back.
await page.$eval('h2[data-yank="where-it-went"] .yank-btn', (b) => b.click());
await page.waitForSelector('#yank-where-it-went #spend-bars');
const ph = await page.$eval('h2[data-yank="where-it-went"]', (h) => h.nextElementSibling.className);
await page.$eval('#yank-where-it-went .cathead', (r) => r.click());
const stillPicks = await page.$eval('#txwin .tw-chip', (c) => c.textContent).catch(() => '');
step(ph === 'yank-ph' && /✕/.test(stillPicks), 'section pops out into a window and stays live', `placeholder=${ph} pick="${stillPicks}"`);
await page.click('#txwin .fw-min'); await sleep(600);
await page.click('#yank-where-it-went .fw-min'); await sleep(600);
const dockOrder = await page.$$eval('#fw-dockbar .fw-dock', (d) => d.map((x) => { const r = x.getBoundingClientRect(); return [x.id, Math.round(r.top), Math.round(r.right)]; }));
step(dockOrder.length === 2 && dockOrder[0][0] === 'txwin-dock' && dockOrder[1][1] < dockOrder[0][1] && dockOrder[1][2] === dockOrder[0][2],
  'popped section docks above Transactions, right-aligned', JSON.stringify(dockOrder));
await page.click('#yank-where-it-went-dock'); await sleep(600);
await page.click('#yank-where-it-went .fw-close'); await sleep(600);
const putBackOk = await page.$eval('h2[data-yank="where-it-went"]', (h) => h.nextElementSibling.querySelector('#spend-bars') != null);
step(putBackOk && !(await page.$('#yank-where-it-went')) && !(await page.$('#yank-where-it-went-dock')), 'close puts the section back on the page', `back=${putBackOk}`);
// The filter bar is a section too: pop it out, filter from the window, put it back.
await page.$eval('#filters-sec .yank-btn', (b) => b.click());
await page.waitForSelector('#yank-filters #filterbar');
await page.$eval('#yank-filters .facet > button', (b) => b.click());
const menu = await page.$eval('#yank-filters .facet.open .facet-menu', (m) => {
  const r = m.getBoundingClientRect(), w = m.closest('.fw').getBoundingClientRect();
  const hit = document.elementFromPoint(r.left + 10, r.bottom - 6);
  return { spills: r.bottom > w.bottom, visible: !!hit && m.contains(hit) };
});
await page.$eval('body', (b) => b.click());
await page.$eval('#txwin .tw-x', (b) => b.click()).catch(() => {});
const before = await page.$eval('#txwin .tw-count', (e) => +e.textContent);
await page.type('#yank-filters #search', 'CAFE');
await page.waitForFunction((n) => +document.querySelector('#txwin .tw-count').textContent < n, { timeout: 5000 }, before);
const after = await page.$eval('#txwin .tw-count', (e) => +e.textContent);
const pill = await page.$eval('#yank-filters #activebar', (a) => a.textContent.replace(/\s+/g, ' ').trim());
step(menu.visible && after > 0 && after < before && /CAFE/.test(pill) && (await page.$eval('#filters-sec', (e) => !!e.closest('.fw'))),
  'filters pop out and keep filtering from the window', JSON.stringify({ menu, before, after, pill }));
await page.$eval('#yank-filters .fw-close', (b) => b.click()); await sleep(600);
await clickText('#filterbar button', 'Reset').catch(() => {});
await sleep(300);
const sticky = await page.$eval('#filters-sec', (e) => ({ inPage: !e.closest('.fw'), pos: getComputedStyle(e).position }));
step(sticky.inPage && sticky.pos === 'sticky' && (await page.$eval('#txwin .tw-count', (e) => +e.textContent)) === before,
  'filters go back to the page (still sticky, filter reset)', JSON.stringify(sticky));

// Tear-off: drag a heading off the page; the chart re-fits the window width.
await page.$eval('h2[data-yank="cash-flow-over-time"]', (h) => h.scrollIntoView({ block: 'center' }));
const hb = await page.$eval('h2[data-yank="cash-flow-over-time"]', (h) => { const r = h.getBoundingClientRect(); return { x: r.left + 30, y: r.top + 10 }; });
await page.mouse.move(hb.x, hb.y); await page.mouse.down();
await page.mouse.move(hb.x + 80, hb.y + 60, { steps: 5 }); await page.mouse.move(300, 200, { steps: 8 }); await page.mouse.up();
await sleep(600);
const torn = await page.$eval('#yank-cash-flow-over-time', (w) => ({ body: w.querySelector('.fw-body').clientWidth,
  canvas: w.querySelector('#cashflow-chart canvas')?.getBoundingClientRect().width || 0, left: Math.round(w.getBoundingClientRect().left) }))
  .catch((e) => ({ err: e.message }));
step(torn.canvas > 0 && torn.canvas <= torn.body && torn.body - torn.canvas < 60, 'dragging a heading tears it off; the chart fits the window', JSON.stringify(torn));
await shot('01d-yanked');
await page.$eval('.yank-ph .btn:last-child', (b) => b.click()); await sleep(600);
step(!(await page.$('#yank-cash-flow-over-time')) && !!(await page.$('#cashflow-chart canvas')), '“Put back here” returns it', 'ok');

// 2. Change-point view: three charts with levels and change marks.
await page.click('#view-buttons [data-view="cp"]');
await page.waitForSelector('#cp-charts .cp-section');
await sleep(300);
const heads = await page.$$eval('#cp-charts .cp-head', (hs) => hs.map((h) => h.firstChild?.textContent + ' :: ' + (h.querySelector('summary')?.textContent || '')));
const canvases = await page.$$eval('#cp-charts canvas', (c) => c.length);
step(heads.length === 3 && canvases === 3, 'change-point view has Net/In/Out charts', JSON.stringify(heads));
const rawHidden = !(await visible('#cashflow-chart'));
step(rawHidden, 'raw chart hidden in CP view', `rawHidden=${rawHidden}`);
await page.$eval('#cp-charts', (e) => e.scrollIntoView());
await shot('02-cp-auto');
await page.click('#gran-buttons [data-gran="month"]');
await sleep(300);
await page.$eval('#cp-charts', (e) => e.scrollIntoView());
await shot('03-cp-month');
const netLevelsStd = await page.$$eval('#cp-charts .cp-section:first-child tbody tr', (r) => r.length);
await page.click('#cp-controls [data-sens="sensitive"]');
await sleep(300);
const netLevelsSens = await page.$$eval('#cp-charts .cp-section:first-child tbody tr', (r) => r.length);
step(netLevelsSens >= netLevelsStd && netLevelsStd >= 2, 'sensitive finds ≥ levels than standard (net)', `standard=${netLevelsStd} sensitive=${netLevelsSens}`);
await page.click('#cp-controls [data-sens="standard"]');
await sleep(300);
const cpOv = await page.$eval('#cp-charts .cp-section:nth-child(3) .u-over', (o) => { o.scrollIntoView({ block: 'center' }); const r = o.getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width, h: r.height }; });
await page.mouse.click(cpOv.x + cpOv.w * 0.6, cpOv.y + cpOv.h * 0.5);
await page.waitForFunction(() => /^Money out · /.test(document.querySelector('#txwin .tw-chip')?.textContent || ''), { timeout: 5000 });
const cpPick = await page.$eval('#txwin', (w) => ({ chip: w.querySelector('.tw-chip').textContent, kinds: [...new Set([...w.querySelectorAll('tbody .tag')].map((t) => t.textContent))] }));
step(cpPick.kinds.length > 0 && !cpPick.kinds.includes('income'), 'clicking a change-point dot (Out) lists that bucket\'s money out', JSON.stringify(cpPick));
await page.click('#txwin .fw-min'); await sleep(700);

// Settings survive a reload (view + sensitivity).
await page.reload();
await page.waitForFunction(() => !document.getElementById('report-root').hidden, { timeout: 30000 });
const cpAfterReload = await visible('#cp-charts');
step(cpAfterReload, 'view choice persists across reload', `cpVisible=${cpAfterReload}`);

// 3. Configuration: demo has Reset (no Delete); create, fill and open a project.
await page.click('#config-open');
await page.waitForSelector('.cfg-proj');
await shot('04-config-demo', true);
const demoBtns = await page.$$eval('.cfg-actions button', (b) => b.map((x) => x.textContent));
step(demoBtns.some((t) => t.includes('Reset')) && !demoBtns.some((t) => t.includes('Delete')), 'demo offers Reset, not Delete', JSON.stringify(demoBtns));

await page.type('.cfg-new input[type=text]', 'E2E household');
await clickText('.cfg-new button', 'New project');
await page.waitForFunction(() => document.querySelector('.cfg-name')?.value === 'E2E household');
const up = await page.$('input[type=file][multiple]');
await up.uploadFile(SAMPLE);
await page.waitForFunction(() => /Statements \(1\)/.test(document.body.textContent), { timeout: 10000 });
step(true, 'uploaded a CSV into a new project', 'Statements (1)');

// Config bundle upload (just the .toml files of the playground, zipped).
const cfgDir = path.resolve(here, '../../playground/config');
const bundle = path.join(OUT, 'config-bundle.zip');
fs.writeFileSync(bundle, zipSync(Object.fromEntries(
  fs.readdirSync(cfgDir).filter((n) => n.endsWith('.toml')).map((n) => [`config/${n}`, fs.readFileSync(path.join(cfgDir, n))]))));
const zipInputs = await page.$$('.cfg-h3-btns input[type=file]');
await zipInputs[1].uploadFile(bundle);
await page.waitForFunction(() => /Loaded .*categories\.toml/.test(document.body.textContent), { timeout: 10000 });
const cfgRows = await page.$$eval('.cfg-files', (t) => t[1].querySelectorAll('td.num').length
  - [...t[1].querySelectorAll('td.num')].filter((td) => td.textContent === '—').length);
step(cfgRows >= 3, 'config bundle (.zip) loads its .toml files', `custom config files=${cfgRows}`);

// Unknown layout -> flagged -> mapped by hand -> read correctly.
const odd = path.join(OUT, 'odd_bank_4321.csv');
fs.writeFileSync(odd, 'Posted;Details;Withdrawal;Deposit;Balance\n'
  + '03.11.2026;"COFFEE; DOWNTOWN";4,50;;100\n03.11.2026;PAYROLL ACME;;2000.00;2100\n'.replace('4,50', '4.50')
  + '15.11.2026;RENT;1500.00;;600\n');
await (await page.$('input[type=file][multiple]')).uploadFile(odd);
await page.waitForFunction(() => /Not recognised/.test(document.body.textContent), { timeout: 10000 });
step(true, 'unknown layout is flagged', 'Not recognised');
await clickText('.cfg-files button', 'Map columns');
await page.waitForSelector('.map-box');
const guessed = await page.$$eval('.map-table thead select', (s) => s.map((x) => x.value));
step(JSON.stringify(guessed) === JSON.stringify(['date', 'merchant', 'out', 'in', '']), 'mapper guesses column labels', JSON.stringify(guessed));
await page.type('.map-bank', 'Odd Bank');
await page.waitForFunction(() => /odd_bank_debit/.test(document.querySelector('.map-settings')?.textContent || ''), { timeout: 10000 });
await page.waitForFunction(() => /Reads 3 rows/.test(document.querySelector('.map-result')?.textContent || ''), { timeout: 10000 });
const parsed = await page.$$eval('.map-parsed tbody tr', (r) => r.map((x) => [...x.cells].map((c) => c.textContent).join('|')));
step(parsed.some((r) => r.startsWith('2026-11-03|-4.5') && r.endsWith('|spend')) && parsed.some((r) => /PAYROLL.*\|income$/.test(r)),
  'mapper preview parses with the real engine', JSON.stringify(parsed));
await shot('05a-mapper', true);
await clickText('.map-actions button', 'Save format');
await page.waitForFunction(() => /✓ odd_bank_debit · 3 rows/.test(document.body.textContent), { timeout: 10000 });
step(true, 'mapped file now recognised', 'odd_bank_debit · 3 rows');
// The mapping is stored in the project's profiles.toml (part of the config bundle).
const stored = await page.evaluate(() => new Promise((res, rej) => {
  const rq = indexedDB.open('wdmg-projects');
  rq.onerror = () => rej(rq.error);
  rq.onsuccess = () => {
    const all = rq.result.transaction('projects').objectStore('projects').getAll();
    all.onsuccess = () => {
      const doc = all.result.find((d) => d.name === 'E2E household');
      const f = doc?.files.find((x) => x.name === 'profiles.toml');
      res(f ? new TextDecoder().decode(f.bytes) : '');
    };
  };
}));
step(/id = "odd_bank_debit"/.test(stored) && /\[profile\.type\.by_sign\]/.test(stored),
  'mapped format saved in profiles.toml (config bundle)', `${stored.length} bytes, has odd_bank_debit=${/odd_bank_debit/.test(stored)}`);
// A savings export with the description split over 3 columns (and a time on each
// date) is a built-in format: recognised with no mapping.
const sav = path.join(OUT, 'savings_export.csv');
const savRows = '100000001,2026-03-31 12:00:00 AM,Savings - 0,"Interest Credit","Interest Amount 0.07",,0.07,140.07\n'
  + '100000002,2026-11-03 12:00:00 AM,Savings - 0,"Transfer Out","To chequing",ref 9,-5.00,135.07\n';
fs.writeFileSync(sav, 'ID, Date, Account Name, Description1, Description2, Description3, Amount, Balance\n' + savRows);
await (await page.$('input[type=file][multiple]')).uploadFile(sav);
await page.waitForFunction(() => /✓ split_description_ledger · 2 rows/.test(document.body.textContent), { timeout: 10000 });
step(true, 'split-description savings export is recognised built-in', 'split_description_ledger · 2 rows');
// The mapper handles the same shape for an unknown layout: dates with a time part,
// several description columns joined.
const odd2 = path.join(OUT, 'ref_export.csv');
fs.writeFileSync(odd2, 'Ref, Date, Account Name, Description1, Description2, Description3, Amount, Balance\n' + savRows);
await (await page.$('input[type=file][multiple]')).uploadFile(odd2);
await page.waitForFunction(() => [...document.querySelectorAll('.cfg-files tr')].some((r) => /ref_export/.test(r.textContent) && /Not recognised/.test(r.textContent)), { timeout: 10000 });
await page.$$eval('.cfg-files tr', (rs) => rs.find((r) => /ref_export/.test(r.textContent)).querySelector('button').click());
await page.waitForSelector('.map-box');
const g2 = await page.$$eval('.map-table thead select', (s) => s.map((x) => x.value));
step(JSON.stringify(g2) === JSON.stringify(['', 'date', '', 'merchant', 'merchant', 'merchant', 'amount', '']), 'mapper guesses several description columns', JSON.stringify(g2));
const dfmt = await page.$eval('.map-settings', (e) => e.textContent);
step(/YYYY-MM-DD/.test(dfmt) && /time part is ignored/.test(dfmt), 'date with a time part is recognised', dfmt.slice(0, 90));
await page.type('.map-bank', 'Some Bank');
await page.waitForFunction(() => /Reads 2 rows/.test(document.querySelector('.map-result')?.textContent || ''), { timeout: 10000 });
const p2 = await page.$$eval('.map-parsed tbody tr', (r) => r.map((x) => [...x.cells].map((c) => c.textContent).join('|')));
step(p2.includes('2026-03-31|0.07|INTEREST CREDIT INTEREST AMOUNT 0.07|income') && p2.includes('2026-11-03|-5.00|TRANSFER OUT TO CHEQUING REF 9|spend'),
  'description columns are joined, dates parsed', JSON.stringify(p2));
await clickText('.map-actions button', 'Save format');
await page.waitForFunction(() => /✓ some_bank_debit · 2 rows/.test(document.body.textContent), { timeout: 10000 });
step(true, 'multi-description format saved and recognised', 'some_bank_debit · 2 rows');
// People & accounts: files on no account get one; renames and owner moves are
// saved into people.toml with its comments intact.
const orphanWarn = await page.$eval('.cfg-right', (r) => /belong to no account/.test(r.textContent));
await clickText('.cfg-h3-btns button', 'People & accounts');
await page.waitForSelector('.pe-box');
const orphanFiles = await page.$$eval('.pe-orphan', (o) => o.map((x) => x.dataset.file));
await page.$eval('.pe-orphan[data-file="odd_bank_4321.csv"]', (row) => {
  const name = row.querySelector('.pe-newbits input'); name.value = 'Kid savings';
  const own = row.querySelectorAll('.pe-newbits select')[0]; own.value = 'leela';
  row.querySelector('button').click();
});
// The guide proposes a rule from the ticked file and previews it with the engine.
await page.waitForSelector('.mg-box');
const kidGuide = await page.evaluate(() => ({
  verdict: document.querySelector('.mg-verdict').textContent,
  ticked: [...document.querySelectorAll('.mg-files tr.on')].map((r) => r.dataset.file),
  crit: document.querySelector('.mg-crit').textContent,
}));
// The mapped format reads "4321" from the file name, so the number is the durable choice.
step(/Picks up exactly the 1 ticked file/.test(kidGuide.verdict) && kidGuide.ticked.join() === 'odd_bank_4321.csv' && /carries the number 4321/.test(kidGuide.crit),
  'picks-up guide: proposes a rule for the ticked file and previews it', JSON.stringify(kidGuide));
await shot('people-guide');
await clickText('.mg-box .map-actions button', 'Apply');
await page.waitForFunction(() => document.querySelector('tr[data-account="Kid savings"]') && !document.querySelector('.mg-box'));
const kidRows = await page.$eval('tr[data-account="Kid savings"] td.num', (td) => td.textContent);
const kidHow = await page.$eval('tr[data-account="Kid savings"] .pe-how', (td) => td.textContent);
// An external account can be given statements: savings_export.csv → TFSA.
const extOrphan = await page.$$eval('.pe-orphan', (o) => o.map((x) => x.dataset.file).find((f) => /savings_export/.test(f)));
await page.$eval(`.pe-orphan[data-file="${extOrphan}"]`, (row) => {
  const t = row.querySelector('select'); t.value = 'TFSA'; t.dispatchEvent(new Event('change'));
  [...row.querySelectorAll('button')].find((b) => /Assign/.test(b.textContent)).click();
});
await page.waitForSelector('.mg-box');
await clickText('.mg-box .map-actions button', 'Apply');
await page.waitForFunction(() => !document.querySelector('.mg-box'));
const tfsa = await page.$eval('tr[data-account="TFSA"]', (tr) => ({ rows: tr.querySelector('td.num').textContent, how: tr.querySelector('.pe-how').textContent }));
step(/^[1-9]/.test(tfsa.rows) && !/external/.test(tfsa.how), 'an external account given a statement becomes a regular one', JSON.stringify(tfsa));
// Kind is editable; a narrowed rule that misses its file shows in the health check.
await page.$eval('tr[data-account="Kid savings"] td:nth-child(2) select', (s) => { s.value = 'debit'; s.dispatchEvent(new Event('change')); });
await page.waitForFunction(() => document.querySelector('tr[data-account="Kid savings"] td:nth-child(2) select').value === 'debit');
await page.$eval('tr[data-account="Kid savings"] .pe-how-btn', (b) => b.click());
await page.waitForSelector('.mg-box');
await page.$$eval('.mg-row', (rows) => {
  const r = rows.find((x) => /File name/.test(x.textContent));
  const cb = r.querySelector('input[type=checkbox]'); cb.checked = true; cb.dispatchEvent(new Event('change'));
});
await page.$eval('.mg-name', (i) => { i.value = 'nope_'; i.dispatchEvent(new Event('input')); });
await page.waitForFunction(() => /not picked up/.test(document.querySelector('.mg-verdict').textContent), { timeout: 5000 });
await clickText('.mg-box .map-actions button', 'Apply');
await page.waitForFunction(() => !document.querySelector('.mg-box'));
const health = await page.$$eval('.pe-hi', (hs) => hs.map((x) => x.className + ':' + x.textContent));
step(health.some((x) => /info/.test(x) && /pick up none of this project’s statements/.test(x) && /Kid savings/.test(x)) && health.some((x) => /bad/.test(x) && /no account/.test(x)),
  'health check flags an account that picks up nothing and files on no account', JSON.stringify(health));
await page.$eval('.pe-health', (e) => e.scrollIntoView()); await shot('people-health');
await clickText('.pe-box .map-actions button', '↶ Undo');
await page.waitForFunction(() => ![...document.querySelectorAll('.pe-hi.info button')].some((b) => b.textContent === 'Kid savings'));
await page.$eval('[data-person="fry"] .pe-person-name', (i) => { i.value = 'Philip'; i.dispatchEvent(new Event('change')); });
await page.waitForFunction(() => document.querySelector('[data-person="Philip"]'));
await page.$eval('tr[data-account="Joint Savings"] td:nth-child(5) select', (s) => { s.value = 'leela'; s.dispatchEvent(new Event('change')); });
await page.waitForFunction(() => document.querySelector('[data-person="leela"] tr[data-account="Joint Savings"]'));
step(orphanWarn && orphanFiles.includes('odd_bank_4321.csv') && kidRows === '3' && /number 4321 in the file name/.test(kidHow),
  'people editor: assign a file to a new account, rename a person, move an account', JSON.stringify({ orphanFiles, kidRows, kidHow }));
// Delete: a person with accounts can't go until they're gone; then they can.
const delBlocked = await page.$eval('[data-person="bender"] .pe-person-head .pe-del', (b) => b.disabled);
for (const a of ['Bender Mastercard', 'Bender Ledger']) {
  await page.$eval(`tr[data-account="${a}"] .pe-del`, (b) => b.click());
  await page.waitForFunction((a) => !document.querySelector(`tr[data-account="${a}"]`), {}, a);
}
await page.$eval('[data-person="bender"] .pe-person-head .pe-del', (b) => b.click());
await page.waitForFunction(() => !document.querySelector('[data-person="bender"]'));
step(delBlocked, 'delete accounts, then their emptied person', 'bender gone');
await clickText('.map-actions button', 'Save');
await page.waitForFunction(() => !document.querySelector('.pe-box'));
const ppl = await page.evaluate(() => new Promise((res) => {
  const rq = indexedDB.open('wdmg-projects');
  rq.onsuccess = () => {
    const all = rq.result.transaction('projects').objectStore('projects').getAll();
    all.onsuccess = () => {
      const f = all.result.find((d) => d.name === 'E2E household')?.files.find((x) => x.name === 'people.toml');
      res(f ? new TextDecoder().decode(f.bytes) : '');
    };
  };
}));
step(/PLAYGROUND household/.test(ppl) && /id = "Kid savings"/.test(ppl) && /id = "Philip"/.test(ppl) && /card_last4 = "4321"/.test(ppl) && !/id = "TFSA"\s*\n\s*external/.test(ppl) && !/bender/i.test(ppl.replace(/^\s*#.*$/gm, '')),
  'people.toml keeps its comments and holds the edits (part of the config bundle)', ppl.split('\n').filter((l) => /Kid savings|Philip|odd_bank|4321|TFSA/.test(l)).join(' / '));
await clickText('.cfg-actions button', 'Open in report');
await page.waitForFunction(() => !document.getElementById('report-root').hidden && /E2E/.test(document.getElementById('proj-name').textContent), { timeout: 30000 });
const txRows = await page.$$eval('#txwin .fw-body tbody tr', (r) => r.length);
step(txRows > 0, 'new project analyzed in-browser', `txn rows shown=${txRows}`);
await shot('05-new-project');
const accts = await page.$$eval('#by-account-panel tbody td:first-child', (t) => t.map((x) => x.textContent));
const persons = await page.$$eval('#by-person-panel tbody td:first-child', (t) => t.map((x) => x.textContent));
step(accts.includes('Kid savings') && persons.includes('Philip') && !persons.includes('fry'), 'report uses the new names', JSON.stringify({ accts, persons }));

// 4. Delete it; app falls back to the demo.
await page.click('#config-open');
await page.waitForSelector('.cfg-proj');
await clickText('.cfg-proj', 'E2E household');
await clickText('.cfg-actions button', 'Delete project');
await page.waitForFunction(() => !/E2E household/.test(document.querySelector('.cfg-projects').textContent));
await page.click('#config-close');
await page.waitForFunction(() => !document.getElementById('report-root').hidden, { timeout: 30000 });
const back = await page.$eval('#proj-name', (e) => e.textContent);
step(/Demo/.test(back), 'deleting the active project falls back to the demo', back);

// Explore view → ✎ Rename: the account is renamed in the demo's people.toml and the report re-analyzes.
await page.$eval('#transfers-panel', (e) => { e.classList.remove('sec-closed'); window.dispatchEvent(new Event('resize')); });
await sleep(300);
await page.$$eval('#flow-graph .node-lbl', (ts) => ts.find((x) => x.textContent === 'Bender Ledger').dispatchEvent(new MouseEvent('click', { bubbles: true })));
await page.waitForSelector('.fc-edit');
await page.$eval('.fc-edit', (b) => b.click());
await page.waitForSelector('.pe-box tr.pe-focus[data-account="Bender Ledger"]');
await page.$eval('tr[data-account="Bender Ledger"] input', (i) => { i.value = 'Bender Main'; i.dispatchEvent(new Event('change')); });
await page.waitForFunction(() => document.querySelector('tr[data-account="Bender Main"]'));
await Promise.all([page.waitForNavigation({ waitUntil: 'load' }), clickText('.map-actions button', 'Save')]);
await page.waitForFunction(() => !document.getElementById('report-root').hidden, { timeout: 30000 });
const renamed = await page.$$eval('#by-account-panel tbody td:first-child', (t) => t.map((x) => x.textContent));
step(renamed.includes('Bender Main') && !renamed.includes('Bender Ledger'), 'rename from the explore view re-analyzes the report', JSON.stringify(renamed));

} catch (e) {
  failed++;
  console.log('FAIL | aborted |', e.message.split('\n')[0]);
  await shot('zz-failure', true).catch(() => {});
  console.log('page text:', (await page.evaluate(() => document.body.innerText.slice(0, 600)).catch(() => '?')).replace(/\s+/g, ' '));
}
step(errors.length === 0, 'no page errors', errors.slice(0, 5).join(' | ') || 'none');
await browser.close();
console.log(failed ? `${failed} FAILED` : 'ALL OK', `screenshots in ${OUT}`);
process.exit(failed ? 1 : 0);
