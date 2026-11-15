// Manual column mapping for a statement file no format recognises. Shows the
// top rows, lets the user label columns, writes a new [[profile]] block into the
// project's profiles.toml, and previews the result with the real Rust parser
// before saving. Everything stays in the browser.
import { checkFiles, defaultConfig, projectConfig, type FileCheck } from './engine';
import { upsertFile, type ProjectDoc, type ProjectFile } from './store';

// ---------- labels ----------
type Label = '' | 'date' | 'merchant' | 'amount' | 'out' | 'in' | 'type' | 'memo' | 'person';
const LABELS: { id: Label; text: string; hint: string }[] = [
  { id: '', text: '— ignore —', hint: '' },
  { id: 'date', text: 'Date', hint: 'When the transaction happened (or posted).' },
  { id: 'merchant', text: 'Description / merchant', hint: 'Who was paid or who paid you. Pick it on several columns to join them.' },
  { id: 'amount', text: 'Amount (one column, ±)', hint: 'One column holding both money in and out.' },
  { id: 'out', text: 'Money out (debit)', hint: 'A separate column for withdrawals/purchases.' },
  { id: 'in', text: 'Money in (credit)', hint: 'A separate column for deposits/payments.' },
  { id: 'type', text: 'Transaction type', hint: 'A column like DEBIT / CREDIT / PURCHASE / PAYMENT.' },
  { id: 'memo', text: 'Memo / notes', hint: 'Extra detail; kept for reference.' },
  { id: 'person', text: 'Cardholder', hint: 'Who used the card, on shared cards.' },
];
// Labels one column at most may carry. Description may go on several (joined).
const SINGLE: Label[] = ['date', 'amount', 'out', 'in', 'type', 'memo', 'person'];

type Kind = 'spend' | 'income' | 'transfer' | '';

// ---------- CSV ----------
function sniffDelimiter(headerLine: string): string {
  const counts = [',', ';', '\t', '|'].map((d) => [d, headerLine.split(d).length - 1] as const);
  counts.sort((a, b) => b[1] - a[1]);
  return counts[0][1] > 0 ? counts[0][0] : ',';
}

/** Small RFC-4180 parser (quotes, doubled quotes, CRLF); enough for previews. */
export function parseCsv(text: string, delim: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [], cell = '', q = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (q) {
      if (c === '"') { if (text[i + 1] === '"') { cell += '"'; i++; } else q = false; }
      else cell += c;
    } else if (c === '"' && cell === '') q = true;
    else if (c === delim) { row.push(cell); cell = ''; }
    else if (c === '\n' || c === '\r') {
      if (c === '\r' && text[i + 1] === '\n') i++;
      row.push(cell); cell = '';
      rows.push(row); row = [];
    } else cell += c;
  }
  if (cell !== '' || row.length) { row.push(cell); rows.push(row); }
  return rows.filter((r) => r.some((x) => x.trim() !== ''));
}

// ---------- guesses ----------
function guessLabel(header: string): Label {
  const h = header.toLowerCase();
  if (/card ?holder|card member|cardmember/.test(h)) return 'person';
  if (/account|acct|balance|^\s*id\s*$/.test(h)) return '';
  if (/date|posted|posting/.test(h) && !/update/.test(h)) return 'date';
  if (/withdraw|debit|money out|paid out|charges?$/.test(h)) return 'out';
  if (/deposit|credit(?! card)|money in|paid in|payments?$/.test(h)) return 'in';
  if (/amount|value|sum|cad|usd/.test(h)) return 'amount';
  if (/type|transaction$|category/.test(h)) return 'type';
  if (/memo|note|detail|reference/.test(h)) return 'memo';
  if (/desc|merchant|payee|name|narrative|particulars/.test(h)) return 'merchant';
  return '';
}

/** strptime patterns the engine understands (jiff), with a JS validator each. */
const DATE_FORMATS: { fmt: string; label: string }[] = [
  { fmt: '%m/%d/%Y', label: 'MM/DD/YYYY' }, { fmt: '%d/%m/%Y', label: 'DD/MM/YYYY' },
  { fmt: '%Y-%m-%d', label: 'YYYY-MM-DD' }, { fmt: '%Y/%m/%d', label: 'YYYY/MM/DD' },
  { fmt: '%m-%d-%Y', label: 'MM-DD-YYYY' }, { fmt: '%d-%m-%Y', label: 'DD-MM-YYYY' },
  { fmt: '%d.%m.%Y', label: 'DD.MM.YYYY' }, { fmt: '%Y%m%d', label: 'YYYYMMDD' },
  { fmt: '%m/%d/%y', label: 'MM/DD/YY' }, { fmt: '%d/%m/%y', label: 'DD/MM/YY' },
  { fmt: '%d %b %Y', label: 'DD Mon YYYY' }, { fmt: '%b %d, %Y', label: 'Mon DD, YYYY' },
  { fmt: '%d-%b-%Y', label: 'DD-Mon-YYYY' },
];
const MON = ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec'];

/** Same rule as the engine: a trailing clock time ("12:00:00 AM", "T08:15Z") is ignored. */
const TIME_SUFFIX = /^(.*?\d)[ T]+\d{1,2}:\d{2}(?::\d{2}(?:\.\d+)?)?\s*(?:[ap]\.?m\.?)?\s*(?:z|[+-]\d{2}:?\d{2})?$/i;
const dateOnly = (v: string) => { const t = v.trim(); return TIME_SUFFIX.exec(t)?.[1] ?? t; };

function dateFits(fmt: string, v: string): boolean {
  const parts: string[] = [];
  const re = fmt.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/%([YymdbB])/g, (_, k: string) => {
    parts.push(k);
    return k === 'Y' ? '(\\d{4})' : k === 'y' ? '(\\d{2})' : k === 'b' ? '([A-Za-z]{3})'
      : fmt === '%Y%m%d' ? '(\\d{2})' : '(\\d{1,2})';
  });
  const m = new RegExp(`^${re}$`).exec(dateOnly(v));
  if (!m) return false;
  let mo = 0, d = 0;
  parts.forEach((k, i) => {
    const s = m[i + 1];
    if (k === 'm') mo = +s;
    if (k === 'd') d = +s;
    if (k === 'b') mo = MON.indexOf(s.toLowerCase()) + 1;
  });
  return mo >= 1 && mo <= 12 && d >= 1 && d <= 31;
}

/** Engine formats for a pattern: zero-padded plus a non-padded variant. */
function engineFormats(fmt: string): string[] {
  const loose = fmt.replace('%m', '%-m').replace('%d', '%-d');
  return fmt === '%Y%m%d' || loose === fmt ? [fmt] : [fmt, loose];
}

const parseNum = (s: string) => {
  const t = s.replace(/[\s$€£,]/g, '').replace(/^\((.*)\)$/, '-$1');
  return t === '' ? NaN : Number(t);
};

// ---------- TOML ----------
const tq = (s: string) => JSON.stringify(s); // JSON strings are valid TOML basic strings
const reEscape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Account types; the format is named `<bank>_<type>` (e.g. first_bank_mastercard). */
const ACCOUNT_TYPES: { v: string; t: string; kind: 'credit_card' | 'debit' }[] = [
  { v: 'mastercard', t: 'Mastercard', kind: 'credit_card' },
  { v: 'visa', t: 'Visa', kind: 'credit_card' },
  { v: 'amex', t: 'American Express', kind: 'credit_card' },
  { v: 'credit', t: 'Other credit card', kind: 'credit_card' },
  { v: 'debit', t: 'Debit / chequing account', kind: 'debit' },
  { v: 'savings', t: 'Savings account', kind: 'debit' },
];
const slugify = (s: string) => s.toLowerCase().normalize('NFKD').replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '').slice(0, 40);
const idsIn = (toml: string) => [...toml.matchAll(/^\s*id\s*=\s*["']([^"']+)["']/gm)].map((x) => x[1]);

interface Mapping {
  bank: string; acctType: string;
  id: string; headers: string[]; labels: Label[]; dateFmt: string; delim: string;
  outPositive: boolean; accountKind: 'credit_card' | 'debit'; currency: string;
  typeMap: Record<string, Kind>; posKind: Kind; negKind: Kind; idFromFilename: boolean;
}

function col(m: Mapping, l: Label): string | undefined {
  const i = m.labels.indexOf(l);
  return i >= 0 ? m.headers[i] : undefined;
}

export function profileToml(m: Mapping): string {
  const L: string[] = [];
  L.push('', `# ---- Added by the column mapper (${new Date().toISOString().slice(0, 10)}) ----`,
    '[[profile]]', `id = ${tq(m.id)}`, '[profile.match]',
    `header_first_cell = ${tq(reEscape(m.headers[0].trim()))}`,
    `header_contains   = [${m.headers.filter((x) => x.trim()).map(tq).join(', ')}]`);
  if (m.delim !== ',') L.push('[profile.layout]', `delimiter = ${tq(m.delim)}`);
  L.push('[profile.date]', `column  = ${tq(col(m, 'date')!)}`,
    `formats = [${engineFormats(m.dateFmt).map(tq).join(', ')}]`);
  const out = col(m, 'out'), inn = col(m, 'in');
  if (col(m, 'amount')) {
    L.push('[profile.amount]', `column = ${tq(col(m, 'amount')!)}`, `sign   = ${tq(m.outPositive ? 'outflow_positive' : 'signed')}`);
  } else {
    L.push('[profile.amount]', `column        = ${tq(out!)}`, 'sign          = "separate_debit_credit"',
      `debit_column  = ${tq(out!)}`, `credit_column = ${tq(inn!)}`);
  }
  const desc = m.headers.filter((_, i) => m.labels[i] === 'merchant');
  L.push('[profile.merchant]', `column = ${tq(desc[0])}`);
  if (desc.length > 1) L.push(`extra_columns = [${desc.slice(1).map(tq).join(', ')}]`);
  if (col(m, 'memo')) L.push('[profile.memo]', `column = ${tq(col(m, 'memo')!)}`);
  if (col(m, 'person')) L.push('[profile.person]', `column = ${tq(col(m, 'person')!)}`);
  L.push('[profile.type]');
  if (col(m, 'type')) {
    L.push(`column = ${tq(col(m, 'type')!)}`);
    const pairs = Object.entries(m.typeMap).filter(([, k]) => k);
    if (pairs.length) L.push(`map    = { ${pairs.map(([v, k]) => `${tq(v)} = ${tq(k)}`).join(', ')} }`);
  }
  if (m.negKind && m.posKind) L.push('[profile.type.by_sign]', `negative = ${tq(m.negKind)}`, `positive = ${tq(m.posKind)}`);
  L.push('[profile.account]', `kind     = ${tq(m.accountKind)}`, `currency = ${tq(m.currency)}`);
  if (m.idFromFilename) L.push(`id_from_filename = '(\\d{4})'`);
  return L.join('\n') + '\n';
}

// ---------- UI ----------
function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) { if (k === 'class') e.className = v; else e.setAttribute(k, v); }
  e.append(...kids);
  return e;
}
function select<T extends string>(opts: { v: T; t: string }[], val: T, on: (v: T) => void): HTMLSelectElement {
  const s = h('select', { class: 'map-sel' });
  for (const o of opts) { const op = h('option', { value: o.v }, o.t); if (o.v === val) op.selected = true; s.append(op); }
  s.addEventListener('change', () => on(s.value as T));
  return s;
}

const KIND_OPTS: { v: Kind; t: string }[] = [
  { v: 'spend', t: 'Spending' }, { v: 'income', t: 'Income' }, { v: 'transfer', t: 'Transfer / card payment' },
  { v: '', t: 'Decide by amount sign' },
];

/**
 * Open the mapper for one file of `doc`. Resolves `true` when a format was saved
 * (the caller persists `doc`), `false` when cancelled.
 */
export function openMapper(doc: ProjectDoc, file: ProjectFile): Promise<boolean> {
  return new Promise((resolve) => {
    const text = new TextDecoder().decode(file.bytes).replace(/^\uFEFF/, '');
    const firstLine = text.split(/\r?\n/, 1)[0] ?? '';
    const delim = sniffDelimiter(firstLine);
    const rows = parseCsv(text, delim);
    const headers = rows[0] ?? [];
    const body = rows.slice(1);
    const preview = body.slice(0, 6);

    // Initial guesses (each single-use label goes to the first column that wants it).
    const labels: Label[] = [];
    for (const hd of headers) {
      const g = guessLabel(hd);
      labels.push(g && SINGLE.includes(g) && labels.includes(g) ? '' : g);
    }
    // Every file needs a description; a lone "Details"/"Memo" column is it.
    if (!labels.includes('merchant') && labels.includes('memo')) labels[labels.indexOf('memo')] = 'merchant';
    if (labels.includes('amount') && (labels.includes('out') || labels.includes('in'))) {
      for (let i = 0; i < labels.length; i++) if (labels[i] === 'out' || labels[i] === 'in') labels[i] = '';
    }
    const fn = file.name.toLowerCase();
    const acctType = /master|\bmc\b/.test(fn) ? 'mastercard' : /visa/.test(fn) ? 'visa' : /amex|american/.test(fn) ? 'amex'
      : /saving/.test(fn) ? 'savings' : /credit|card/.test(fn) ? 'credit' : 'debit';
    const m: Mapping = {
      bank: '', acctType, id: '', headers, labels, dateFmt: '', delim,
      outPositive: false, accountKind: ACCOUNT_TYPES.find((a) => a.v === acctType)!.kind,
      currency: 'CAD', typeMap: {}, negKind: 'spend', posKind: 'income', idFromFilename: /\d{4}/.test(file.name),
    };
    m.posKind = m.accountKind === 'credit_card' ? 'transfer' : 'income';
    // Names already taken (the project's own formats and the built-ins).
    const projectProfiles = projectConfig(doc)['profiles.toml'] ?? '';
    const taken = new Set([...idsIn(projectProfiles), ...idsIn(defaultConfig()['profiles.toml'] ?? '')]);
    const nameFor = () => {
      const base = `${slugify(m.bank)}_${m.acctType}`;
      let id = base;
      for (let n = 2; taken.has(id); n++) id = `${base}_${n}`;
      return id;
    };

    const overlay = h('div', { class: 'map-overlay', role: 'dialog', 'aria-modal': 'true' });
    const box = h('div', { class: 'map-box panel' });
    overlay.append(box);
    document.body.append(overlay);
    const close = (saved: boolean) => { overlay.remove(); document.removeEventListener('keydown', onKey); resolve(saved); };
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') close(false); };
    document.addEventListener('keydown', onKey);

    const colValues = (l: Label) => { const i = m.labels.indexOf(l); return i < 0 ? [] : body.map((r) => r[i] ?? '').filter((v) => v.trim()); };

    let previewTimer = 0;
    // Created once so typing survives redraws (focus and caret are restored).
    const bankIn = h('input', { type: 'text', placeholder: 'e.g. First Bank', maxlength: '40', class: 'map-bank' }) as HTMLInputElement;
    let bankTimer = 0;
    bankIn.addEventListener('input', () => {
      clearTimeout(bankTimer);
      bankTimer = window.setTimeout(() => {
        const caret = bankIn.selectionStart;
        m.bank = bankIn.value; draw();
        bankIn.focus(); if (caret != null) bankIn.setSelectionRange(caret, caret);
      }, 250);
    });
    const draw = () => {
      // Date formats that fit every sampled value of the date column.
      const dates = colValues('date').slice(0, 200);
      const fits = dates.length ? DATE_FORMATS.filter((f) => dates.every((v) => dateFits(f.fmt, v))) : [];
      if (!fits.some((f) => f.fmt === m.dateFmt)) m.dateFmt = fits[0]?.fmt ?? '';
      const typeVals = [...new Set(colValues('type').map((v) => v.trim()))].slice(0, 30);
      for (const v of Object.keys(m.typeMap)) if (!typeVals.includes(v)) delete m.typeMap[v];

      // Column table with a label picker above each column.
      const head = h('tr', {}, ...headers.map((hd, i) => h('th', {},
        select(LABELS.map((l) => ({ v: l.id, t: l.text })), m.labels[i], (v) => {
          if (v && SINGLE.includes(v)) m.labels = m.labels.map((x) => (x === v ? '' : x));
          if (v === 'amount') m.labels = m.labels.map((x) => (x === 'out' || x === 'in' ? '' : x));
          if (v === 'out' || v === 'in') m.labels = m.labels.map((x) => (x === 'amount' ? '' : x));
          m.labels[i] = v; draw();
        }),
        h('div', { class: 'map-colname' }, hd || `(column ${i + 1})`))));
      const tbl = h('table', { class: 'map-table' }, h('thead', {}, head),
        h('tbody', {}, ...preview.map((r) => h('tr', {}, ...headers.map((_, i) =>
          h('td', { class: m.labels[i] ? 'on' : 'off' }, r[i] ?? '')))))) ;

      m.id = m.bank.trim() ? nameFor() : '';
      // Problems that block saving.
      const problems: string[] = [];
      if (!m.bank.trim()) problems.push('Enter the bank name (it names the format).');
      if (!m.labels.includes('date')) problems.push('Label the Date column.');
      if (!m.labels.includes('merchant')) problems.push('Label the Description / merchant column (or several — they’re joined).');
      const hasAmt = m.labels.includes('amount');
      if (!hasAmt && !(m.labels.includes('out') && m.labels.includes('in'))) problems.push('Label an Amount column, or both Money out and Money in.');
      if (m.labels.includes('date') && !m.dateFmt) problems.push('The Date column doesn’t look like a date format I know.');

      // Settings.
      const settings = h('div', { class: 'map-settings' });
      const row = (label: string, ...ctl: (Node | string)[]) => settings.append(h('label', { class: 'map-row' }, h('span', {}, label), ...ctl));
      if (m.labels.includes('date')) {
        row('Date format', fits.length
          ? select(fits.map((f) => ({ v: f.fmt, t: `${f.label}  (e.g. ${dates[0]})` })), m.dateFmt, (v) => { m.dateFmt = v; draw(); })
          : h('span', { class: 'neg' }, 'not recognised'),
          ...(dates.some((v) => dateOnly(v) !== v.trim()) ? [h('span', { class: 'muted' }, ' the time part is ignored')] : []));
      }
      if (hasAmt) {
        const sample = colValues('amount').slice(0, 200).map(parseNum).filter((n) => !Number.isNaN(n));
        const neg = sample.filter((n) => n < 0).length;
        row('Money spent shows as', select([{ v: 'neg', t: 'Negative  (−12.34)' }, { v: 'pos', t: 'Positive  (12.34)' }],
          m.outPositive ? 'pos' : 'neg', (v) => { m.outPositive = v === 'pos'; draw(); }),
          h('span', { class: 'muted' }, ` ${neg} of ${sample.length} values are negative`));
      }
      row('Bank', bankIn);
      row('Account type', select(ACCOUNT_TYPES.map((a) => ({ v: a.v, t: a.t })), m.acctType, (v) => {
        m.acctType = v;
        m.accountKind = ACCOUNT_TYPES.find((a) => a.v === v)!.kind;
        m.posKind = m.accountKind === 'credit_card' ? 'transfer' : 'income';
        draw();
      }), h('span', { class: 'muted' }, m.id ? ` format name: ` : ''), ...(m.id ? [h('code', {}, m.id)] : []));
      const cur = h('input', { type: 'text', value: m.currency, maxlength: '3', class: 'map-cur' }) as HTMLInputElement;
      cur.addEventListener('change', () => { m.currency = cur.value.trim().toUpperCase() || 'CAD'; draw(); });
      row('Currency', cur);
      if (typeVals.length) {
        settings.append(h('div', { class: 'map-sub' }, 'What each transaction type means:'));
        for (const v of typeVals) {
          row(`“${v}”`, select(KIND_OPTS, m.typeMap[v] ?? '', (k) => { m.typeMap[v] = k; draw(); }));
        }
      }
      settings.append(h('div', { class: 'map-sub' }, typeVals.length ? 'Rows whose type is “decide by amount sign”:' : 'How to classify rows (there’s no type column):'));
      const kindPick = (k: Kind, on: (v: Kind) => void) => select(KIND_OPTS.filter((o) => o.v), k, on);
      row('Money out is', kindPick(m.negKind, (v) => { m.negKind = v; draw(); }));
      row('Money in is', kindPick(m.posKind, (v) => { m.posKind = v; draw(); }),
        h('span', { class: 'muted' }, m.accountKind === 'credit_card' ? ' (on a card this is usually your payment to it)' : ' (on a bank account this is usually pay)'));

      // Live check with the real parser.
      const result = h('div', { class: 'map-result muted' }, problems.length ? '' : 'Checking…');
      const saveBtn = h('button', { class: 'btn active', type: 'button' }, 'Save format') as HTMLButtonElement;
      saveBtn.disabled = true;
      let pending: { profiles: string } | null = null;
      if (!problems.length) {
        clearTimeout(previewTimer);
        previewTimer = window.setTimeout(() => {
          const cfg = projectConfig(doc);
          // Only the project's own formats live in profiles.toml; built-ins are always added by the engine.
          const base = cfg['profiles.toml'] ?? '# Your own statement formats. Built-in formats are always available too;\n# a format here with the same id replaces the built-in one.\nschema_version = 1\n';
          const profiles = base.replace(/\s*$/, '\n') + profileToml(m);
          let chk: FileCheck | undefined;
          try { chk = checkFiles([{ name: file.name, bytes: file.bytes }], { ...cfg, 'profiles.toml': profiles })[0]; }
          catch (e) { result.className = 'map-result neg'; result.textContent = (e as Error).message; return; }
          if (!chk || chk.error) { result.className = 'map-result neg'; result.textContent = `Not readable yet: ${chk?.error ?? 'unknown error'}`; return; }
          if (chk.profile !== m.id) {
            result.className = 'map-result neg';
            result.textContent = `This file is already matched by the “${chk.profile}” format, so a new one wouldn’t be used.`;
            return;
          }
          pending = { profiles };
          saveBtn.disabled = false;
          result.className = 'map-result';
          result.replaceChildren(
            h('div', {}, h('b', { class: 'pos' }, `✓ Reads ${chk.rows} row${chk.rows === 1 ? '' : 's'}.`), ' First rows as the analyzer will see them:'),
            h('table', { class: 'map-table map-parsed' },
              h('thead', {}, h('tr', {}, ...['Date', 'Amount', 'Merchant', 'Kind'].map((t) => h('th', {}, t)))),
              h('tbody', {}, ...chk.sample.map((s) => h('tr', {},
                h('td', {}, s.date), h('td', { class: `num ${s.amount.startsWith('-') ? 'neg' : 'pos'}` }, s.amount),
                h('td', {}, s.merchant), h('td', {}, s.kind))))));
        }, 150);
      }
      saveBtn.addEventListener('click', () => {
        if (!pending) return;
        upsertFile(doc, 'profiles.toml', new TextEncoder().encode(pending.profiles));
        close(true);
      });

      box.replaceChildren(
        h('div', { class: 'map-head' }, h('h3', {}, `Map columns — ${file.name}`),
          h('button', { class: 'btn small', type: 'button', title: 'Close (Esc)' }, '✕')),
        h('p', { class: 'muted' }, 'No known format matches this file. Pick what each column holds. The result is saved as a new format in this project’s profiles.toml — part of the config bundle — so other files with the same columns are recognised too, here and wherever you upload that bundle.'),
        h('div', { class: 'map-scroll' }, tbl),
        settings,
        ...(problems.length ? [h('div', { class: 'map-problems' }, ...problems.map((p) => h('div', {}, '• ' + p)))] : []),
        result,
        h('div', { class: 'map-actions' }, h('button', { class: 'btn', type: 'button' }, 'Cancel'), saveBtn),
      );
      (box.querySelector('.map-head button') as HTMLElement).addEventListener('click', () => close(false));
      (box.querySelector('.map-actions .btn:not(.active)') as HTMLElement).addEventListener('click', () => close(false));
    };
    draw();
  });
}
