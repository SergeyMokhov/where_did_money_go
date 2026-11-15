// "People & accounts": a structured editor for the project's people.toml.
// Rename people and accounts, move an account to another owner, add people and
// external accounts, and give statement files that belong to no account an
// owner. Every edit is applied by the engine (comments in the file survive) and
// re-checked against the project's statements before it can be saved. The file
// is part of the config bundle, so edits export/import with it.
import {
  checkFiles, editPeople, parsePeople, peopleText, projectConfig,
  type FileCheck, type PeopleDoc, type PeopleOp,
} from './engine';
import { upsertFile, type ProjectDoc } from './store';
import { describeFilename, formatLabel, openMatchGuide } from './matchguide';

const KINDS: { v: string; t: string }[] = [
  { v: 'debit', t: 'Chequing / debit' }, { v: 'savings', t: 'Savings' },
  { v: 'credit_card', t: 'Credit card' }, { v: 'investment', t: 'Investment' }, { v: 'retirement', t: 'Retirement' },
  { v: 'loan', t: 'Loan / line of credit' },
];
/** Kind options, keeping a custom kind already in the file. */
const kindOpts = (cur: string) => (KINDS.some((k) => k.v === cur) || !cur ? KINDS : [...KINDS, { v: cur, t: cur }]);
const UNASSIGNED = 'unassigned';

function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) { if (k === 'class') e.className = v; else e.setAttribute(k, v); }
  e.append(...kids);
  return e;
}
function select(opts: { v: string; t: string }[], val: string, cls = 'map-sel'): HTMLSelectElement {
  const s = h('select', { class: cls });
  for (const o of opts) { const op = h('option', { value: o.v }, o.t); if (o.v === val) op.selected = true; s.append(op); }
  return s;
}
function button(text: string, onClick: () => void, cls = 'btn small', title = ''): HTMLButtonElement {
  const b = h('button', { type: 'button', class: cls, ...(title ? { title } : {}) }, text) as HTMLButtonElement;
  b.addEventListener('click', onClick);
  return b;
}

/** "Chq_Main_all.CSV" → "Chq Main all"; a starting name for a new account. */
const nameFromFile = (f: string) => f.replace(/\.[a-z0-9]+$/i, '').replace(/[_\-]+/g, ' ').replace(/\s+/g, ' ').trim() || f;
const kindFromFile = (f: string) => /saving/i.test(f) ? 'savings' : /credit|card|visa|master|amex|\d{4}\s*x/i.test(f) ? 'credit_card' : 'debit';

/** Human description of how an account picks up statement rows. */
function matchedBy(a: PeopleDoc['person'][number]['account'][number]): string {
  if (a.external) return 'nothing — external: no statements, a transfer destination';
  const parts: string[] = [];
  if (a.profile) parts.push(`format ${formatLabel(a.profile)}`);
  if (a.card_last4) parts.push(`number ${a.card_last4} in the file name`);
  if (a.filename) parts.push(describeFilename(a.filename));
  if (a.cardholder) parts.push(`cardholder “${a.cardholder}”`);
  return parts.join(' · ') || 'nothing (no conditions set)';
}

/**
 * Open the editor for `doc`. Resolves `true` when changes were saved into the
 * project's people.toml (the caller persists `doc`), `false` when cancelled.
 * `focus` highlights an account or person (e.g. the one being explored).
 */
export function openPeopleEditor(doc: ProjectDoc, opts: { focus?: string } = {}): Promise<boolean> {
  return new Promise((resolve) => {
    const original = peopleText(doc);
    let text = original;
    const undo: string[] = [];
    let error = '';
    let focus = opts.focus || '';
    const dataFiles = doc.files.filter((f) => f.role === 'data').map((f) => ({ name: f.name, bytes: f.bytes }));

    const overlay = h('div', { class: 'map-overlay', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'People and accounts' });
    const box = h('div', { class: 'map-box panel pe-box' });
    overlay.append(box);
    document.body.append(overlay);
    const close = (saved: boolean) => { overlay.remove(); document.removeEventListener('keydown', onKey); resolve(saved); };
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') close(false); };
    document.addEventListener('keydown', onKey);

    const apply = (ops: PeopleOp[], focusAfter?: string) => {
      try {
        const next = editPeople(text, ops);
        undo.push(text);
        text = next;
        error = '';
        if (focusAfter) focus = focusAfter;
      } catch (e) {
        error = (e as Error).message;
      }
      draw();
    };

    const draw = () => {
      let model: PeopleDoc | null = null;
      let checks: FileCheck[] = [];
      let configError = '';
      try { model = parsePeople(text); } catch (e) { configError = (e as Error).message; }
      if (model) {
        try {
          checks = checkFiles(dataFiles, { ...projectConfig(doc), 'people.toml': text });
        } catch (e) { configError = (e as Error).message; }
      }
      const rowsOf: Record<string, number> = {};
      for (const c of checks) for (const [a, n] of Object.entries(c.accounts || {})) rowsOf[a] = (rowsOf[a] || 0) + n;
      const persons = model?.person ?? [];
      const personOpts = persons.map((p) => ({ v: p.id, t: p.id }));

      const allAccounts = persons.flatMap((p) => p.account.map((a) => ({ a, p })));
      // "Which statements are X?" — the guided picks-up editor.
      const guide = (account: string, o: { create?: { person: string; kind: string }; tick?: string[] } = {}) => {
        const { 'people.toml': _omit, ...config } = projectConfig(doc);
        void _omit;
        openMatchGuide({
          text, files: dataFiles, config, checks, account, create: o.create, tick: o.tick,
          current: allAccounts.find((x) => x.a.id === account)?.a,
        }).then((ops) => { if (ops) apply(ops, account); });
      };

      // ---- health check ----
      const health = h('div', { class: 'pe-health' });
      {
        const items: HTMLElement[] = [];
        const item = (lvl: 'bad' | 'warn' | 'info', msg: string, ...acts: HTMLElement[]) =>
          items.push(h('div', { class: `pe-hi ${lvl}` }, h('span', { class: 'pe-ic' }, lvl === 'bad' ? '✗' : lvl === 'warn' ? '⚠' : 'ℹ'),
            h('span', {}, msg), ...acts));
        const unread = checks.filter((c) => c.rows === 0);
        if (unread.length) item('bad', `${unread.length} statement file${unread.length === 1 ? '' : 's'} can’t be read (${unread.map((c) => c.file).join(', ')}) — set up their format under ⚙ Configuration.`);
        const orphanN = checks.filter((c) => (c.accounts?.[UNASSIGNED] || 0) > 0).length;
        if (orphanN) item('bad', `${orphanN} statement file${orphanN === 1 ? ' has' : 's have'} rows on no account — assign ${orphanN === 1 ? 'it' : 'them'} below.`);
        for (const c of checks) {
          for (const [acct, n] of Object.entries(c.also || {})) {
            const winners = Object.keys(c.accounts).filter((x) => x !== acct && x !== UNASSIGNED);
            item('warn', `${n} row${n === 1 ? '' : 's'} of ${c.file} also fit “${acct}”, but go to ${winners.map((w) => `“${w}”`).join(', ')} (listed earlier wins).`,
              ...[acct, ...winners].map((x) => button(`Picks up… (${x})`, () => guide(x), 'btn small')));
          }
        }
        // Usually just statements not uploaded (yet); one line, each name opens its guide.
        const idle = allAccounts.filter(({ a }) => !a.external && !rowsOf[a.id]).map(({ a }) => a.id);
        if (idle.length) {
          item('info', `${idle.length === 1 ? '1 account picks' : `${idle.length} accounts pick`} up none of this project’s statements — fine if they aren’t uploaded; otherwise check what ${idle.length === 1 ? 'it picks' : 'they pick'} up:`,
            ...idle.map((id) => button(id, () => guide(id), 'btn small', `Which statements are “${id}”? (also: make it external)`)));
        }
        const known = new Set([...persons.map((p) => p.id), ...allAccounts.map((x) => x.a.id)]);
        for (const r of model?.transfer_rule ?? []) {
          if (!known.has(r.to)) item('warn', `Transfer rule /${r.pattern}/ sends money to “${r.to}”, which isn’t a person or account here.`);
        }
        if (model && !items.some((e) => !e.classList.contains('info'))) {
          items.unshift(h('div', { class: 'pe-hi ok' }, h('span', { class: 'pe-ic' }, '✓'), h('span', {}, 'Every statement row belongs to exactly one account.')));
        }
        health.append(...items);
      }

      // ---- statements on no account ----
      const orphans = checks.filter((c) => (c.accounts?.[UNASSIGNED] || 0) > 0);
      const orphanBox = h('div', { class: 'pe-orphans' });
      if (orphans.length) {
        orphanBox.append(h('h4', {}, `Statements on no account (${orphans.length})`),
          h('p', { class: 'muted' }, 'Their transactions show as “unassigned”. Pick the account each belongs to — a new one, or one you already have (also one marked external) — then confirm how it’s recognised.'));
        for (const c of orphans) {
          const target = select([{ v: '', t: 'New account…' }, ...allAccounts.map(({ a, p }) => ({ v: a.id, t: `${a.id} (${p.id})${a.external ? ' — no statements yet' : ''}` }))], '');
          const nameIn = h('input', { type: 'text', class: 'pe-in', value: nameFromFile(c.file), maxlength: '60', 'aria-label': 'New account name' }) as HTMLInputElement;
          const owner = select(personOpts, persons[0]?.id ?? '');
          const kind = select(KINDS, kindFromFile(c.file));
          const newBits = h('span', { class: 'pe-newbits' }, nameIn, ' owned by ', owner, ' ', kind);
          target.addEventListener('change', () => { newBits.hidden = !!target.value; });
          const go = button('Assign…', () => {
            if (target.value) guide(target.value, { tick: [c.file] });
            else {
              const id = nameIn.value.trim();
              if (!id) { nameIn.focus(); return; }
              guide(id, { create: { person: owner.value, kind: kind.value }, tick: [c.file] });
            }
          }, 'btn small active');
          if (!persons.length) { go.disabled = true; go.title = 'Add a person first'; }
          const n = c.accounts[UNASSIGNED];
          orphanBox.append(h('div', { class: 'pe-orphan', 'data-file': c.file },
            h('div', { class: 'pe-file' }, h('b', {}, c.file), h('span', { class: 'muted' }, ` · ${formatLabel(c.profile)} · ${n}${n !== c.rows ? ` of ${c.rows}` : ''} row${n === 1 ? '' : 's'} on no account`)),
            h('div', { class: 'pe-orphan-ctl' }, target, newBits, go)));
        }
      }

      // ---- people and their accounts ----
      const peopleBox = h('div', { class: 'pe-people' });
      for (const p of persons) {
        const pName = h('input', { type: 'text', class: 'pe-in pe-person-name', value: p.id, maxlength: '60', 'aria-label': `Rename ${p.id}` }) as HTMLInputElement;
        pName.addEventListener('change', () => { if (pName.value.trim() !== p.id) apply([{ op: 'rename_person', from: p.id, to: pName.value }], pName.value.trim()); });
        const prows = p.account.reduce((s, a) => s + (rowsOf[a.id] || 0), 0);
        const rulesTo = (id: string) => (model?.transfer_rule ?? []).filter((r) => r.to === id).length;
        const alsoRules = (id: string) => { const n = rulesTo(id); return n ? ` Its ${n} transfer rule${n === 1 ? '' : 's'} go${n === 1 ? 'es' : ''} too.` : ''; };
        const delPerson = button('🗑', () => apply([{ op: 'delete_person', person: p.id }]), 'btn small pe-del',
          p.account.length ? 'Move or delete their accounts first' : `Delete ${p.id}.${alsoRules(p.id)} (Undo is available until you save)`);
        delPerson.disabled = p.account.length > 0;
        delPerson.setAttribute('aria-label', `Delete person ${p.id}`);
        const card = h('div', { class: 'pe-person' + (focus === p.id ? ' pe-focus' : ''), 'data-person': p.id },
          h('div', { class: 'pe-person-head' }, pName,
            h('span', { class: 'muted' }, ` ${p.account.length} account${p.account.length === 1 ? '' : 's'}${prows ? ` · ${prows} rows` : ''}`),
            delPerson));
        if (p.account.length) {
          const tb = h('tbody', {});
          for (const a of p.account) {
            const aName = h('input', { type: 'text', class: 'pe-in', value: a.id, maxlength: '60', 'aria-label': `Rename account ${a.id}` }) as HTMLInputElement;
            aName.addEventListener('change', () => { if (aName.value.trim() !== a.id) apply([{ op: 'rename_account', from: a.id, to: aName.value }], aName.value.trim()); });
            const kindSel = select(kindOpts(a.kind), a.kind);
            kindSel.setAttribute('aria-label', `Kind of ${a.id}`);
            kindSel.addEventListener('change', () => apply([{ op: 'set_kind', account: a.id, kind: kindSel.value }], a.id));
            const own = select(personOpts, p.id);
            own.title = 'Owner';
            own.addEventListener('change', () => apply([{ op: 'move_account', account: a.id, to_person: own.value }], a.id));
            const how = button(a.external ? 'Give it statements…' : 'Change…', () => guide(a.id), 'btn small pe-how-btn',
              'Choose which statement files belong to this account');
            const rows = rowsOf[a.id] || 0;
            const del = button('🗑', () => apply([{ op: 'delete_account', account: a.id }]), 'btn small pe-del',
              `Delete “${a.id}”.${rows ? ` Its ${rows} rows will belong to no account.` : ''}${alsoRules(a.id)} (Undo is available until you save)`);
            del.setAttribute('aria-label', `Delete account ${a.id}`);
            tb.append(h('tr', { class: focus === a.id ? 'pe-focus' : '', 'data-account': a.id },
              h('td', {}, aName),
              h('td', {}, kindSel),
              h('td', { class: 'pe-how' }, h('span', { class: 'muted' }, matchedBy(a)), ' ', how),
              h('td', { class: 'num' + (!a.external && !rows ? ' pe-zero' : '') }, a.external ? '—' : String(rows)),
              h('td', {}, own),
              h('td', {}, del)));
          }
          card.append(h('table', { class: 'map-table pe-table' },
            h('thead', {}, h('tr', {}, ...['Account', 'Kind', 'Picks up', 'Rows', 'Owner', ''].map((t) => h('th', { class: t === 'Rows' ? 'num' : '' }, t)))), tb));
        }
        // Add an external account (savings elsewhere, an RRSP, …): a transfer destination.
        const extName = h('input', { type: 'text', class: 'pe-in', placeholder: 'e.g. RRSP at another bank', maxlength: '60' }) as HTMLInputElement;
        const extKind = select(KINDS, 'savings');
        card.append(h('div', { class: 'pe-add' },
          h('span', { class: 'muted' }, '+ external account '), extName, ' ', extKind, ' ',
          button('Add', () => { if (extName.value.trim()) apply([{ op: 'add_account', person: p.id, id: extName.value, kind: extKind.value, external: true }], extName.value.trim()); })));
        peopleBox.append(card);
      }
      const newPerson = h('input', { type: 'text', class: 'pe-in', placeholder: 'Name', maxlength: '60', 'aria-label': 'New person' }) as HTMLInputElement;
      const addPerson = () => { if (newPerson.value.trim()) apply([{ op: 'add_person', id: newPerson.value }], newPerson.value.trim()); };
      newPerson.addEventListener('keydown', (e) => { if (e.key === 'Enter') addPerson(); });
      peopleBox.append(h('div', { class: 'pe-add pe-add-person' }, h('b', {}, '+ Person '), newPerson, ' ', button('Add', addPerson)));

      // ---- footer ----
      const changes = undo.length;
      const saveBtn = button('Save', () => { upsertFile(doc, 'people.toml', new TextEncoder().encode(text)); close(true); }, 'btn active');
      saveBtn.disabled = !changes || !!configError;
      const undoBtn = button('↶ Undo', () => { const prev = undo.pop(); if (prev != null) { text = prev; error = ''; draw(); } }, 'btn');
      undoBtn.disabled = !changes;

      box.replaceChildren(
        h('div', { class: 'map-head' }, h('h3', {}, 'People & accounts'),
          button('✕', () => close(false), 'btn small', 'Close without saving (Esc)')),
        h('p', { class: 'muted' }, 'Who owns which account, and what they’re called in the report. Saved in this project’s people.toml — part of the config bundle, so it travels with ⇩ Download / ⇪ Upload config. Renames also update the transfer rules that name them.'),
        ...(error ? [h('div', { class: 'map-problems' }, error)] : []),
        ...(configError ? [h('div', { class: 'map-problems' }, `This people.toml can’t be used: ${configError}`)] : []),
        health, orphanBox, peopleBox,
        h('div', { class: 'map-actions' },
          h('span', { class: 'muted pe-count' }, changes ? `${changes} change${changes === 1 ? '' : 's'} not saved yet` : 'No changes'),
          undoBtn, button('Cancel', () => close(false), 'btn'), saveBtn),
      );
      const f = focus && (box.querySelector(`[data-account="${CSS.escape(focus)}"]`) || box.querySelector(`[data-person="${CSS.escape(focus)}"]`));
      if (f) f.scrollIntoView({ block: 'center' });
    };
    draw();
  });
}
