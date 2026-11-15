// "Picks up…": decide which statement rows an account owns, by example.
// The user ticks the files that belong to the account; we propose the most
// durable rule that picks up exactly those files (a number the format reads
// from the file name, the start of the file name, the format itself…), in plain
// words, and preview — with the real engine — what the rule would pick up.
// Nobody has to write a regex; "pattern" stays available for those who want it.
import { checkFiles, editPeople, type FileCheck, type PeopleAccount, type PeopleOp } from './engine';

// ---------- pure helpers (unit-tested) ----------

export type NameMode = 'none' | 'starts' | 'contains' | 'exact' | 'pattern';
export interface Criteria {
  profile: string | null;
  fileId: string | null;
  cardholder: string | null;
  nameMode: NameMode;
  /** starts/contains: the literal text; pattern: the regex; exact: unused. */
  nameValue: string;
}
export interface FileInfo { name: string; profile: string | null; fileId: string | null }

/** Escape like Rust's `regex::escape`. */
export const reEscape = (s: string) => s.replace(/[\\.+*?()|[\]{}^$#&\-~]/g, '\\$&');
const unescape = (s: string) => s.replace(/\\(.)/g, '$1');
/** Is this regex body a plain literal (only escaped metacharacters)? */
const isLiteral = (body: string) => !/[.+*?()|[\]{}^$]/.test(body.replace(/\\./g, ''));
/** Split on `|` that isn't escaped. */
const splitAlt = (s: string) => s.split(/(?<!\\)\|/);

/** Regex for a file-name criterion, or null for none. */
export function filenameRegex(mode: NameMode, value: string, exactNames: string[]): string | null {
  const v = value.trim();
  switch (mode) {
    case 'starts': return v ? `(?i)^${reEscape(v)}` : null;
    case 'contains': return v ? `(?i)${reEscape(v)}` : null;
    case 'exact': return exactNames.length ? `(?i)^(?:${exactNames.map(reEscape).join('|')})$` : null;
    case 'pattern': return v || null;
    default: return null;
  }
}

/** Read a file-name regex back into a mode the guide can show. */
export function parseFilenameRegex(re: string | null | undefined): { mode: NameMode; value: string; names: string[] } {
  if (!re) return { mode: 'none', value: '', names: [] };
  let m = re.match(/^\(\?i\)\^\(\?:(.*)\)\$$/);
  if (m && splitAlt(m[1]).every(isLiteral)) return { mode: 'exact', value: '', names: splitAlt(m[1]).map(unescape) };
  m = re.match(/^\(\?i\)\^(.*)\$$/);
  if (m && isLiteral(m[1])) return { mode: 'exact', value: '', names: [unescape(m[1])] };
  m = re.match(/^\(\?i\)\^(.*)$/);
  if (m && isLiteral(m[1])) return { mode: 'starts', value: unescape(m[1]), names: [] };
  m = re.match(/^\(\?i\)(.*)$/);
  if (m && isLiteral(m[1])) return { mode: 'contains', value: unescape(m[1]), names: [] };
  return { mode: 'pattern', value: re, names: [] };
}

/** Plain-words description of a file-name regex. */
export function describeFilename(re: string): string {
  const p = parseFilenameRegex(re);
  switch (p.mode) {
    case 'starts': return `file name starts with “${p.value}”`;
    case 'contains': return `file name contains “${p.value}”`;
    case 'exact': return p.names.length === 1 ? `file “${p.names[0]}”` : `files ${p.names.map((n) => `“${n}”`).join(', ')}`;
    default: return `file name matches /${re}/`;
  }
}

/** "Card 7012 (2).CSV" → "Card 7012": no extension, no "(2)" download counter. */
export const stemOf = (name: string) => name.replace(/\.[a-z0-9]{1,5}$/i, '').replace(/\s*\(\d+\)$/, '').trim();
// Trailing words that change between exports of one account.
// Dates, years and download counters go; a 4-digit card/account number stays.
const VOLATILE = /[\s_.\-]+(?:\d{5,}|\d{1,2}|(?:19|20)\d{2}(?:-?\d{2}(?:-?\d{2})?)?|all|recent|export|full|history|transactions?|statements?|activity)$/i;
const SEP = /[\s_.\-]/;

/** The stable start shared by the ticked files' names ("" when there isn't one). */
export function commonPrefix(names: string[]): string {
  const stems = names.map(stemOf);
  if (!stems.length) return '';
  let p = stems[0];
  for (const s of stems.slice(1)) {
    let i = 0;
    while (i < p.length && i < s.length && p[i].toLowerCase() === s[i].toLowerCase()) i++;
    p = p.slice(0, i);
  }
  // Don't stop mid-word: every stem must end or break right after the prefix.
  if (stems.some((s) => s.length > p.length && !SEP.test(s[p.length]) && !SEP.test(p[p.length - 1] || ''))) {
    const cut = Math.max(...[...p.matchAll(/[\s_.\-]/g)].map((m) => m.index ?? -1), -1);
    p = cut > 0 ? p.slice(0, cut) : '';
  }
  for (let prev = ''; prev !== p;) { prev = p; p = p.replace(VOLATILE, ''); }
  p = p.replace(/[\s_.\-]+$/, '');
  return p.length >= 3 ? p : '';
}

/** Longest substring (≥4 chars) every ticked name contains, case-insensitively. */
function commonSubstring(names: string[]): string {
  const stems = names.map((n) => stemOf(n).toLowerCase());
  const base = [...stems].sort((a, b) => a.length - b.length)[0] ?? '';
  for (let len = Math.min(base.length, 40); len >= 4; len--) {
    for (let i = 0; i + len <= base.length; i++) {
      const sub = base.slice(i, i + len);
      if (sub.trim().length >= 4 && stems.every((s) => s.includes(sub))) {
        // Return it in the first name's own casing.
        const at = stemOf(names[0]).toLowerCase().indexOf(sub);
        return stemOf(names[0]).slice(at, at + len).trim();
      }
    }
  }
  return '';
}

/** Does a file satisfy the file-level criteria (cardholder is per row, so it's not judged here)? */
export function fileMatches(f: FileInfo, c: Criteria, exactNames: string[]): boolean {
  if (c.profile && f.profile !== c.profile) return false;
  if (c.fileId && f.fileId !== c.fileId) return false;
  const n = f.name.toLowerCase(), v = c.nameValue.trim().toLowerCase();
  if (c.nameMode === 'starts') return n.startsWith(v);
  if (c.nameMode === 'contains') return n.includes(v);
  if (c.nameMode === 'exact') return exactNames.some((x) => x.toLowerCase() === n);
  if (c.nameMode === 'pattern') { try { return new RegExp(c.nameValue.replace(/^\(\?i\)/, ''), 'i').test(f.name); } catch { return false; } }
  return true;
}

/**
 * The most durable criteria that pick up exactly the ticked files, preferring
 * (1) the number the format reads from the file name, (2) the start of the file
 * name, (3) the format alone, (4) a word in the file name, (5) the exact names.
 * The format is kept alongside whenever all ticked files share one.
 */
export function propose(files: FileInfo[], ticked: Set<string>, cardholder: string | null = null): Criteria {
  const T = files.filter((f) => ticked.has(f.name)), O = files.filter((f) => !ticked.has(f.name));
  const names = T.map((f) => f.name);
  const profiles = new Set(T.map((f) => f.profile));
  const profile = profiles.size === 1 ? [...profiles][0] : null;
  const ids = new Set(T.map((f) => f.fileId));
  const fileId = ids.size === 1 ? [...ids][0] : null;
  const base: Criteria = { profile, fileId: null, cardholder, nameMode: 'none', nameValue: '' };
  const prefix = commonPrefix(names), word = commonSubstring(names);
  const cands: Criteria[] = [];
  // The format's captured number is the most durable signal — when it's the
  // card's trailing digits. A leading group (e.g. a card network prefix shared
  // by several cards) is weaker than the name's start.
  const trailing = !!fileId && T.every((f) => stemOf(f.name).endsWith(fileId));
  if (fileId && profile && trailing) cands.push({ ...base, fileId });
  if (prefix) cands.push({ ...base, nameMode: 'starts', nameValue: prefix });
  if (fileId && profile && !trailing) cands.push({ ...base, fileId });
  if (profile) cands.push(base);
  if (word && word.toLowerCase() !== prefix.toLowerCase()) cands.push({ ...base, nameMode: 'contains', nameValue: word });
  const exact: Criteria = { ...base, nameMode: 'exact' };
  const separates = (c: Criteria) => T.every((f) => fileMatches(f, c, names)) && !O.some((f) => fileMatches(f, c, names));
  return cands.find(separates) ?? exact;
}

/** Criteria → the engine's `set_match` fields. */
export function matchFields(c: Criteria, exactNames: string[]) {
  return {
    filename: filenameRegex(c.nameMode, c.nameValue, exactNames),
    profile: c.profile, cardholder: c.cardholder, card_last4: c.fileId,
  };
}

// ---------- the dialog ----------

function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) { if (k === 'class') e.className = v; else e.setAttribute(k, v); }
  e.append(...kids);
  return e;
}
const checkbox = (on: boolean, label: string) => {
  const c = h('input', { type: 'checkbox', 'aria-label': label }) as HTMLInputElement;
  c.checked = on;
  return c;
};
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? '' : 's'}`;
export const formatLabel = (p: string | null) => p ? p.replace(/_/g, ' ') : 'unrecognised';

export interface GuideOpts {
  /** people.toml as it stands in the editor. */
  text: string;
  files: { name: string; bytes: Uint8Array }[];
  /** The project's other config files (people.toml is replaced by the candidate). */
  config: Record<string, string>;
  /** Checks for `text` (who gets what now). */
  checks: FileCheck[];
  account: string;
  /** Set when the account doesn't exist yet: it's created with these. */
  create?: { person: string; kind: string };
  current?: PeopleAccount;
  /** Files to tick on top of the ones the account already picks up. */
  tick?: string[];
}

/** Resolves the ops to apply, or null when cancelled. */
export function openMatchGuide(o: GuideOpts): Promise<PeopleOp[] | null> {
  return new Promise((resolve) => {
    const usable = o.checks.filter((c) => c.rows > 0);
    const infos: FileInfo[] = usable.map((c) => ({ name: c.file, profile: c.profile, fileId: c.file_id }));
    const ticked = new Set<string>([...usable.filter((c) => (c.accounts[o.account] || 0) > 0).map((c) => c.file), ...(o.tick ?? [])]);
    const cur = o.current && !o.current.external ? o.current : null;

    let crit: Criteria;
    const holdersOf = () => [...new Set(usable.filter((c) => ticked.has(c.file)).flatMap((c) => Object.keys(c.cardholders || {})))].sort();
    const keepHolder = () => (cur?.cardholder && holdersOf().some((x) => x.toLowerCase() === cur.cardholder!.toLowerCase()) ? cur.cardholder : null);
    if (cur && !o.tick?.length) {
      const fn = parseFilenameRegex(cur.filename);
      crit = { profile: cur.profile ?? null, fileId: cur.card_last4 ?? null, cardholder: cur.cardholder ?? null, nameMode: fn.mode, nameValue: fn.value };
      if (fn.mode === 'exact') fn.names.forEach((n) => ticked.add(n));
    } else crit = propose(infos, ticked, keepHolder());
    const exactNames = () => [...ticked];

    const overlay = h('div', { class: 'map-overlay mg-overlay', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Which statements belong to this account' });
    const box = h('div', { class: 'map-box panel mg-box' });
    overlay.append(box);
    document.body.append(overlay);
    const close = (ops: PeopleOp[] | null) => {
      overlay.remove(); document.removeEventListener('keydown', onKey, true); resolve(ops);
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.stopPropagation(); close(null); } };
    document.addEventListener('keydown', onKey, true);

    const ops = (): PeopleOp[] => [
      ...(o.create ? [{ op: 'add_account' as const, person: o.create.person, id: o.account, kind: o.create.kind }] : []),
      { op: 'set_match', account: o.account, ...matchFields(crit, exactNames()) },
    ];

    // ---- static parts ----
    const fileRows = h('tbody', {});
    const critBox = h('div', { class: 'mg-crit' });
    const preview = h('div', { class: 'mg-preview' });
    const verdict = h('div', { class: 'mg-verdict' });
    const applyBtn = h('button', { type: 'button', class: 'btn active' }, 'Apply') as HTMLButtonElement;
    applyBtn.addEventListener('click', () => close(ops()));
    const footer = h('div', { class: 'map-actions' });
    if (o.current && !o.current.external) {
      const ext = h('button', { type: 'button', class: 'btn mg-ext', title: 'For an account you have no statements for: it stays a transfer destination' }, 'No statements — make it external') as HTMLButtonElement;
      ext.addEventListener('click', () => close([{ op: 'make_external', account: o.account }]));
      footer.append(ext);
    }
    const cancel = h('button', { type: 'button', class: 'btn' }, 'Cancel');
    cancel.addEventListener('click', () => close(null));
    if (!footer.childElementCount) footer.append(h('span', { class: 'pe-count' }));
    footer.append(cancel, applyBtn);

    box.append(
      h('div', { class: 'map-head' }, h('h3', {}, `Which statements are “${o.account}”?`)),
      h('p', { class: 'muted' }, 'Tick the files that belong to this account. A rule that recognises them — and later downloads of the same account — is worked out for you; below it you see exactly what it would pick up.'),
      h('table', { class: 'map-table mg-files' },
        h('thead', {}, h('tr', {}, h('th', {}, ''), h('th', {}, 'File'), h('th', {}, 'Format'), h('th', { class: 'num' }, 'Rows'), h('th', {}, 'Goes to now'))),
        fileRows),
      h('h4', {}, 'Recognise them by'), critBox,
      h('h4', {}, 'What this picks up'), verdict, preview, footer,
    );

    const nowGoesTo = (c: FileCheck) => Object.entries(c.accounts).map(([a, n]) => (a === 'unassigned' ? `no account (${n})` : `${a} (${n})`)).join(', ');
    for (const c of o.checks) {
      const ok = c.rows > 0;
      const cb = checkbox(ticked.has(c.file), `Belongs to ${o.account}: ${c.file}`);
      cb.disabled = !ok;
      cb.addEventListener('change', () => {
        if (cb.checked) ticked.add(c.file); else ticked.delete(c.file);
        crit = propose(infos, ticked, keepHolder());
        drawCrit(); refresh();
      });
      fileRows.append(h('tr', { class: ticked.has(c.file) ? 'on' : '', 'data-file': c.file },
        h('td', {}, cb), h('td', {}, c.file),
        h('td', { class: 'muted' }, ok ? formatLabel(c.profile) : 'can’t be read'),
        h('td', { class: 'num' }, String(c.rows)),
        h('td', { class: 'muted' }, ok ? nowGoesTo(c) : '—')));
    }

    // ---- criteria ----
    function drawCrit() {
      const tk = usable.filter((c) => ticked.has(c.file));
      const profiles = [...new Set(tk.map((c) => c.profile).filter(Boolean))] as string[];
      const ids = [...new Set(tk.map((c) => c.file_id))];
      const rows: HTMLElement[] = [];
      // Format.
      {
        const p = crit.profile ?? (profiles.length === 1 ? profiles[0] : null);
        const cb = checkbox(!!crit.profile, 'Recognise by format');
        cb.disabled = !p;
        cb.addEventListener('change', () => { crit.profile = cb.checked ? p : null; refresh(); });
        rows.push(h('label', { class: 'mg-row' }, cb, ' Format is ', h('b', {}, formatLabel(p)),
          ...(!p ? [h('span', { class: 'muted' }, ' (the ticked files have different formats)')] : [])));
      }
      // Number read from the file name.
      const id = crit.fileId ?? (ids.length === 1 ? ids[0] : null);
      if (id || crit.fileId) {
        const cb = checkbox(!!crit.fileId, 'Recognise by the number in the file name');
        cb.addEventListener('change', () => { crit.fileId = cb.checked ? id : null; refresh(); });
        rows.push(h('label', { class: 'mg-row' }, cb, ' The file name carries the number ', h('b', {}, id ?? ''),
          h('span', { class: 'muted' }, ' (card or account digits)')));
      }
      // File name.
      {
        const cb = checkbox(crit.nameMode !== 'none', 'Recognise by file name');
        const mode = h('select', { class: 'map-sel', 'aria-label': 'How the file name is matched' }) as HTMLSelectElement;
        for (const [v, t] of [['starts', 'starts with'], ['contains', 'contains'], ['exact', 'is one of the ticked files'], ['pattern', 'matches a pattern (advanced)']]) {
          const op = h('option', { value: v }, t); if (crit.nameMode === v) op.selected = true; mode.append(op);
        }
        const val = h('input', { type: 'text', class: 'pe-in mg-name', 'aria-label': 'File name text' }) as HTMLInputElement;
        const sync = () => {
          val.hidden = crit.nameMode === 'exact' || crit.nameMode === 'none';
          val.value = crit.nameValue;
          val.placeholder = crit.nameMode === 'pattern' ? '(?i)^Card \\d{4}' : 'text';
          mode.disabled = crit.nameMode === 'none';
        };
        if (crit.nameMode === 'none') mode.value = 'starts';
        cb.addEventListener('change', () => {
          if (cb.checked) {
            crit.nameMode = mode.value as NameMode;
            if (!crit.nameValue) crit.nameValue = commonPrefix(exactNames()) || stemOf(exactNames()[0] ?? '');
          } else crit.nameMode = 'none';
          sync(); refresh();
        });
        mode.addEventListener('change', () => {
          const prev = crit.nameMode;
          crit.nameMode = mode.value as NameMode;
          if (crit.nameMode === 'pattern' && prev !== 'pattern') crit.nameValue = filenameRegex(prev, crit.nameValue, exactNames()) ?? '';
          else if (prev === 'pattern' && crit.nameMode !== 'pattern') crit.nameValue = commonPrefix(exactNames());
          sync(); refresh();
        });
        let t = 0;
        val.addEventListener('input', () => { crit.nameValue = val.value; clearTimeout(t); t = window.setTimeout(refresh, 250); });
        sync();
        rows.push(h('label', { class: 'mg-row' }, cb, ' File name ', mode, ' ', val));
      }
      // Cardholder (per row).
      const holders = holdersOf();
      if (holders.length || crit.cardholder) {
        const cb = checkbox(!!crit.cardholder, 'Only one cardholder’s rows');
        const sel = h('select', { class: 'map-sel', 'aria-label': 'Cardholder' }) as HTMLSelectElement;
        for (const x of new Set([...holders, ...(crit.cardholder ? [crit.cardholder] : [])])) {
          const op = h('option', { value: x }, x); if (crit.cardholder?.toLowerCase() === x.toLowerCase()) op.selected = true; sel.append(op);
        }
        sel.disabled = !crit.cardholder;
        cb.addEventListener('change', () => { crit.cardholder = cb.checked ? sel.value : null; sel.disabled = !cb.checked; refresh(); });
        sel.addEventListener('change', () => { crit.cardholder = sel.value; refresh(); });
        rows.push(h('label', { class: 'mg-row' }, cb, ' Only rows for cardholder ', sel,
          h('span', { class: 'muted' }, holders.length > 1 ? ' (these files mix cardholders)' : '')));
      }
      const again = h('button', { type: 'button', class: 'btn small' }, '↺ Suggest again');
      again.addEventListener('click', () => { crit = propose(infos, ticked, keepHolder()); drawCrit(); refresh(); });
      critBox.replaceChildren(...rows, h('div', { class: 'mg-row' }, again,
        h('span', { class: 'muted' }, ' All checked conditions must hold. The suggestion is the most durable rule that picks up exactly the ticked files.')));
    }

    // ---- live preview (the real engine on the edited people.toml) ----
    function refresh() {
      for (const tr of fileRows.querySelectorAll<HTMLElement>('tr[data-file]')) tr.classList.toggle('on', ticked.has(tr.dataset.file!));
      let res: FileCheck[] = [];
      let err = '';
      try {
        const cand = editPeople(o.text, ops());
        res = checkFiles(o.files, { ...o.config, 'people.toml': cand });
      } catch (e) { err = (e as Error).message; }
      applyBtn.disabled = !!err || !ticked.size;
      if (err) { verdict.className = 'mg-verdict bad'; verdict.textContent = err; preview.replaceChildren(); return; }
      const items: HTMLElement[] = [];
      let missing = 0, partial = 0, extra = 0;
      for (const c of res) {
        if (c.rows === 0) continue;
        const got = c.accounts[o.account] || 0, lost = c.also[o.account] || 0;
        const others = Object.keys(c.accounts).filter((a) => a !== o.account && a !== 'unassigned');
        const want = ticked.has(c.file);
        let icon = '', cls = '', msg = '';
        if (want && got === c.rows) { icon = '✓'; cls = 'ok'; msg = `all ${plural(c.rows, 'row')}`; }
        else if (want && got > 0) {
          icon = '⚠'; cls = 'warn'; partial++;
          msg = `${got} of ${c.rows} rows` + (lost ? ` — ${lost} fit but go to ${others.join(', ')}, which is listed earlier` : ' — the rest don’t meet the conditions (e.g. another cardholder)');
        } else if (want) {
          icon = '✗'; cls = 'bad'; missing++;
          msg = lost ? `fits, but ${others.join(', ')} is listed earlier and takes its rows — narrow that account’s rule` : 'not picked up';
        } else if (got > 0) { icon = '⚠'; cls = 'warn'; extra++; msg = `not ticked, but ${plural(got, 'row')} would be picked up`; }
        else continue;
        items.push(h('div', { class: `mg-item ${cls}` }, h('span', { class: 'mg-ic' }, icon), h('b', {}, c.file), h('span', { class: 'muted' }, ` · ${msg}`)));
      }
      preview.replaceChildren(...items);
      if (!ticked.size) { verdict.className = 'mg-verdict warn'; verdict.textContent = 'Tick at least one file.'; }
      else if (!missing && !partial && !extra) { verdict.className = 'mg-verdict ok'; verdict.textContent = `✓ Picks up exactly the ${plural(ticked.size, 'ticked file')}, nothing else.`; }
      else {
        verdict.className = 'mg-verdict warn';
        verdict.textContent = [missing && `${plural(missing, 'ticked file')} not picked up`, partial && `${plural(partial, 'file')} only partly`, extra && `${plural(extra, 'other file')} picked up too`].filter(Boolean).join(' · ') + ' — you can still apply it.';
      }
    }

    drawCrit();
    refresh();
    (box.querySelector('input:not([disabled])') as HTMLElement | null)?.focus();
  });
}
