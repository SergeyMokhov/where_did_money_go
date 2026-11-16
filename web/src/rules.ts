// "Categories & rules": sort transactions into categories and say where
// transfers went — without knowing the config files. Three tabs:
//  - To sort: everything no rule categorizes, grouped by merchant, biggest first;
//  - Categories: every category › subcategory with the texts that put
//    transactions there (built-in ones too), each with how many it catches;
//  - Transfer rules: where unpaired transfers go, in the order they're tried,
//    plus the transfers nothing explains yet.
// Rules are made from an example ("Sort…"): pick the text that identifies it
// and where it belongs; the real engine re-runs on the project to preview
// exactly which transactions move, from where to where. Edits go to the
// project's categories.toml / people.toml (comments kept) on Save.
import {
  editCategories, editPeople, peopleText, projectConfig, ruleLab,
  type CategoryKind, type CategoryOp, type Lab, type LabCategory, type LabRow, type LabSub, type PeopleOp,
} from './engine';
import { reEscape } from './matchguide';
import { upsertFile, type ProjectDoc } from './store';

// ---------- pure helpers (unit-tested) ----------

const META = /[.+*?()|[\]{}^$]/;
const unescape = (s: string) => s.replace(/\\(.)/g, '$1');
/** The plain text a pattern looks for, or null when it's a real regex. */
export function literalOf(pattern: string): string | null {
  const body = pattern.replace(/^\(\?i\)/, '');
  return META.test(body.replace(/\\./g, '')) ? null : unescape(body);
}
/** "contains “CAFE”" / "matches /CAF(E|É)/". */
export const describePattern = (p: string) => {
  const l = literalOf(p);
  return l != null ? `“${l}”` : `/${p}/`;
};
/** Category patterns are case-insensitive by the engine; transfer rules aren't, so they get (?i). */
export const catPattern = (text: string) => reEscape(text.trim());
export const rulePattern = (text: string) => `(?i)${reEscape(text.trim())}`;
/** User input: "/regex/" means a pattern, anything else is plain text. */
export function patternFromInput(input: string, forRule: boolean): string {
  const t = input.trim();
  const m = t.match(/^\/(.+)\/$/);
  if (m) return forRule && !/^\(\?i\)/.test(m[1]) ? `(?i)${m[1]}` : m[1];
  return forRule ? rulePattern(t) : catPattern(t);
}
/** JS approximation of the engine's match (case-insensitive, unanchored). */
export function jsMatcher(pattern: string): (r: { merchant: string; raw: string }) => boolean {
  try {
    const re = new RegExp(pattern.replace(/\(\?i\)/g, ''), 'i');
    return (r) => re.test(r.merchant) || re.test(r.raw);
  } catch { return () => false; }
}
const words = (s: string) => s.replace(/\s+/g, ' ').trim().split(' ').filter(Boolean);
/**
 * How many leading words of `merchant` are boilerplate of its account: some
 * banks start every description with the account's name ("SPACE SAVER
 * ACCOUNT - 1 INTEREST CREDIT"). A prefix counts when EVERY row of the account
 * starts with it, it's at least two words, and the rows go on differently
 * (≥2 different next words) — so an account that is mostly one merchant keeps
 * that merchant's name.
 */
export function boilerplateWords(merchant: string, accountMerchants: string[]): number {
  if (accountMerchants.length < 5) return 0;
  const mine = words(merchant.toUpperCase());
  const rows = accountMerchants.map((m) => words(m.toUpperCase()));
  let best = 0;
  for (let n = 1; n < mine.length; n++) { // always keep at least one word
    const same = rows.filter((r) => r.length > n && mine.slice(0, n).every((w, i) => r[i] === w));
    if (same.length < rows.length) break;
    if (n >= 2 && new Set(same.map((r) => r[n])).size >= 2) best = n;
  }
  return best;
}
/** `merchant` without its first `skip` words. */
export const dropWords = (merchant: string, skip: number) => words(merchant).slice(skip).join(' ');
/** Texts worth suggesting for a description: whole name, first words, without trailing ids/numbers. */
export function suggestTexts(merchant: string, skip = 0): string[] {
  const full = merchant.replace(/\s+/g, ' ').trim();
  const m = skip ? dropWords(full, skip) : full;
  // "E-TRANSFER TO: JANE DOE" — the part after the colon names the other side.
  const after = m.includes(':') ? m.slice(m.lastIndexOf(':') + 1).trim() : '';
  if (after.length >= 3) return [...new Set([after, m])];
  const clean = m.replace(/(\s+(#?\d[\w.,-]*|[A-Z]{2}|#))+$/i, '').trim(); // store numbers, province codes, "#"
  const ws = clean.split(' ');
  const out = new Set<string>();
  if (ws.length > 2) out.add(ws.slice(0, 2).join(' '));
  if (ws.length > 1 && ws[0].length >= 4) out.add(ws[0]);
  out.add(clean);
  out.add(m);
  if (skip) out.add(full);
  return [...out].filter((s) => s.length >= 3);
}
export const isToSort = (r: LabRow) => r.kind !== 'transfer' && !r.hit;
export function rowLabel(r: LabRow): string {
  if (r.kind === 'transfer') {
    const t = r.transfer;
    return t?.other ? `transfer ↔ ${t.other}` : 'transfer, other side unknown';
  }
  if (!r.hit) return 'not sorted';
  return `${r.category} › ${r.subcategory}`;
}
/** Rows whose label differs between two labs of the same files, grouped "from → to". */
export function diffLabs(a: Lab, b: Lab): Map<string, LabRow[]> {
  const out = new Map<string, LabRow[]>();
  if (a.rows.length !== b.rows.length) return out;
  b.rows.forEach((r, i) => {
    const from = rowLabel(a.rows[i]), to = rowLabel(r);
    if (from !== to) { const k = `${from} → ${to}`; out.set(k, [...(out.get(k) ?? []), r]); }
  });
  return out;
}
/** Rule file indices in the order the engine tries them (priority desc, then file order). */
export const triedOrder = (rules: Lab['rules']) => rules.map((r, i) => ({ r, i })).sort((x, y) => y.r.priority - x.r.priority || x.i - y.i).map((x) => x.i);

// ---------- DOM helpers ----------
function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) { if (k === 'class') e.className = v; else e.setAttribute(k, v); }
  e.append(...kids);
  return e;
}
function btn(text: string, onClick: () => void, cls = 'btn small', title = ''): HTMLButtonElement {
  const b = h('button', { type: 'button', class: cls, ...(title ? { title } : {}) }, text) as HTMLButtonElement;
  b.addEventListener('click', onClick);
  return b;
}
function input(value: string, attrs: Record<string, string> = {}): HTMLInputElement {
  const i = h('input', { type: 'text', class: 'pe-in', ...attrs }) as HTMLInputElement;
  i.value = value;
  return i;
}
function sel(opts: { v: string; t: string; group?: string }[], val: string, cls = 'map-sel'): HTMLSelectElement {
  const s = h('select', { class: cls }) as HTMLSelectElement;
  const groups = new Map<string, HTMLElement>();
  for (const o of opts) {
    const op = h('option', { value: o.v }, o.t);
    if (o.v === val) (op as HTMLOptionElement).selected = true;
    if (o.group) {
      if (!groups.has(o.group)) { const g = h('optgroup', { label: o.group }); groups.set(o.group, g); s.append(g); }
      groups.get(o.group)!.append(op);
    } else s.append(op);
  }
  return s;
}
const money = (n: number) => (n < 0 ? '−' : '') + Math.abs(n).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const sum = (rows: LabRow[]) => rows.reduce((a, r) => a + Number(r.amount), 0);
const plural = (n: number, w: string) => `${n.toLocaleString()} ${n === 1 ? w : w.endsWith('y') ? `${w.slice(0, -1)}ies` : `${w}s`}`;
const KIND_LABEL: Record<CategoryKind, string> = { spend: 'Spending', income: 'Income', transfer: 'Moving money (not spending)' };
const SOURCE_LABEL: Record<string, string> = { builtin: 'built-in', changed: 'built-in, edited', yours: 'yours' };

function modal(cls: string, label: string, onEsc: () => void) {
  const overlay = h('div', { class: `map-overlay ${cls}`, role: 'dialog', 'aria-modal': 'true', 'aria-label': label });
  const box = h('div', { class: 'map-box panel' });
  overlay.append(box);
  document.body.append(overlay);
  const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.stopPropagation(); onEsc(); } };
  document.addEventListener('keydown', onKey, true);
  return { box, close: () => { overlay.remove(); document.removeEventListener('keydown', onKey, true); } };
}

/** A sample table of rows (date, description, account, amount). */
function sample(rows: LabRow[], max = 6): HTMLElement {
  const tb = h('tbody', {});
  for (const r of rows.slice(0, max)) {
    tb.append(h('tr', {}, h('td', { class: 'muted' }, r.date), h('td', {}, r.merchant), h('td', { class: 'muted' }, r.account),
      h('td', { class: 'num ' + (Number(r.amount) < 0 ? 'neg' : 'pos') }, money(Number(r.amount)))));
  }
  const t = h('table', { class: 'map-table ru-sample' }, tb);
  return rows.length > max ? h('div', {}, t, h('div', { class: 'muted ru-more' }, `… and ${rows.length - max} more`)) : t;
}

/** Rows grouped by description ("24 × INTEREST CREDIT …"); ⚠ marks descriptions not in `known`. */
function grouped(rows: LabRow[], known?: Set<string>): HTMLElement {
  const g = new Map<string, LabRow[]>();
  for (const r of rows) g.set(r.merchant, [...(g.get(r.merchant) ?? []), r]);
  if (g.size === 1) return sample(rows);
  const list = [...g.entries()].sort((a, b) => b[1].length - a[1].length);
  const tb = h('tbody', {});
  for (const [m, rs] of list.slice(0, 12)) {
    const odd = !!known && !known.has(m);
    tb.append(h('tr', odd ? { class: 'ru-odd', title: 'A different description from your example — check it belongs here' } : {},
      h('td', { class: 'num' }, `${rs.length} ×`), h('td', {}, odd ? '⚠ ' : '', m),
      h('td', { class: 'muted' }, [...new Set(rs.map((r) => r.account))].join(', ')),
      h('td', { class: 'num ' + (sum(rs) < 0 ? 'neg' : 'pos') }, money(sum(rs)))));
  }
  const t = h('table', { class: 'map-table ru-sample' }, tb);
  return list.length > 12 ? h('div', {}, t, h('div', { class: 'muted ru-more' }, `… and ${list.length - 12} more descriptions`)) : t;
}

// ---------- help (static text) ----------
const HELP_MATCH = `
<h5>How a transaction gets its category</h5>
<ul>
<li>Every subcategory has one or more <b>texts</b>. A transaction goes to the subcategory whose text appears
<b>anywhere</b> in its description — the cleaned-up name you see, or the bank's original wording. Upper/lower case never matters.</li>
<li>Several fit? The <b>longest text wins</b>: “UBER EATS” beats “UBER”. Equally long: the higher <i>tie-break priority</i> wins, then the one listed first.</li>
<li><b>📌 Pinned</b> transactions were put somewhere one at a time (“Only this one”) and ignore all texts — for one-offs like a car or a gift.</li>
<li>Nothing fits: it waits under <b>To sort</b>, and the report shows it as uncategorized.</li>
<li><b>Click a text's chip</b> to edit it — the list shows every transaction it catches and where each would end up, before you apply.
Fixing an existing text (“FISHY JOES” → “FISHY JOE”) is usually better than adding a second one.</li>
</ul>
<h5>Counts as</h5>
<ul>
<li><b>Spending</b> and <b>Income</b> add up in the report's totals. <b>Moving money</b> (transfers) is left out of both — see the Transfer rules tab.</li>
<li>The bank's sign still wins: a refund that matches a spending text is money <i>in</i>, just labelled with that category.
A row the bank's file itself marks as a transfer (a card payment) stays a transfer.</li>
</ul>
<h5>Advanced: amount limits and priority</h5>
<ul>
<li><b>Amount ranges</b> make a text apply only when the amount — ignoring its sign — is in the range (both ends included).
A <b>text can have its own range</b>: “SHELL” up to 20 in Snacks, and “SHELL” from 35 in Gas — a $12 stop is snacks, a $60 one is gas,
and the $20–35 ones stay to sort. Set it in “Sort…” (tick the transactions it should take) or by clicking a text's chip.
A subcategory can also have a range for all its texts that don't have their own (under “amount limits”): built-in Coffee takes up to 20,
so a $300 “COFFEE” machine isn't coffee.</li>
<li><b>Tie-break priority</b> only matters when two texts of the <i>same length</i> from different subcategories both fit. Higher wins; normally 0.</li>
</ul>
<h5>Built-in and yours</h5>
<p>The app comes with built-in categories. Your changes are stored in this project's <code>categories.toml</code> on top of them:
editing a built-in subcategory copies it there first (“built-in, edited”); deleting a built-in switches it off.
Nothing changes until you press Save, and Undo steps back one change at a time.</p>
<h5>Patterns (for advanced users)</h5>
<p>Plain text is matched as typed. For a pattern, wrap it in slashes: <code>/…/</code>. Patterns use the
<a href="https://docs.rs/regex/latest/regex/#syntax" target="_blank" rel="noopener">Rust <code>regex</code> syntax</a> — like most
regex flavours (<code>. * + ? | ( ) [ ] { } ^ $ \d \s \b</code>), always case-insensitive, and matched anywhere unless you anchor it
with <code>^</code> (start) or <code>$</code> (end). Not supported: look-ahead/behind such as <code>(?!…)</code>, and back-references.</p>
<table class="map-table ru-help-ex"><tbody>
<tr><td><code>/TIM HORTONS?/</code></td><td>“TIM HORTON” or “TIM HORTONS”</td></tr>
<tr><td><code>/COSTCO (GAS|WHOLESALE)/</code></td><td>either one</td></tr>
<tr><td><code>/^AMZN/</code></td><td>descriptions that <i>start</i> with AMZN</td></tr>
<tr><td><code>/\bBAR\b/</code></td><td>the word BAR, but not BARBER</td></tr>
<tr><td><code>/PAYMENT.*THANK YOU/</code></td><td>PAYMENT, anything, then THANK YOU</td></tr>
</tbody></table>
<p class="muted">Chips show plain text in quotes (“CAFE”) and patterns between slashes (/CAF(E|É)/). Note a pattern's “length” for the
longest-wins rule is the length of what you wrote.</p>`;

const HELP_TRANSFERS = `
<h5>Rules: who's on the other side</h5>
<ul>
<li>A rule says: “if a transfer's description contains this text, the other side is that person or account”.
Only transfers whose other half <i>isn't</i> in your statements use them.</li>
<li>Rules are tried <b>from the top</b> and the <b>first</b> that fits wins (unlike categories, where the longest text wins) — use ↑ ↓ to order them.</li>
<li><b>Direction comes from the amount</b>: money out means you sent it to them, money in means you got it from them.
That's why one rule such as <code>e-Transfer (To|From): LEELA</code> covers both ways: <code>(To|From)</code> means “To” or “From”.</li>
<li>Rules never change totals or categories. They fill in <i>Money between accounts and people</i> and clear the “unknown” list.</li>
<li>The other side can be a person, one of your accounts, or an <i>external</i> account (somewhere you don't have statements for —
add those in People &amp; accounts).</li>
</ul>
<h5>Text and patterns</h5>
<p>Plain text matches anywhere, ignoring case. <code>/…/</code> is a pattern in
<a href="https://docs.rs/regex/latest/regex/#syntax" target="_blank" rel="noopener">Rust <code>regex</code> syntax</a>, e.g.
<code>/E-TRANSFER (TO|FROM): JANE/</code>. A pattern you write here is case-insensitive too.</p>`;

function helpBox(html: string): HTMLElement {
  const d = h('div', { class: 'ru-help' });
  d.innerHTML = html; // static text above, no user data
  return d;
}

// ---------- the editor ----------

export interface RuleExample { merchant: string; date?: string; amount?: string; account?: string }
type Tab = 'sort' | 'categories' | 'transfers';
interface Ctx {
  lab: Lab;
  /** Lab with extra ops applied to the current texts (throws with the engine's message). */
  tryOps(cat: CategoryOp[], ppl: PeopleOp[]): Lab;
}

/**
 * Open the editor. Resolves `true` when changes were saved into the project
 * (the caller persists `doc` and re-analyzes). With `example`, opens straight
 * into "Sort…" for that transaction, whose Apply saves at once.
 */
export function openRulesEditor(doc: ProjectDoc, opts: { tab?: Tab; example?: RuleExample } = {}): Promise<boolean> {
  return new Promise((resolve) => {
    const files = doc.files.filter((f) => f.role === 'data').map((f) => ({ name: f.name, bytes: f.bytes }));
    const cfg = projectConfig(doc);
    let cat = cfg['categories.toml'] ?? '';
    let ppl = peopleText(doc);
    const configFor = (c: string, p: string) => {
      const out: Record<string, string> = { ...cfg, 'people.toml': p };
      if (c.trim()) out['categories.toml'] = c; else delete out['categories.toml'];
      return out;
    };
    const labFor = (c: string, p: string) => ruleLab(files, configFor(c, p));
    const saved = labFor(cat, ppl);
    let lab = saved;
    const undo: { cat: string; ppl: string }[] = [];
    let error = '';
    let tab: Tab = opts.tab ?? 'sort';
    let query = '';
    let sortLimit = 40;
    let showHelp = false;
    /** `data-key`s of the <details> the user has open — kept across redraws. */
    const openKeys = new Set<string>();

    const { box, close: closeBox } = modal('ru-overlay', 'Categories and rules', () => finish(false));
    box.classList.add('ru-box');
    const finish = (ok: boolean) => { closeBox(); resolve(ok); };
    const save = () => {
      if (cat.trim()) upsertFile(doc, 'categories.toml', new TextEncoder().encode(cat));
      upsertFile(doc, 'people.toml', new TextEncoder().encode(ppl));
      finish(true);
    };

    const nextTexts = (catOps: CategoryOp[], pplOps: PeopleOp[]) => {
      const c = catOps.length ? editCategories(cat, configFor(cat, ppl), catOps) : cat;
      const p = pplOps.length ? editPeople(ppl, pplOps) : ppl;
      return { c, p };
    };
    const apply = (catOps: CategoryOp[], pplOps: PeopleOp[] = []) => {
      try {
        const { c, p } = nextTexts(catOps, pplOps);
        const l = labFor(c, p);
        undo.push({ cat, ppl });
        cat = c; ppl = p; lab = l; error = '';
      } catch (e) { error = (e as Error).message; }
      draw();
    };
    const ctx = (): Ctx => ({
      lab,
      tryOps: (catOps, pplOps) => { const { c, p } = nextTexts(catOps, pplOps); return labFor(c, p); },
    });
    const sortIt = (rows: LabRow[], mode: 'category' | 'transfer' = 'category', standalone = false, edit?: EditTarget) => {
      openRuleMaker(ctx(), rows, mode, standalone, edit).then((r) => {
        if (!r) return;
        apply(r.cat, r.ppl);
        if (standalone && !error) save();
      });
    };

    // ---- tabs ----
    function drawSort(): HTMLElement {
      const rows = lab.rows.filter(isToSort);
      if (!rows.length) return h('div', { class: 'ru-empty' }, '✓ Every transaction is in a category.');
      const groups = new Map<string, LabRow[]>();
      for (const r of rows) groups.set(r.merchant, [...(groups.get(r.merchant) ?? []), r]);
      const list = [...groups.entries()].sort((a, b) => Math.abs(sum(b[1])) - Math.abs(sum(a[1])));
      const tb = h('tbody', {});
      for (const [m, rs] of list.slice(0, sortLimit)) {
        const kinds = [...new Set(rs.map((r) => r.kind))].join(', ');
        const k = `sort:${m}`;
        const tr = h('tr', { class: 'ru-click', title: 'Click to see these transactions' });
        tr.addEventListener('click', (e) => { if ((e.target as HTMLElement).closest('button')) return; if (openKeys.has(k)) openKeys.delete(k); else openKeys.add(k); draw(); });
        tb.append(tr);
        if (openKeys.has(k)) {
          const det = h('tbody', {});
          for (const r of rs) det.append(h('tr', {}, h('td', { class: 'muted' }, r.date), h('td', {}, r.raw), h('td', { class: 'muted' }, `${r.account} · ${r.file}`),
            h('td', { class: 'num ' + (Number(r.amount) < 0 ? 'neg' : 'pos') }, money(Number(r.amount)))));
          tb.append(h('tr', { class: 'ru-detail' }, h('td', { colspan: '5' }, h('table', { class: 'map-table ru-sample' }, det))));
        }
        tr.append(
          h('td', {}, h('b', {}, m), rs[0].raw !== m ? h('div', { class: 'muted ru-raw' }, rs[0].raw) : ''),
          h('td', { class: 'num' }, String(rs.length)),
          h('td', { class: 'num ' + (sum(rs) < 0 ? 'neg' : 'pos') }, money(sum(rs))),
          h('td', { class: 'muted' }, kinds),
          h('td', {}, btn('Sort…', () => sortIt([...rs].sort((a, b) => Math.abs(Number(b.amount)) - Math.abs(Number(a.amount)))), 'btn small active ru-sort')));
      }
      return h('div', {},
        h('p', { class: 'muted' }, `${plural(rows.length, 'transaction')} from ${plural(groups.size, 'description')} aren’t in any category yet — biggest first. “Sort…” makes a rule from one; you’ll see everything it would catch before it’s applied.`),
        h('table', { class: 'map-table ru-table' },
          h('thead', {}, h('tr', {}, h('th', {}, 'Description'), h('th', { class: 'num' }, 'Count'), h('th', { class: 'num' }, 'Total'), h('th', {}, 'Kind'), h('th', {}, ''))), tb),
        ...(list.length > sortLimit ? [btn(`Show ${Math.min(40, list.length - sortLimit)} more`, () => { sortLimit += 40; draw(); })] : []));
    }

    function drawCategories(): HTMLElement {
      const wrap = h('div', {});
      const q = query.trim().toLowerCase();
      const search = input(query, { placeholder: 'Find a category, text or merchant…', 'aria-label': 'Search categories', class: 'pe-in ru-search' });
      search.addEventListener('input', () => { query = search.value; const pos = search.selectionStart; draw(); const s = box.querySelector<HTMLInputElement>('.ru-search'); s?.focus(); s?.setSelectionRange(pos, pos); });
      // New category.
      const ncName = input('', { placeholder: 'New category', 'aria-label': 'New category name' });
      const ncSub = input('', { placeholder: 'first subcategory', 'aria-label': 'First subcategory' });
      const ncKind = sel((['spend', 'income', 'transfer'] as CategoryKind[]).map((k) => ({ v: k, t: KIND_LABEL[k] })), 'spend');
      wrap.append(h('div', { class: 'ru-bar' }, search,
        h('span', { class: 'ru-new' }, ncName, ncSub, ncKind,
          btn('+ Add', () => { if (ncName.value.trim()) apply([{ op: 'add_subcategory', category: ncName.value, subcategory: ncSub.value.trim() || ncName.value, kind: ncKind.value as CategoryKind }]); }))),
        h('p', { class: 'muted' }, 'A transaction goes to the subcategory whose text appears in its description. When several fit, the longest text wins (so “UBER EATS” beats “UBER”); write /…/ for a pattern. ',
          btn('? How this works', () => { showHelp = true; draw(); }, 'btn small ru-helplink')));

      const catNames = lab.categories.map((c) => c.name);
      for (const c of lab.categories) {
        const subs = c.subs.filter((s) => s.source !== 'off');
        if (!subs.length) continue;
        const crow = lab.rows.filter((r) => r.hit?.category === c.name);
        const matchesQ = !q || c.name.toLowerCase().includes(q) || subs.some((s) => s.name.toLowerCase().includes(q) || s.patterns.some((p) => p.toLowerCase().includes(q)))
          || crow.some((r) => r.merchant.toLowerCase().includes(q));
        if (!matchesQ) continue;
        const det = h('details', { class: 'ru-cat', 'data-category': c.name, 'data-key': `cat:${c.name}` }) as HTMLDetailsElement;
        if (q) det.open = true;
        const name = input(c.name, { 'aria-label': `Rename category ${c.name}`, class: 'pe-in ru-cat-name' });
        name.addEventListener('change', () => {
          if (name.value.trim() && name.value.trim() !== c.name) { openKeys.add(`cat:${name.value.trim()}`); apply([{ op: 'rename_category', from: c.name, to: name.value }]); }
        });
        const kind = sel((['spend', 'income', 'transfer'] as CategoryKind[]).map((k) => ({ v: k, t: KIND_LABEL[k] })), c.kind);
        kind.setAttribute('aria-label', `How ${c.name} counts`);
        kind.addEventListener('change', () => apply([{ op: 'set_kind', category: c.name, kind: kind.value as CategoryKind }]));
        det.append(h('summary', {}, h('b', {}, c.name), h('span', { class: `ru-kind k-${c.kind}` }, KIND_LABEL[c.kind]),
          h('span', { class: 'muted' }, ` ${plural(subs.length, 'subcategory')} · ${plural(crow.length, 'transaction')}` + (crow.length ? ` · ${money(sum(crow))}` : ''))));
        det.append(h('div', { class: 'ru-cat-tools' }, 'Name ', name, ' counts as ', kind,
          btn('🗑 Delete category', () => apply([{ op: 'delete_category', category: c.name }]), 'btn small pe-del',
            'Its transactions go back to “to sort” (or to another rule that fits). Built-in ones are switched off.')));
        for (const s of subs) {
          const srow = crow.filter((r) => r.hit?.subcategory === s.name);
          const sname = input(s.name, { 'aria-label': `Rename subcategory ${s.name}` });
          sname.addEventListener('change', () => { if (sname.value.trim() && sname.value.trim() !== s.name) apply([{ op: 'rename_subcategory', category: c.name, from: s.name, to: sname.value }]); });
          const move = sel([{ v: '', t: 'Move to…' }, ...catNames.filter((n) => n !== c.name).map((n) => ({ v: n, t: n }))], '');
          move.setAttribute('aria-label', `Move ${s.name} to another category`);
          move.addEventListener('change', () => { if (move.value) apply([{ op: 'move_subcategory', category: c.name, subcategory: s.name, to_category: move.value }]); });
          const chips = h('div', { class: 'ru-chips' });
          for (const r of srow.filter((x) => x.hit?.pinned)) {
            chips.append(h('span', { class: 'ru-chip ru-pin', title: `Pinned by hand: only this transaction (${r.raw})` },
              `📌 ${r.date} · ${r.merchant.length > 32 ? `${r.merchant.slice(0, 30)}…` : r.merchant} · ${money(Number(r.amount))}`,
              btn('×', () => apply([{ op: 'unpin', date: r.date, amount: r.amount, description: r.raw }]), 'ru-x', 'Unpin: let the texts decide again')));
          }
          const rangedOf = (p: string) => {
            const rg = (s.ranged ?? []).find((x) => x.pattern === p);
            return rg ? h('span', { class: 'ru-rg', title: 'This text has its own amount range' }, rangeText(limitOf(rg.amount_min), limitOf(rg.amount_max))) : '';
          };
          for (const p of [...s.patterns, ...(s.ranged ?? []).map((x) => x.pattern).filter((x) => !s.patterns.includes(x))]) {
            const n = srow.filter((r) => !r.hit?.pinned && r.hit?.pattern === p).length;
            chips.append(h('span', { class: 'ru-chip' + (n ? '' : ' idle'), title: n ? `${plural(n, 'transaction')} sorted by this` : 'Catches nothing in this project' },
              btn(describePattern(p), () => sortIt(srow.filter((r) => !r.hit?.pinned && r.hit?.pattern === p), 'category', false, { category: c.name, subcategory: s.name, pattern: p }),
                'ru-chip-t', 'Edit this text: see every transaction it catches, change it, give it its own amount range or move it'),
              rangedOf(p), h('span', { class: 'ru-n' }, String(n)),
              btn('×', () => apply([{ op: 'remove_pattern', category: c.name, subcategory: s.name, pattern: p }]), 'ru-x', `Remove ${describePattern(p)}`)));
          }
          const add = input('', { placeholder: '+ text in the description', 'aria-label': `Add text to ${s.name}`, class: 'pe-in ru-add' });
          add.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && add.value.trim()) apply([{ op: 'add_pattern', category: c.name, subcategory: s.name, pattern: patternFromInput(add.value, false) }]);
          });
          chips.append(add);
          const lo = input(s.amount_min ?? '', { placeholder: 'any', 'aria-label': 'Smallest amount', class: 'pe-in ru-amt' });
          const hi = input(s.amount_max ?? '', { placeholder: 'any', 'aria-label': 'Largest amount', class: 'pe-in ru-amt' });
          const setAmt = () => apply([{ op: 'set_amounts', category: c.name, subcategory: s.name, amount_min: lo.value || null, amount_max: hi.value || null }]);
          lo.addEventListener('change', setAmt); hi.addEventListener('change', setAmt);
          const prio = h('input', { type: 'number', class: 'pe-in ru-prio', step: '1', 'aria-label': 'Tie-break priority' }) as HTMLInputElement;
          const limitsSet = s.amount_min != null || s.amount_max != null || s.priority !== 0;
          const limitsLabel = limitsSet
            ? [s.amount_min != null || s.amount_max != null ? `amounts ${s.amount_min ?? '0'} – ${s.amount_max ?? 'any'}` : '', s.priority ? `priority ${s.priority}` : ''].filter(Boolean).join(' · ')
            : 'amount limits';
          prio.value = String(s.priority);
          prio.addEventListener('change', () => apply([{ op: 'set_priority', category: c.name, subcategory: s.name, priority: Math.trunc(Number(prio.value) || 0) }]));
          det.append(h('div', { class: 'ru-sub', 'data-sub': s.name },
            h('div', { class: 'ru-sub-head' }, sname,
              h('span', { class: `ru-src s-${s.source}` }, SOURCE_LABEL[s.source] ?? s.source),
              h('span', { class: 'muted' }, `${plural(srow.length, 'transaction')}` + (srow.length ? ` · ${money(sum(srow))}` : '')),
              h('span', { class: 'ru-grow' }), move,
              btn('🗑', () => apply([{ op: 'delete_subcategory', category: c.name, subcategory: s.name }]), 'btn small pe-del', `Delete ${s.name}${s.source !== 'yours' ? ' (a built-in is switched off)' : ''}`)),
            chips,
            h('details', { class: 'ru-adv', 'data-key': `adv:${c.name}\u0000${s.name}`, ...(limitsSet ? { open: '' } : {}) }, h('summary', { class: 'muted' }, limitsLabel),
              h('div', { class: 'ru-limits muted' },
                h('span', { title: 'This subcategory’s texts only count when the amount, ignoring its sign, is in this range (ends included). A text with its own range (click the text) ignores this. Empty = no limit.' }, 'Texts without their own range: only amounts from '), lo, ' to ', hi,
                h('span', { title: 'Only matters when two equally long texts from different subcategories both fit: the higher number wins. Normally 0.' }, ' · tie-break priority ⓘ '), prio))));
        }
        const nsub = input('', { placeholder: '+ New subcategory', 'aria-label': `New subcategory in ${c.name}` });
        nsub.addEventListener('keydown', (e) => { if (e.key === 'Enter' && nsub.value.trim()) apply([{ op: 'add_subcategory', category: c.name, subcategory: nsub.value }]); });
        det.append(h('div', { class: 'ru-sub ru-newsub' }, nsub));
        wrap.append(det);
      }
      return wrap;
    }

    function drawTransfers(): HTMLElement {
      const tr = lab.rows.filter((r) => r.kind === 'transfer');
      const by = (how: string) => tr.filter((r) => r.transfer?.how === how);
      const order = triedOrder(lab.rules);
      const targets = lab.targets.map((t) => ({ v: t, t }));
      const intro = h('div', { class: 'ru-tr-intro' });
      intro.innerHTML = `
<p><b>A transfer is money moving, not money spent or earned.</b> It's left out of both spending and income. Examples:
between your own accounts (chequing → savings), <b>paying off a credit card</b> (the purchases already counted as spending when you made them —
counting the payment too would count them twice), and money to or from people you track.</p>
<p class="muted">What makes a row a transfer: the bank's file marks it so (a card “PAYMENT”), or it's in a category that counts as
<i>Moving money</i> — the built-in <i>Transfers</i> category catches e-Transfers. If an e-Transfer was really a purchase (rent, a sale),
sort it into a spending category with a longer text and it counts as spending.</p>`;
      const stats = h('div', { class: 'ru-stats' },
        h('span', { title: 'Both halves are in your statements (−500 leaves chequing, +500 reaches savings within 3 days), so they were matched. Nothing to do.' },
          h('b', {}, String(by('paired').length)), ' paired automatically — both halves are in your statements'),
        h('span', { title: 'Only one half is in your files; a rule below names the other side.' }, h('b', {}, String(by('rule').length)), ' placed by a rule'),
        h('span', { class: by('unmatched').length ? 'warn' : '', title: 'Still left out of spending and income — they just show as “Unmatched” in Money between accounts and people.' },
          h('b', {}, String(by('unmatched').length)), ' unknown other side'));
      const wrap = h('div', {}, intro, stats,
        h('p', { class: 'muted' }, 'When only one half is in your files — an e-Transfer to someone, a savings account at a bank you didn’t export — these rules name the other side. They’re tried from the top; the first whose text is in the description wins. Money out = sent to them, money in = received from them, so one rule covers both directions. ',
          btn('? More', () => { showHelp = true; draw(); }, 'btn small ru-helplink')));
      const list = h('ol', { class: 'ru-rules' });
      order.forEach((i, rank) => {
        const r = lab.rules[i];
        const caught = by('rule').filter((x) => x.transfer?.rule === i);
        const lit = literalOf(r.pattern);
        const text = input(lit ?? `/${r.pattern}/`, { 'aria-label': `Rule ${rank + 1} text`, class: 'pe-in ru-rule-text' });
        const to = sel(targets.some((t) => t.v === r.to) ? targets : [...targets, { v: r.to, t: `${r.to} (missing!)` }], r.to);
        to.setAttribute('aria-label', `Rule ${rank + 1} target`);
        const upd = () => apply([], [{ op: 'update_rule', index: i, pattern: patternFromInput(text.value, true), to: to.value }]);
        text.addEventListener('change', upd); to.addEventListener('change', upd);
        const move = (d: number) => { const o = [...order]; const j = rank + d; [o[rank], o[j]] = [o[j], o[rank]]; apply([], [{ op: 'order_rules', order: o }]); };
        const up = btn('↑', () => move(-1), 'btn small', 'Try earlier'); up.disabled = rank === 0;
        const down = btn('↓', () => move(1), 'btn small', 'Try later'); down.disabled = rank === order.length - 1;
        const sent = caught.filter((x) => Number(x.amount) < 0), got = caught.filter((x) => Number(x.amount) >= 0);
        const eg = [...new Set(caught.map((x) => x.merchant))].slice(0, 3).map((m) => `“${m}”`).join(', ');
        const det = h('details', { class: 'ru-caught', 'data-key': `rule:${r.pattern}\u0000${r.to}` },
          h('summary', { class: 'muted' }, caught.length
            ? `caught ${plural(caught.length, 'transfer')}: ${sent.length} sent to ${r.to} (${money(sum(sent))}), ${got.length} received from ${r.to} (${money(sum(got))})`
            : 'caught nothing yet'),
          caught.length ? h('div', { class: 'muted ru-eg' }, `e.g. ${eg}`) : '', grouped(caught));
        text.title = lit != null ? `Plain text, any case: “${lit}”` : `Pattern (Rust regex): ${r.pattern.replace(/^\(\?i\)/, '')}${/^\(\?i\)/.test(r.pattern) ? ' — any case' : ' — case matters'}`;
        list.append(h('li', { 'data-rule': String(i) },
          h('div', { class: 'ru-rule' }, 'Description contains ', text, ' → other side: ', to, up, down,
            btn('🗑', () => apply([], [{ op: 'delete_rule', index: i }]), 'btn small pe-del', 'Delete this rule')), det));
      });
      wrap.append(list);
      const nt = input('', { placeholder: 'text in the description', 'aria-label': 'New rule text' });
      const nto = sel(targets, targets[0]?.v ?? '');
      nto.setAttribute('aria-label', 'New rule target');
      wrap.append(h('div', { class: 'pe-add' }, h('b', {}, '+ Rule: '), 'description contains ', nt, ' → ', nto,
        btn('Add', () => { if (nt.value.trim()) apply([], [{ op: 'add_rule', pattern: patternFromInput(nt.value, true), to: nto.value }]); })));

      const un = by('unmatched');
      if (un.length) {
        const groups = new Map<string, LabRow[]>();
        for (const r of un) groups.set(r.merchant, [...(groups.get(r.merchant) ?? []), r]);
        const tb = h('tbody', {});
        for (const [m, rs] of [...groups.entries()].sort((a, b) => Math.abs(sum(b[1])) - Math.abs(sum(a[1]))).slice(0, 60)) {
          tb.append(h('tr', {}, h('td', {}, h('b', {}, m)), h('td', { class: 'muted' }, [...new Set(rs.map((r) => r.account))].join(', ')),
            h('td', { class: 'num' }, String(rs.length)), h('td', { class: 'num ' + (sum(rs) < 0 ? 'neg' : 'pos') }, money(sum(rs))),
            h('td', {}, btn('Make a rule…', () => sortIt(rs, 'transfer'), 'btn small active'))));
        }
        wrap.append(h('h4', {}, `Transfers with no known other side (${un.length})`),
          h('table', { class: 'map-table ru-table' },
            h('thead', {}, h('tr', {}, h('th', {}, 'Description'), h('th', {}, 'From account'), h('th', { class: 'num' }, 'Count'), h('th', { class: 'num' }, 'Total'), h('th', {}, ''))), tb));
      }
      return wrap;
    }

    function draw() {
      const changed = diffLabs(saved, lab);
      const nChanged = [...changed.values()].reduce((a, r) => a + r.length, 0);
      const toSort = lab.rows.filter(isToSort).length;
      const unknownTr = lab.rows.filter((r) => r.transfer?.how === 'unmatched').length;
      const tabBtn = (t: Tab, label: string) => {
        const b = btn(label, () => { tab = t; draw(); }, 'btn ru-tab' + (tab === t ? ' active' : ''));
        b.setAttribute('role', 'tab'); b.setAttribute('aria-selected', String(tab === t)); b.dataset.tab = t;
        return b;
      };
      const body = tab === 'sort' ? drawSort() : tab === 'categories' ? drawCategories() : drawTransfers();
      const changes = h('details', { class: 'ru-changes', 'data-key': 'changes' },
        h('summary', {}, nChanged ? `${plural(nChanged, 'transaction')} sorted differently than when you opened this` : 'No changes yet'),
        ...[...changed.entries()].sort((a, b) => b[1].length - a[1].length).map(([k, rs]) =>
          h('details', { 'data-key': `chg:${k}` }, h('summary', {}, `${k} · ${rs.length}`), grouped(rs))));
      const undoBtn = btn('↶ Undo', () => { const u = undo.pop(); if (u) { cat = u.cat; ppl = u.ppl; lab = labFor(cat, ppl); error = ''; draw(); } }, 'btn');
      undoBtn.disabled = !undo.length;
      const saveBtn = btn('Save', save, 'btn active');
      saveBtn.disabled = !undo.length;
      const scroll = box.querySelector('.ru-body')?.scrollTop ?? 0;
      // Keep focus on the same field (e.g. "+ text" after Enter) across the redraw.
      const act = document.activeElement;
      const focusLabel = act instanceof HTMLElement && box.contains(act) ? act.getAttribute('aria-label') : null;
      const helpBtn = btn(showHelp ? '? Hide help' : '? Help', () => { showHelp = !showHelp; draw(); }, 'btn small' + (showHelp ? ' active' : ''), 'How sorting and transfer rules work');
      helpBtn.setAttribute('aria-expanded', String(showHelp));
      const help = showHelp ? helpBox(tab === 'transfers' ? HELP_TRANSFERS : HELP_MATCH) : null;
      box.replaceChildren(
        h('div', { class: 'map-head' }, h('h3', {}, 'Categories & rules'), helpBtn, btn('✕', () => finish(false), 'btn small', 'Close without saving (Esc)')),
        h('div', { class: 'ru-tabs', role: 'tablist' },
          tabBtn('sort', `To sort (${toSort})`), tabBtn('categories', 'Categories'), tabBtn('transfers', `Transfer rules${unknownTr ? ` (${unknownTr} unknown)` : ''}`)),
        ...(error ? [h('div', { class: 'map-problems' }, error)] : []),
        h('div', { class: 'ru-body' }, ...(help ? [help] : []), body),
        changes,
        h('div', { class: 'map-actions' },
          h('span', { class: 'muted pe-count' }, undo.length ? `${plural(undo.length, 'change')} not saved yet · saved to this project’s categories.toml / people.toml` : 'Saved to this project’s categories.toml and people.toml (part of the config bundle).'),
          undoBtn, btn('Cancel', () => finish(false), 'btn'), saveBtn));
      for (const d of box.querySelectorAll<HTMLDetailsElement>('details[data-key]')) {
        const k = d.dataset.key!;
        if (openKeys.has(k)) d.open = true;
        d.addEventListener('toggle', () => {
          if (!d.open) openKeys.delete(k);
          else if (!(query.trim() && k.startsWith('cat:'))) openKeys.add(k); // search opens them all; don't remember that
        });
      }
      const b = box.querySelector('.ru-body'); if (b) b.scrollTop = scroll;
      if (focusLabel) box.querySelector<HTMLElement>(`[aria-label="${CSS.escape(focusLabel)}"]`)?.focus();
    }
    draw();

    if (opts.example) {
      const ex = opts.example;
      const hits = lab.rows.filter((r) => r.merchant === ex.merchant && (!ex.date || r.date === ex.date) && (!ex.account || r.account === ex.account));
      const rows = hits.length ? hits : lab.rows.filter((r) => r.merchant === ex.merchant);
      if (rows.length) sortIt(rows, rows[0].kind === 'transfer' && rows[0].transfer?.how !== 'paired' ? 'transfer' : 'category', true);
    }
  });
}

// ---------- "Sort…": make or edit a rule, on the list of transactions it catches ----------

interface MakerResult { cat: CategoryOp[]; ppl: PeopleOp[] }
/** An existing text being edited. */
export interface EditTarget { category: string; subcategory: string; pattern: string }

const absAmt = (r: LabRow) => Math.abs(Number(r.amount));
/** A limit string as a number ('' / null = none). */
export const limitOf = (v: string | null | undefined) => (v == null || v.trim() === '' || Number.isNaN(Number(v)) ? null : Math.abs(Number(v)));
export const inRange = (a: number, lo: number | null, hi: number | null) => (lo == null || a >= lo) && (hi == null || a <= hi);
/**
 * The amount range that ticked amounts ask for, open-ended where they reach the
 * smallest/largest of `all`; `leaks` = unticked amounts that still fall inside.
 */
export function rangeFromTicks(ticked: number[], all: number[]): { lo: number | null; hi: number | null; leaks: number } {
  if (!ticked.length || !all.length) return { lo: null, hi: null, leaks: 0 };
  const tmin = Math.min(...ticked), tmax = Math.max(...ticked);
  const lo = tmin <= Math.min(...all) ? null : tmin, hi = tmax >= Math.max(...all) ? null : tmax;
  return { lo, hi, leaks: all.filter((a) => inRange(a, lo, hi)).length - ticked.length };
}
/** Longest common substring. */
export function lcs(a: string, b: string): string {
  let best = 0, end = 0;
  let prev = new Array<number>(b.length + 1).fill(0);
  for (let i = 1; i <= a.length; i++) {
    const cur = new Array<number>(b.length + 1).fill(0);
    for (let j = 1; j <= b.length; j++) {
      if (a[i - 1] === b[j - 1]) { cur[j] = prev[j - 1] + 1; if (cur[j] > best) { best = cur[j]; end = i; } }
    }
    prev = cur;
  }
  return a.slice(end - best, end);
}
/** Every text of a subcategory with the amount range it applies under. */
export function textsOf(s: LabSub): { pattern: string; lo: number | null; hi: number | null; ranged: boolean }[] {
  return [
    ...s.patterns.map((p) => ({ pattern: p, lo: limitOf(s.amount_min), hi: limitOf(s.amount_max), ranged: false })),
    ...(s.ranged ?? []).map((r) => ({ pattern: r.pattern, lo: limitOf(r.amount_min), hi: limitOf(r.amount_max), ranged: true })),
  ];
}
const rangeText = (lo: number | null, hi: number | null) =>
  lo == null && hi == null ? 'any amount' : lo == null ? `up to ${money(hi!)}` : hi == null ? `${money(lo)} and up` : `${money(lo)} – ${money(hi)}`;
export interface NearMiss extends EditTarget { why: 'close' | 'range'; common: string; note: string }
/**
 * Existing texts that *almost* sort this row: one that matches but whose amount
 * range leaves it out (Coffee up to 20 vs a 46.00 one), or a literal that
 * nearly appears ("FISHY JOES" vs "FISHY JOE GRILL"). Editing it is usually
 * better than adding a second rule.
 */
export function nearMisses(row: LabRow, cats: LabCategory[]): NearMiss[] {
  const out: NearMiss[] = [];
  const M = row.merchant.toUpperCase(), Raw = row.raw.toUpperCase();
  for (const c of cats) {
    for (const s of c.subs) {
      if (s.source === 'off') continue;
      for (const t of textsOf(s)) {
        if (row.hit && row.hit.category === c.name && row.hit.subcategory === s.name && row.hit.pattern === t.pattern) continue;
        const base = { category: c.name, subcategory: s.name, pattern: t.pattern };
        if (jsMatcher(t.pattern)(row)) {
          if (!inRange(absAmt(row), t.lo, t.hi)) {
            out.push({ ...base, why: 'range', common: literalOf(t.pattern) ?? '',
              note: `${describePattern(t.pattern)} in ${c.name} › ${s.name} already matches, but only takes ${rangeText(t.lo, t.hi)}${t.ranged ? '' : ` (${s.name}’s amount limit)`}` });
          }
          continue;
        }
        const lit = literalOf(t.pattern)?.toUpperCase();
        if (!lit || lit.length < 4) continue;
        const common = [lcs(lit, M), lcs(lit, Raw)].sort((a, b) => b.length - a.length)[0].trim();
        if (common.length >= Math.max(4, lit.length - 2)) {
          out.push({ ...base, why: 'close', common, note: `${describePattern(t.pattern)} in ${c.name} › ${s.name} almost matches (“${common}” does)` });
        }
      }
    }
  }
  return out.sort((a, b) => (a.why === b.why ? 0 : a.why === 'range' ? -1 : 1)).slice(0, 3);
}

function openRuleMaker(ctx: Ctx, examples: LabRow[], mode0: 'category' | 'transfer', standalone: boolean, edit0?: EditTarget): Promise<MakerResult | null> {
  return new Promise((resolve) => {
    const lab = ctx.lab;
    const ex: LabRow | undefined = examples[0];
    const { box, close } = modal('ru-maker-overlay', edit0 ? 'Edit a text' : 'Make a rule', () => done(null));
    box.classList.add('ru-maker');
    const done = (r: MakerResult | null) => { close(); resolve(r); };
    let mode = mode0;
    let editing: EditTarget | null = edit0 ?? null;
    let scope: 'text' | 'pins' = 'text';
    const picked = new Set<LabRow>();
    const expanded = new Set<LabRow>();
    let sortBy: 'date' | 'amount' = 'date';
    let showAll = false;
    let rangeTouched = false;
    let next: Lab | null = null;
    const idx = new Map(lab.rows.map((r, i) => [r, i]));
    const subOf = (c: string, s: string) => lab.categories.find((x) => x.name === c)?.subs.find((x) => x.name === s);

    // ---- what identifies it
    const skip = ex ? boilerplateWords(ex.merchant, lab.rows.filter((r) => r.account === ex.account).map((r) => r.merchant)) : 0;
    const shown = ex ? (skip ? dropWords(ex.merchant, skip) : ex.merchant) : '';
    const sugg = ex ? suggestTexts(ex.merchant, skip) : [];
    const pool = () => lab.rows.filter((r) => (mode === 'transfer') === (r.kind === 'transfer'));
    const startText = editing ? (literalOf(editing.pattern) ?? `/${editing.pattern}/`) : (sugg[0] ?? ex?.merchant ?? '');
    const text = input(startText, { 'aria-label': 'Text that identifies it', class: 'pe-in ru-maker-text' });
    const chips = h('span', { class: 'ru-sugg' });
    const drawChips = () => chips.replaceChildren(...sugg.map((sg) => {
      const n = pool().filter(jsMatcher(catPattern(sg))).length;
      const b = btn('', () => { text.value = sg; refresh(); }, 'btn small', `${plural(n, 'transaction')} contain this`);
      b.append(sg, h('span', { class: 'ru-n' }, String(n)));
      return b;
    }));
    const lo = input('', { placeholder: 'any', 'aria-label': 'Smallest amount', class: 'pe-in ru-amt' });
    const hi = input('', { placeholder: 'any', 'aria-label': 'Largest amount', class: 'pe-in ru-amt' });
    const rangeNote = h('span', { class: 'muted ru-range-note' });
    const rangeRow = h('div', { class: 'ru-row ru-range' }, 'Amounts (ignoring the sign) from ', lo, ' to ', hi, rangeNote);
    for (const e of [lo, hi]) e.addEventListener('input', () => { rangeTouched = true; later(); });

    // ---- where it goes: a category…
    const live = lab.categories.flatMap((c) => c.subs.filter((s) => s.source !== 'off').map((s) => ({ c, s })));
    const key = (c: string, s: string) => `${c}\u0000${s}`;
    const cur = editing ? key(editing.category, editing.subcategory) : ex?.hit ? key(ex.hit.category, ex.hit.subcategory) : '';
    const target = sel([
      { v: '', t: '— choose —' },
      ...live.map(({ c, s }) => ({ v: key(c.name, s.name), t: `${c.name} › ${s.name}`, group: `${c.name}${c.kind === 'spend' ? '' : ` (${KIND_LABEL[c.kind].toLowerCase()})`}` })),
      { v: '+new', t: '+ New category or subcategory…' },
    ], cur, 'map-sel ru-target');
    target.setAttribute('aria-label', 'Category');
    const newCat = input('', { placeholder: 'Category', 'aria-label': 'New category', list: 'ru-catlist' });
    const newSub = input('', { placeholder: 'Subcategory', 'aria-label': 'New subcategory' });
    const newKind = sel((['spend', 'income', 'transfer'] as CategoryKind[]).map((k) => ({ v: k, t: KIND_LABEL[k] })), ex?.kind === 'income' ? 'income' : 'spend');
    const dl = h('datalist', { id: 'ru-catlist' }, ...lab.categories.map((c) => h('option', { value: c.name })));
    const newBits = h('span', { class: 'ru-newbits' }, newCat, ' › ', newSub, ' ', newKind, dl);
    // …or a transfer target.
    const guess = ex ? lab.targets.find((t) => t.length >= 3 && ex.merchant.toLowerCase().includes(t.toLowerCase())) ?? '' : '';
    const to = sel([{ v: '', t: '— choose —' }, ...lab.targets.map((t) => ({ v: t, t })), { v: '+new', t: '+ New person…' }], guess);
    to.setAttribute('aria-label', 'Where the money went');
    const newPerson = input('', { placeholder: 'Their name', 'aria-label': 'New person' });
    const newPersonBits = h('span', { class: 'ru-newbits' }, newPerson, h('span', { class: 'muted' }, ' (added to People & accounts)'));

    const isNewCat = () => target.value === '+new';
    const catOf = (): { category: string; subcategory: string; kind?: CategoryKind } | null => {
      if (isNewCat()) {
        const c = newCat.value.trim(), sb = newSub.value.trim() || c;
        if (!c) return null;
        return { category: c, subcategory: sb, ...(lab.categories.some((x) => x.name === c) ? {} : { kind: newKind.value as CategoryKind }) };
      }
      if (!target.value) return null;
      const [c, sb] = target.value.split('\u0000');
      return { category: c, subcategory: sb };
    };
    /** The range a plain text in the chosen subcategory gets (its limits). */
    const subRange = () => { const c = catOf(); const sb = c && subOf(c.category, c.subcategory); return { lo: limitOf(sb?.amount_min), hi: limitOf(sb?.amount_max) }; };
    const curRange = () => ({ lo: limitOf(lo.value), hi: limitOf(hi.value) });
    const fmtLimit = (n: number | null) => (n == null ? '' : String(n));
    /** Prefill the range: the edited text's own, else the subcategory's — widened so the examples fit. */
    const prefillRange = () => {
      if (rangeTouched) return;
      let r = subRange();
      if (editing) {
        const sb = subOf(editing.category, editing.subcategory);
        const t = sb && textsOf(sb).find((x) => x.pattern === editing!.pattern);
        if (t) r = { lo: t.lo, hi: t.hi };
      }
      if (!editing && ex) {
        const amts = examples.map(absAmt);
        if (r.lo != null && Math.min(...amts) < r.lo) r = { ...r, lo: null };
        if (r.hi != null && Math.max(...amts) > r.hi) r = { ...r, hi: null };
      }
      lo.value = fmtLimit(r.lo); hi.value = fmtLimit(r.hi);
    };

    const ops = (): MakerResult | null => {
      if (mode === 'transfer') {
        if (!text.value.trim()) return null;
        const top = Math.max(0, ...lab.rules.map((r) => r.priority)) + 1;
        const pattern = patternFromInput(text.value, true);
        if (to.value === '+new') {
          const who = newPerson.value.trim();
          return who ? { cat: [], ppl: [{ op: 'add_person', id: who }, { op: 'add_rule', pattern, to: who, priority: top }] } : null;
        }
        if (!to.value) return null;
        return { cat: [], ppl: [{ op: 'add_rule', pattern, to: to.value, priority: top }] };
      }
      const c = catOf();
      if (!c) return null;
      if (scope === 'pins') {
        if (!picked.size) return null;
        return { cat: [...picked].map((r): CategoryOp => ({ op: 'pin', date: r.date, amount: r.amount, description: r.raw, ...c })), ppl: [] };
      }
      if (!text.value.trim()) return null;
      const pattern = patternFromInput(text.value, false);
      const want = curRange(), plain = subRange();
      const range = want.lo === plain.lo && want.hi === plain.hi ? undefined : { amount_min: fmtLimit(want.lo) || null, amount_max: fmtLimit(want.hi) || null };
      if (editing) {
        if (editing.category === c.category && editing.subcategory === c.subcategory) {
          return { cat: [{ op: 'edit_pattern', category: c.category, subcategory: c.subcategory, from: editing.pattern, pattern, range }], ppl: [] };
        }
        return { cat: [{ op: 'remove_pattern', category: editing.category, subcategory: editing.subcategory, pattern: editing.pattern }, { op: 'add_pattern', ...c, pattern, range }], ppl: [] };
      }
      const sb = subOf(c.category, c.subcategory);
      if (sb && textsOf(sb).some((t) => t.pattern === pattern)) {
        return { cat: [{ op: 'edit_pattern', category: c.category, subcategory: c.subcategory, from: pattern, pattern, range }], ppl: [] };
      }
      return { cat: [{ op: 'add_pattern', ...c, pattern, range }], ppl: [] };
    };

    // ---- the transactions list
    const matcher = () => {
      const t = text.value.trim();
      if (!t) return () => false;
      return jsMatcher(patternFromInput(t, mode === 'transfer'));
    };
    const listRows = (): LabRow[] => {
      const m = matcher();
      const set = new Set<LabRow>([...pool().filter(m), ...examples, ...picked]);
      if (editing) for (const r of lab.rows) if (r.hit?.pattern === editing.pattern && r.hit.category === editing.category && r.hit.subcategory === editing.subcategory) set.add(r);
      const rows = [...set];
      return rows.sort(sortBy === 'amount' ? (a, b) => absAmt(b) - absAmt(a) : (a, b) => b.date.localeCompare(a.date));
    };
    const ticked = (r: LabRow) => (scope === 'pins' ? picked.has(r) : inRange(absAmt(r), curRange().lo, curRange().hi) && matcher()(r));
    const listBox = h('div', { class: 'ru-list' });
    const listNote = h('div', { class: 'ru-list-note' });
    const reason = (r: LabRow) => {
      if (r.kind === 'transfer') return r.transfer?.how === 'paired' ? `paired with ${r.transfer.other}` : r.transfer?.how === 'rule' ? `rule → ${r.transfer.other}` : 'transfer, other side unknown';
      if (!r.hit) return 'no text matches';
      return r.hit.pinned ? 'pinned by hand' : `contains ${describePattern(r.hit.pattern)}`;
    };
    const drawList = () => {
      const rows = listRows();
      const isText = mode === 'category' && scope === 'text';
      const tickable = mode === 'category';
      const tb = h('tbody', {});
      for (const r of showAll ? rows : rows.slice(0, 150)) {
        const i = idx.get(r)!;
        const after = next ? next.rows[i] : null;
        const nowL = rowLabel(r), afterL = after ? rowLabel(after) : '…';
        const cb = h('input', { type: 'checkbox', 'aria-label': `Include ${r.date} ${r.amount}` }) as HTMLInputElement;
        cb.checked = ticked(r);
        cb.addEventListener('click', (e) => e.stopPropagation());
        cb.addEventListener('change', () => onTick(r, cb.checked));
        const tr = h('tr', { class: 'ru-li' + (afterL !== nowL && next ? ' ru-chg' : '') + (examples.includes(r) ? ' ru-ex' : '') },
          h('td', {}, tickable ? cb : ''), h('td', { class: 'muted' }, r.date), h('td', {}, r.merchant),
          h('td', { class: 'muted' }, r.account), h('td', { class: 'num ' + (Number(r.amount) < 0 ? 'neg' : 'pos') }, money(Number(r.amount))),
          h('td', { class: 'muted' }, nowL), h('td', {}, afterL === nowL ? h('span', { class: 'muted' }, 'stays') : `→ ${afterL}`));
        tr.addEventListener('click', () => { if (expanded.has(r)) expanded.delete(r); else expanded.add(r); drawList(); });
        tb.append(tr);
        if (expanded.has(r)) {
          tb.append(h('tr', { class: 'ru-detail' }, h('td', {}), h('td', { colspan: '6' },
            h('div', {}, h('span', { class: 'muted' }, 'Bank wording: '), r.raw),
            h('div', {}, h('span', { class: 'muted' }, 'File: '), r.file || '—', h('span', { class: 'muted' }, ' · Account: '), r.account, h('span', { class: 'muted' }, ' · Counts as: '), r.kind),
            h('div', {}, h('span', { class: 'muted' }, 'Now: '), `${nowL} — ${reason(r)}`),
            after ? h('div', {}, h('span', { class: 'muted' }, 'After: '), `${afterL}${after.hit ? ` — ${reason(after)}` : ''}`) : '')));
        }
      }
      const sortHead = (label: string, k: typeof sortBy, cls = '') => {
        const b = btn(label + (sortBy === k ? ' ▾' : ''), () => { sortBy = k; drawList(); }, 'ru-sorth', `Sort by ${label.toLowerCase()}`);
        return h('th', cls ? { class: cls } : {}, b);
      };
      listBox.replaceChildren(
        h('table', { class: 'map-table ru-sample ru-listt' },
          h('thead', {}, h('tr', {}, h('th', {}, ''), sortHead('Date', 'date'), h('th', {}, 'Description'), h('th', {}, 'Account'), sortHead('Amount', 'amount', 'num'), h('th', {}, 'Now'), h('th', {}, 'After'))),
          tb),
        ...(rows.length > 150 && !showAll ? [btn(`Show all ${rows.length}`, () => { showAll = true; drawList(); })] : []));
      listHead.textContent = scope === 'pins'
        ? `Tick the transactions to put there (${picked.size} ticked). Click a row for its details.`
        : isText ? `${plural(rows.length, 'transaction')} with this text — ticked = inside the amount range. Untick to leave some out; click a row for its details.`
          : `${plural(rows.length, 'transaction')} with this text. Click a row for its details.`;
    };
    const listHead = h('div', { class: 'muted ru-list-head' });
    const onTick = (r: LabRow, on: boolean) => {
      if (scope === 'pins') { if (on) picked.add(r); else picked.delete(r); refresh(); return; }
      const rows = listRows().filter(matcher());
      const t = new Set(rows.filter(ticked));
      if (on) t.add(r); else t.delete(r);
      const want = rangeFromTicks([...t].map(absAmt), rows.map(absAmt));
      rangeTouched = true;
      lo.value = fmtLimit(want.lo); hi.value = fmtLimit(want.hi);
      if (want.leaks > 0) {
        const tickedNow = [...t];
        listNote.replaceChildren(h('span', {}, `One amount range can’t leave out ${plural(want.leaks, 'transaction')} in between. `),
          btn('Sort exactly the ticked ones instead (one by one)', () => { scope = 'pins'; picked.clear(); for (const x of tickedNow) picked.add(x); drawScope(); refresh(); }, 'btn small'));
      } else listNote.replaceChildren();
      refresh();
    };

    // ---- step 1 scope, near misses
    const scopeSel = h('div', { class: 'ru-scope' });
    const drawScope = () => {
      if (mode !== 'category' || editing) { scope = 'text'; scopeSel.replaceChildren(); }
      else {
        const radio = (v: typeof scope, ...label: (Node | string)[]) => {
          const r = h('input', { type: 'radio', name: 'ru-scope', value: v, ...(scope === v ? { checked: '' } : {}) }) as HTMLInputElement;
          r.addEventListener('change', () => { scope = v; if (v === 'pins' && !picked.size && ex) picked.add(ex); listNote.replaceChildren(); rangeRow.hidden = scope !== 'text'; refresh(); });
          return h('label', { class: 'ru-scope-opt' }, r, ' ', ...label);
        };
        scopeSel.replaceChildren(
          radio('text', 'Every transaction with this text (and amount range) — now and in future statements'),
          radio('pins', 'Only the ones I tick', h('span', { class: 'muted' }, ' — for one-offs like a car, a gift or a refund; each is pinned on its own and nothing else is touched')));
      }
      rangeRow.hidden = mode !== 'category' || scope !== 'text';
    };
    const nearBox = h('div', { class: 'ru-near' });
    const drawNear = () => {
      if (!ex || editing || mode !== 'category') { nearBox.replaceChildren(); return; }
      const nm = nearMisses(ex, lab.categories);
      nearBox.replaceChildren(...nm.map((n) => h('div', { class: 'ru-near-item' }, `💡 ${n.note}. `,
        btn('Edit that text instead', () => {
          editing = { category: n.category, subcategory: n.subcategory, pattern: n.pattern };
          text.value = n.why === 'close' ? n.common : (literalOf(n.pattern) ?? `/${n.pattern}/`);
          target.value = key(n.category, n.subcategory);
          rangeTouched = false; prefillRange();
          if (n.why === 'range') { // widen it so the example fits
            const a = absAmt(ex);
            const r = curRange();
            if (r.lo != null && a < r.lo) lo.value = '';
            if (r.hi != null && a > r.hi) hi.value = '';
            rangeTouched = true;
          }
          drawTitle(); drawScope(); drawNear(); refresh();
        }, 'btn small active'))));
    };

    // ---- verdict and summary
    const modeSel = h('div', { class: 'ru-mode' });
    const verdict = h('div', { class: 'mg-verdict' });
    const summary = h('div', { class: 'ru-preview' });
    const applyBtn = btn(standalone ? 'Apply & save' : 'Apply', () => { const r = ops(); if (r) done(r); }, 'btn active');
    let t = 0;
    const refresh = () => {
      newBits.hidden = !isNewCat() || mode !== 'category';
      newPersonBits.hidden = to.value !== '+new';
      if (to.value === '+new' && !newPerson.value) newPerson.value = text.value.trim().replace(/\b\w+/g, (w) => w[0] + w.slice(1).toLowerCase());
      const { lo: l, hi: hh } = subRange();
      const c = catOf();
      rangeNote.textContent = c && !isNewCat() && (l != null || hh != null)
        ? ` · ${c.subcategory}’s other texts take ${rangeText(l, hh)}; a different range here applies to this text only.`
        : ' · leave empty for any amount.';
      const r = ops();
      next = null;
      if (!r) {
        applyBtn.disabled = true; verdict.className = 'mg-verdict warn';
        verdict.textContent = scope === 'pins' && !picked.size ? 'Tick at least one transaction below.' : !text.value.trim() ? 'Type the text that identifies it.' : mode === 'category' ? 'Choose where it belongs.' : 'Choose who or which account is on the other side.';
        summary.replaceChildren(); drawList(); return;
      }
      try { next = ctx.tryOps(r.cat, r.ppl); } catch (e) {
        applyBtn.disabled = true; verdict.className = 'mg-verdict bad'; verdict.textContent = (e as Error).message; summary.replaceChildren(); drawList(); return;
      }
      const diff = diffLabs(lab, next);
      const moved = [...diff.values()].reduce((a, x) => a + x.length, 0);
      const known = new Set(examples.map((e) => e.merchant));
      const odd = scope === 'pins' || editing ? [] : [...diff.values()].flat().filter((x) => !known.has(x.merchant));
      const goal = scope === 'pins' ? [...picked] : editing ? [] : examples;
      const goalMoved = goal.filter((e) => rowLabel(next!.rows[idx.get(e)!]) !== rowLabel(e)).length;
      applyBtn.disabled = moved === 0 && !(editing && r.cat.length);
      verdict.className = 'mg-verdict ' + (moved ? (odd.length || goalMoved < goal.length ? 'warn' : 'ok') : editing ? 'ok' : 'bad');
      verdict.textContent = !moved ? (editing ? 'No transaction would be sorted differently.' : 'This wouldn’t change anything — the list below says why for each transaction.')
        : `${plural(moved, 'transaction')} would be sorted differently` + (goal.length && goalMoved < goal.length ? ` — but ${goal.length - goalMoved} of your ${plural(goal.length, 'example')} wouldn’t (see “After” below)` : '') + '.'
          + (odd.length ? ` ${odd.length === 1 ? 'One has' : `${odd.length.toLocaleString()} have`} a different description than your example (⚠) — check they belong.` : '');
      // Why rows with the text stay where they are.
      const mine = r.cat.find((o) => 'pattern' in o && o.op !== 'remove_pattern') as { pattern: string } | undefined;
      const why = new Map<string, number>();
      if (mine && scope === 'text') {
        const m = matcher(), rg = curRange();
        for (const x of pool()) {
          if (!m(x) || rowLabel(next.rows[idx.get(x)!]) !== rowLabel(x)) continue;
          const after = next.rows[idx.get(x)!];
          const k = mode === 'category' && !inRange(absAmt(x), rg.lo, rg.hi) ? 'outside the amount range'
            : after.hit?.pinned ? 'pinned by hand to another place'
              : after.hit && after.hit.pattern !== mine.pattern ? (after.hit.pattern.length > mine.pattern.length ? `a longer text wins: ${describePattern(after.hit.pattern)} → ${after.hit.category} › ${after.hit.subcategory}` : `an equally long text wins the tie: ${describePattern(after.hit.pattern)} → ${after.hit.category} › ${after.hit.subcategory} (make yours longer)`)
                : x.transfer?.how === 'paired' ? 'paired with the other half of the transfer' : mode === 'transfer' ? 'an earlier rule wins' : 'already there';
          why.set(k, (why.get(k) ?? 0) + 1);
        }
      }
      summary.replaceChildren(
        ...[...diff.entries()].sort((a, b) => b[1].length - a[1].length).map(([k, rs]) => h('div', {}, h('b', {}, String(rs.length)), ` ${k}`)),
        ...[...why.entries()].filter(([k]) => k !== 'already there').map(([k, n]) => h('div', { class: 'muted' }, `${plural(n, 'transaction')} with this text stay: ${k}`)),
        ...(odd.length ? [h('details', {}, h('summary', { class: 'muted' }, 'Descriptions it would catch'), grouped([...diff.values()].flat(), known))] : []));
      drawList();
    };
    const later = () => { clearTimeout(t); t = window.setTimeout(refresh, 250); };
    text.addEventListener('input', later);
    target.addEventListener('change', () => { prefillRange(); refresh(); });
    for (const e of [newKind, to]) e.addEventListener('change', refresh);
    for (const e of [newCat, newSub, newPerson]) e.addEventListener('input', later);

    const drawMode = () => {
      const r1 = h('label', {}, h('input', { type: 'radio', name: 'ru-mode', value: 'category', ...(mode === 'category' ? { checked: '' } : {}) }), ' a category');
      const r2 = h('label', {}, h('input', { type: 'radio', name: 'ru-mode', value: 'transfer', ...(mode === 'transfer' ? { checked: '' } : {}) }), ' money moved to / from someone (a transfer rule)');
      modeSel.replaceChildren(ex?.kind === 'transfer' && !editing ? h('div', { class: 'ru-mode-pick' }, 'It belongs to ', r1, r2) : '',
        mode === 'category' ? h('div', { class: 'ru-row' }, target, ' ', newBits, editing ? h('span', { class: 'muted' }, ' (choosing another moves the text there)') : '')
          : h('div', { class: 'ru-row' }, 'The other side is ', to, ' ', newPersonBits, h('span', { class: 'muted' }, ' — tried before your other transfer rules.')));
      modeSel.querySelectorAll<HTMLInputElement>('input[name=ru-mode]').forEach((i) => i.addEventListener('change', () => { mode = i.value as typeof mode; drawMode(); drawScope(); drawChips(); drawNear(); refresh(); }));
    };

    const help = helpBox(mode0 === 'transfer' ? HELP_TRANSFERS : HELP_MATCH);
    help.hidden = true;
    const helpBtn = btn('? Help', () => { help.hidden = !help.hidden; helpBtn.textContent = help.hidden ? '? Help' : '? Hide help'; }, 'btn small', 'How matching works, and the pattern syntax');
    const title = h('h3', {});
    const drawTitle = () => { title.textContent = editing ? `Edit ${describePattern(editing.pattern)} — ${editing.category} › ${editing.subcategory}` : `Sort “${shown}”`; };
    const now = !ex ? '' : ex.kind === 'transfer'
      ? (ex.transfer?.how === 'paired' ? `a transfer paired with ${ex.transfer.other}` : ex.transfer?.how === 'rule' ? `a transfer to ${ex.transfer.other} (by a rule)` : 'a transfer with no known other side')
      : ex.hit ? `${ex.hit.category} › ${ex.hit.subcategory}, because ${reason(ex)}` : 'not in any category';
    drawTitle(); drawMode(); drawScope(); drawChips(); drawNear(); prefillRange();
    box.append(
      h('div', { class: 'map-head' }, title, helpBtn), help,
      ...(ex && !edit0 ? [h('div', { class: 'ru-example' },
        h('div', {}, h('span', { class: 'muted' }, 'Example: '), `${ex.date} · ${ex.account} · `, h('b', { class: Number(ex.amount) < 0 ? 'neg' : 'pos' }, money(Number(ex.amount))),
          examples.length > 1 ? h('span', { class: 'muted' }, ` (and ${examples.length - 1} more like it)`) : ''),
        ex.raw !== ex.merchant ? h('div', { class: 'muted' }, `Bank wording: ${ex.raw}`) : '',
        h('div', {}, h('span', { class: 'muted' }, 'Now: '), now))] : []),
      nearBox,
      h('h4', {}, editing ? '1. The text' : '1. Which transactions?'), scopeSel,
      h('div', { class: 'ru-row' }, text), sugg.length && !edit0 ? h('div', { class: 'ru-row muted' }, 'Try: ', chips) : '',
      rangeRow,
      h('p', { class: 'muted ru-hint' }, 'Upper/lower case doesn’t matter; the text can be anywhere in the description. Shorter text catches more, longer is more precise. Write /…/ for a pattern (see ? Help).'),
      h('h4', {}, '2. Where it belongs'), modeSel,
      h('h4', {}, '3. What changes'), verdict, summary,
      listHead, listNote, listBox,
      h('div', { class: 'map-actions' }, btn('Cancel', () => done(null), 'btn'), applyBtn));
    refresh();
    text.focus(); text.select();
  });
}
