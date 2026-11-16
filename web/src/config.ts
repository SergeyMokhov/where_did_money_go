// The ⚙ Configuration screen: manage projects (data set + config files) that
// live in this browser's IndexedDB. Uploads are read locally with the File API;
// downloads are generated in-page — no file ever leaves the browser.
import { DEMO_ID, resetDemo } from './demo';
import { checkProject, defaultConfig, type FileCheck } from './engine';
import { openMapper } from './mapper';
import { openPeopleEditor } from './people';
import { openRulesEditor } from './rules';
import { MAX_FILE_BYTES, exportBundle, exportZip, importZip, readZipEntries, type BundleKind } from './zip';
import {
  CONFIG_NAMES, INSIGHTS_NAME, deleteProject, getActiveId, listProjects, newId,
  removeFile, roleFor, saveProject, setActiveId, upsertFile, type ProjectDoc,
} from './store';


const CONFIG_HELP: Record<string, string> = {
  'profiles.toml': 'Your own statement formats (e.g. from “Map columns…”), added to the built-in ones.',
  'categories.toml': 'Spend / income / transfer categories and their merchant patterns.',
  'people.toml': 'Household: who owns which card/account, transfer rules.',
  'events.toml': 'Life events drawn on the chart.',
  'settings.toml': 'Analysis profile (which unusual-activity methods show) and thresholds.',
  'learned.toml': 'Rules learned by the AI review step (layered under categories.toml).',
};

let selectedId: string | null = null;
let changed = false;

function h<K extends keyof HTMLElementTagNameMap>(
  tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') e.className = v; else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
}
function btn(label: string, onClick: () => void | Promise<void>, cls = 'btn', title = ''): HTMLButtonElement {
  const b = h('button', { class: cls, type: 'button', ...(title ? { title } : {}) }, label);
  b.addEventListener('click', async () => {
    try { await onClick(); } catch (e) { toast((e as Error).message || String(e), true); }
  });
  return b;
}

function toast(msg: string, bad = false) {
  const t = h('div', { class: `cfg-toast${bad ? ' bad' : ''}` }, msg);
  document.body.append(t);
  setTimeout(() => t.remove(), bad ? 7000 : 3500);
}

function download(name: string, data: Uint8Array | string, type = 'application/octet-stream') {
  const blob = new Blob([typeof data === 'string' ? data : (data as BlobPart)], { type });
  const a = h('a', { href: URL.createObjectURL(blob), download: name });
  document.body.append(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
}

const fmtSize = (n: number) => (n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`);
const fmtDate = (ms: number) => new Date(ms).toLocaleString();
const safeName = (s: string) => s.replace(/[^\w.-]+/g, '_').replace(/^_+|_+$/g, '') || 'project';

/** Ask the browser not to evict our data. Fire-and-forget: Firefox may show a
 *  permission prompt, and awaiting it would stall the UI until it's answered. */
function persistStorage() {
  try { void navigator.storage?.persist?.()?.catch(() => {}); } catch { /* best effort */ }
}

// ---------- screen ----------
export async function openConfig(activeId: string) {
  selectedId = selectedId || activeId;
  document.getElementById('report-root')!.hidden = true;
  document.getElementById('config-screen')!.hidden = false;
  window.scrollTo(0, 0);
  await render();
}

async function render() {
  const body = document.getElementById('config-body')!;
  const projects = await listProjects();
  const activeId = (await getActiveId()) || DEMO_ID;
  if (!projects.some((p) => p.id === selectedId)) selectedId = activeId;
  const sel = projects.find((p) => p.id === selectedId) || projects[0];

  const closeBtn = document.getElementById('config-close')!;
  closeBtn.textContent = changed ? '← Back to report (re-analyze)' : '← Back to report';

  body.replaceChildren(
    h('div', { class: 'cfg-grid' }, projectList(projects, activeId), sel ? projectDetail(sel, activeId) : h('div')),
  );
}

function projectList(projects: ProjectDoc[], activeId: string): HTMLElement {
  const nameIn = h('input', { type: 'text', placeholder: 'New project name', maxlength: '80' }) as HTMLInputElement;
  const create = async () => {
    const name = nameIn.value.trim();
    if (!name) { nameIn.focus(); return; }
    const now = Date.now();
    const doc: ProjectDoc = { id: newId(), name, files: [], createdAt: now, savedAt: now };
    await saveProject(doc);
    persistStorage();
    selectedId = doc.id;
    toast(`Created "${name}". Add statement CSVs to it.`);
    await render();
  };
  nameIn.addEventListener('keydown', (e) => { if (e.key === 'Enter') create(); });

  const zipIn = h('input', { type: 'file', accept: '.zip,application/zip', hidden: '' }) as HTMLInputElement;
  zipIn.addEventListener('change', async () => {
    const f = zipIn.files?.[0];
    zipIn.value = '';
    if (!f) return;
    try {
      const { doc, skipped } = importZip(new Uint8Array(await f.arrayBuffer()), f.name.replace(/\.zip$/i, ''));
      await saveProject(doc);
      persistStorage();
      selectedId = doc.id;
      toast(`Imported "${doc.name}" (${doc.files.length} files)${skipped.length ? `; skipped ${skipped.join(', ')}` : ''}.`);
      await render();
    } catch (e) { toast(`Import failed: ${(e as Error).message}`, true); }
  });

  const list = h('div', { class: 'cfg-projects' });
  for (const p of projects) {
    const nData = p.files.filter((f) => f.role === 'data').length;
    const item = h('div', { class: `cfg-proj${p.id === selectedId ? ' sel' : ''}`, tabindex: '0', role: 'button' },
      h('div', { class: 'cfg-proj-name' }, p.name,
        ...(p.id === activeId ? [h('span', { class: 'cfg-badge' }, 'showing')] : []),
        ...(p.demo ? [h('span', { class: 'cfg-badge muted' }, p.demo.modified ? 'demo · modified' : 'demo')] : [])),
      h('div', { class: 'muted cfg-proj-sub' }, `${nData} statement file${nData === 1 ? '' : 's'} · saved ${fmtDate(p.savedAt)}`),
    );
    const pick = async () => { selectedId = p.id; await render(); };
    item.addEventListener('click', pick);
    item.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); pick(); } });
    list.append(item);
  }

  return h('div', { class: 'panel cfg-left' },
    h('h2', { class: 'cfg-h' }, 'Projects'),
    list,
    h('div', { class: 'cfg-new' }, nameIn, btn('+ New project', create, 'btn')),
    h('div', { class: 'cfg-new' }, btn('⇪ Import project (.zip)', () => zipIn.click()), zipIn),
  );
}

function projectDetail(doc: ProjectDoc, activeId: string): HTMLElement {
  const isDemo = doc.id === DEMO_ID || !!doc.demo;
  const save = async () => { await saveProject(doc); changed = true; await render(); };

  // Header: name + actions.
  const nameIn = h('input', { type: 'text', value: doc.name, maxlength: '80', class: 'cfg-name' }) as HTMLInputElement;
  nameIn.addEventListener('change', async () => {
    const v = nameIn.value.trim();
    if (!v) { nameIn.value = doc.name; return; }
    doc.name = v; await save();
  });
  const actions = h('div', { class: 'cfg-actions' },
    btn(doc.id === activeId ? '▶ Showing in report' : '▶ Open in report', async () => {
      await setActiveId(doc.id); location.reload();
    }, doc.id === activeId ? 'btn active' : 'btn'),
    btn('⇩ Export (.zip)', () => download(`${safeName(doc.name)}.zip`, exportZip(doc), 'application/zip'), 'btn',
      'Everything in this project as one zip — give it to Claude for the AI-insights step, or keep as a backup.'),
    isDemo
      ? btn('↺ Reset demo', async () => {
          if (!doc.demo?.modified) { toast('The demo is already in its original state.'); return; }
          if (!confirm('Reset the demo project to its original files? Your changes to it will be lost.')) return;
          await resetDemo(); changed = true; toast('Demo reset.'); await render();
        }, 'btn', 'Restore the bundled demo files')
      : btn('✕ Delete project', async () => {
          if (!confirm(`Delete "${doc.name}" and all its files from this browser? This can't be undone (export it first if unsure).`)) return;
          await deleteProject(doc.id); selectedId = null; changed = true; toast('Project deleted.'); await render();
        }, 'btn danger'),
  );

  // Upload zone (CSV / TOML / insights.json, multi-select or drop).
  const fileIn = h('input', { type: 'file', multiple: '', accept: '.csv,.toml,.json,.zip', hidden: '' }) as HTMLInputElement;
  const addFiles = async (files: FileList | File[]) => {
    const added: string[] = [], bad: string[] = [];
    for (const f of Array.from(files)) {
      if (f.size > MAX_FILE_BYTES) { bad.push(`${f.name} (larger than ${fmtSize(MAX_FILE_BYTES)})`); continue; }
      try {
        if (/\.zip$/i.test(f.name)) {
          // Any bundle or project zip: take every file it holds that we recognise.
          const { entries, skipped } = readZipEntries(new Uint8Array(await f.arrayBuffer()));
          for (const e of entries) { upsertFile(doc, e.name, e.bytes); added.push(e.name); }
          if (skipped.length) bad.push(`skipped from ${f.name}: ${skipped.join(', ')}`);
          continue;
        }
        roleFor(f.name);
        upsertFile(doc, f.name, new Uint8Array(await f.arrayBuffer()));
        added.push(f.name);
      } catch (e) { bad.push((e as Error).message); }
    }
    if (added.length) { persistStorage(); await save(); toast(`Added ${added.join(', ')}`); }
    if (bad.length) toast(bad.join(' · '), true);
  };
  fileIn.addEventListener('change', async () => { const fl = fileIn.files; if (fl) await addFiles(fl); fileIn.value = ''; });
  const drop = h('div', { class: 'cfg-drop', tabindex: '0', role: 'button' },
    h('b', {}, 'Drop files here or click to choose'),
    h('div', { class: 'muted' }, 'Statement exports (.csv), config files (.toml), insights.json, or a .zip of any of them. A file with the same name replaces the old one.'));
  drop.addEventListener('click', () => fileIn.click());
  drop.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); fileIn.click(); } });
  drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', async (e) => {
    e.preventDefault(); drop.classList.remove('over');
    if (e.dataTransfer?.files?.length) await addFiles(e.dataTransfer.files);
  });

  const fileRow = (name: string, size: string, note: Node | string, ...btns: HTMLElement[]) =>
    h('tr', {}, h('td', { class: 'cfg-fname' }, name), h('td', { class: 'num muted' }, size), h('td', { class: 'muted' }, note), h('td', { class: 'cfg-fbtns' }, ...btns));
  const removeBtn = (name: string) => btn('✕', async () => {
    if (!confirm(`Remove ${name} from "${doc.name}"?`)) return;
    removeFile(doc, name); await save();
  }, 'btn small', 'Remove');
  const dlBtn = (name: string, bytes: Uint8Array) => btn('⇩', () => download(name, bytes), 'btn small', 'Download');

  // Per-section bundles: download one part of the project, or upload a zip of it.
  const LABEL: Record<BundleKind, string> = { data: 'statements', config: 'config' };
  const bundleBtns = (kind: BundleKind) => {
    const zipIn = h('input', { type: 'file', accept: '.zip', hidden: '' }) as HTMLInputElement;
    zipIn.addEventListener('change', async () => {
      const f = zipIn.files?.[0];
      zipIn.value = '';
      if (!f) return;
      try {
        const { entries, skipped } = readZipEntries(new Uint8Array(await f.arrayBuffer()));
        const mine = entries.filter((e) => e.role === kind);
        const other = entries.filter((e) => e.role !== kind).map((e) => e.name);
        if (!mine.length) { toast(`${f.name} has no ${kind === 'data' ? '.csv' : 'config .toml'} files.`, true); return; }
        if (kind === 'config') {
          // A config bundle is a whole configuration: files it lacks fall back to defaults.
          if (doc.files.some((x) => x.role === 'config') &&
              !confirm(`Replace this project's configuration with the ${mine.length} file(s) in ${f.name}? Config files not in the bundle go back to the built-in defaults.`)) return;
          doc.files = doc.files.filter((x) => x.role !== 'config');
        }
        for (const e of mine) upsertFile(doc, e.name, e.bytes);
        persistStorage(); await save();
        const ign = [...other, ...skipped];
        toast(`Loaded ${mine.map((e) => e.name).join(', ')}${ign.length ? ` (ignored ${ign.join(', ')})` : ''}.`);
      } catch (e) { toast(`Upload failed: ${(e as Error).message}`, true); }
    });
    return h('span', { class: 'cfg-h3-btns' },
      btn(`⇩ Download ${LABEL[kind]} (.zip)`, () => {
        const z = exportBundle(doc, kind);
        if (!z) { toast(kind === 'config' ? 'No custom config files — this project uses the built-in defaults.' : 'No statement files to download.'); return; }
        download(`${safeName(doc.name)}-${LABEL[kind]}.zip`, z, 'application/zip');
      }, 'btn small', kind === 'config' ? 'Just the .toml files, to reuse in another project' : 'Just the statement CSVs'),
      btn(`⇪ Upload ${LABEL[kind]} (.zip)`, () => zipIn.click(), 'btn small',
        kind === 'config' ? 'Replace this project’s configuration with a config bundle' : 'Add the CSVs in a zip (same-named files are replaced)'),
      zipIn);
  };

  // Statements, each with whether (and how) the engine can read it.
  const data = doc.files.filter((f) => f.role === 'data').sort((a, b) => a.name.localeCompare(b.name));
  let checks: FileCheck[] = [];
  let checkErr = '';
  try { checks = checkProject(doc); } catch (e) { checkErr = (e as Error).message; }
  const statusOf = (name: string): Node => {
    if (checkErr) return h('span', { class: 'neg' }, 'Config problem — see below');
    const c = checks.find((x) => x.file === name);
    if (!c) return h('span', {}, '');
    if (!c.error) return h('span', { class: 'cfg-ok', title: `Format: ${c.profile}` }, `✓ ${c.profile} · ${c.rows} row${c.rows === 1 ? '' : 's'}`);
    return h('span', { class: 'neg', title: c.error }, c.unrecognized ? '⚠ Not recognised' : `✕ ${c.error.length > 90 ? c.error.slice(0, 90) + '…' : c.error}`);
  };
  const mapBtn = (name: string) => {
    const c = checks.find((x) => x.file === name);
    if (!c?.unrecognized) return [];
    const f = doc.files.find((x) => x.name === name)!;
    return [btn('Map columns…', async () => { if (await openMapper(doc, f)) { await save(); toast('Saved a new format to profiles.toml.'); } },
      'btn small active', 'Tell the analyzer what each column of this file holds')];
  };
  const dataTbl = h('table', { class: 'cfg-files' }, h('tbody', {},
    ...(data.length ? data.map((f) => {
      const tr = fileRow(f.name, fmtSize(f.bytes.length), statusOf(f.name), ...mapBtn(f.name), dlBtn(f.name, f.bytes), removeBtn(f.name));
      tr.title = `added ${fmtDate(f.addedAt)}`;
      if (checks.find((x) => x.file === f.name)?.error) tr.classList.add('cfg-bad');
      return tr;
    })
      : [h('tr', {}, h('td', { class: 'empty', colspan: '4' }, 'No statement files yet.'))])));
  const badCount = checks.filter((c) => c.error).length;
  const orphanFiles = checks.filter((c) => (c.accounts?.unassigned || 0) > 0).length;
  const rulesBtn = btn('🏷 Categories & rules…', async () => {
    if (await openRulesEditor(doc)) { await save(); toast('Saved categories & rules.'); }
  }, 'btn small');
  const peopleBtn = (label: string, cls = 'btn small') => btn(label, async () => {
    if (await openPeopleEditor(doc)) { await save(); toast('Saved people & accounts to people.toml.'); }
  }, cls, 'Rename people and accounts, change owners, give statement files an account');

  // Config: every known file, present or falling back to the built-in default.
  let defaults: Record<string, string> = {};
  try { defaults = defaultConfig(); } catch { /* engine not ready: no default downloads */ }
  const cfgRows = CONFIG_NAMES.map((name) => {
    const f = doc.files.find((x) => x.name === name);
    const upload = btn('⇪', () => {
      const one = h('input', { type: 'file', accept: '.toml', hidden: '' }) as HTMLInputElement;
      one.addEventListener('change', async () => {
        const file = one.files?.[0];
        if (!file) return;
        upsertFile(doc, name, new Uint8Array(await file.arrayBuffer()));
        await save(); toast(`Replaced ${name}${file.name !== name ? ` with ${file.name}` : ''}.`);
      });
      one.click();
    }, 'btn small', `Upload a file to use as ${name}`);
    if (f) return fileRow(name, fmtSize(f.bytes.length), CONFIG_HELP[name], dlBtn(name, f.bytes), upload, removeBtn(name));
    const def = defaults[name];
    const defNote = name === 'learned.toml' ? 'none (optional)' : 'using built-in default';
    return fileRow(name, '—', h('span', {}, h('i', {}, defNote), ' — ', CONFIG_HELP[name]),
      ...(def != null ? [btn('⇩', () => download(name, def, 'text/plain'), 'btn small', 'Download the built-in default to edit')] : []), upload);
  });
  const cfgTbl = h('table', { class: 'cfg-files' }, h('tbody', {}, ...cfgRows));

  // Insights.
  const ins = doc.files.find((f) => f.name === INSIGHTS_NAME);
  const insTbl = h('table', { class: 'cfg-files' }, h('tbody', {},
    ins ? fileRow(INSIGHTS_NAME, fmtSize(ins.bytes.length), 'AI recommendations shown in the report', dlBtn(INSIGHTS_NAME, ins.bytes), removeBtn(INSIGHTS_NAME))
      : fileRow(INSIGHTS_NAME, '—', 'none — export the project, ask Claude (where-did-money-go skill) to write insights for it, then upload the insights.json here')));

  return h('div', { class: 'panel cfg-right' },
    h('div', { class: 'cfg-detail-head' }, nameIn, actions),
    ...(isDemo ? [h('div', { class: 'muted cfg-note' }, 'The demo can’t be deleted. Edit it freely — ↺ Reset demo restores the original files.')] : []),
    drop, fileIn,
    h('h3', { class: 'cfg-h3' }, `Statements (${data.length})`, bundleBtns('data')),
    ...(badCount ? [h('div', { class: 'cfg-warn' }, `${badCount} file${badCount === 1 ? '' : 's'} can’t be read yet. Use “Map columns…” to teach the analyzer an unrecognised layout.`)] : []),
    ...(checkErr ? [h('div', { class: 'cfg-warn' }, `The configuration can’t be loaded: ${checkErr}`)] : []),
    ...(orphanFiles ? [h('div', { class: 'cfg-warn' },
      `${orphanFiles} statement file${orphanFiles === 1 ? ' has' : 's have'} rows that belong to no account. `,
      peopleBtn('Assign accounts…', 'btn small active'))] : []),
    dataTbl,
    h('h3', { class: 'cfg-h3' }, 'Configuration', h('span', { class: 'cfg-h3-btns' }, peopleBtn('👥 People & accounts…'), rulesBtn, bundleBtns('config'))), cfgTbl,
    h('h3', { class: 'cfg-h3' }, 'AI insights'), insTbl,
  );
}
