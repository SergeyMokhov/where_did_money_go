import 'uplot/dist/uPlot.min.css';
import './report.css';
import './app.css';
import { analyzeProject, initEngine, type AnalyzeResult } from './engine';
import { DEMO_ID, ensureDemo } from './demo';
import { fileText, getActiveId, getProject, INSIGHTS_NAME, type ProjectDoc } from './store';
import { runReport } from './report.js';
import { drawChangePoints, initCpControls } from './cpview';
import { openConfig } from './config';
import { openRulesEditor, type RuleExample } from './rules';
import { openPeopleEditor } from './people';
import { saveProject } from './store';
import { initYank } from './yank';

const $ = (id: string) => document.getElementById(id) as HTMLElement;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

interface Insights {
  summary?: string | null;
  recommendations?: { title?: string | null; detail?: string | null; est_monthly_savings?: string | null }[];
  trend_notes?: string[];
}

function readInsights(doc: ProjectDoc): { insights: Insights; error?: string } {
  const f = doc.files.find((x) => x.name === INSIGHTS_NAME);
  if (!f) return { insights: {} };
  try {
    const v = JSON.parse(fileText(f));
    return { insights: v && typeof v === 'object' ? v : {} };
  } catch (e) {
    return { insights: {}, error: `insights.json is not valid JSON: ${(e as Error).message}` };
  }
}

/** The parts the CLI template rendered server-side (askama), now filled in here. */
function renderStatic(doc: ProjectDoc, res: AnalyzeResult, insights: Insights) {
  const a = res.analysis;
  $('proj-name').textContent = doc.name;
  document.title = `${doc.name} · Where Did Money Go?`;
  const p = a.period || {};
  $('period-sub').textContent = [p.start, p.end].filter(Boolean).join(' → ');
  $('currency-sub').textContent = a.currency || '';

  const sum = $('summary');
  if (insights.summary) { sum.textContent = insights.summary; sum.hidden = false; }

  const recs = $('recs-panel');
  recs.replaceChildren();
  const list = Array.isArray(insights.recommendations) ? insights.recommendations : [];
  for (const r of list) {
    const box = el('div', 'rec');
    const t = el('div', 't', r.title || '');
    if (r.est_monthly_savings) t.append(el('span', 'save', ` · save ~${r.est_monthly_savings}/mo`));
    box.append(t);
    if (r.detail) box.append(el('div', undefined, r.detail));
    recs.append(box);
  }
  const notes = Array.isArray(insights.trend_notes) ? insights.trend_notes : [];
  if (notes.length) {
    const h = el('h3', 'muted', 'Notes');
    h.style.cssText = 'font-size:14px;margin:14px 0 6px;';
    const ul = el('ul');
    notes.forEach((n) => ul.append(el('li', undefined, n)));
    recs.append(h, ul);
  }
  if (!list.length && !notes.length) {
    recs.append(el('div', 'empty',
      'No insights yet. Export this project, let Claude run the where-did-money-go insight step on it, then upload the resulting insights.json in ⚙ Configuration.'));
  }

  const d = a.dedup_report || {};
  const overlaps = (d.overlaps || []).map((o: any) => `${o.file_a} ∩ ${o.file_b} (${o.count})`).join('; ');
  $('dedup-note').textContent =
    `Deduplication: kept ${d.kept ?? 0}, removed ${d.removed ?? 0} duplicate row(s) across overlapping files.` +
    (overlaps ? ` Overlaps: ${overlaps}` : '');
}

/** Problems worth telling the user about, shown above the report. */
/** Open the People & accounts editor from the report; a save re-analyzes (reload). */
function editPeopleFrom(doc: ProjectDoc, focus?: string) {
  openPeopleEditor(doc, { focus }).then(async (saved) => {
    if (saved) { await saveProject(doc); location.reload(); }
  });
}

/** Open Categories & rules from the report (optionally on one transaction); a save re-analyzes. */
function editRulesFrom(doc: ProjectDoc, example?: RuleExample) {
  openRulesEditor(doc, { example }).then(async (saved) => {
    if (saved) { await saveProject(doc); location.reload(); }
  });
}

function renderNotices(res: AnalyzeResult | null, extra: (string | Node)[], doc?: ProjectDoc) {
  const msgs: (string | Node)[] = [...extra];
  // Spending/income no category rule matched: offer to sort it.
  const unsorted = (res?.analysis?.transactions || []).filter((t: any) => t.kind !== 'transfer' && (!t.category || t.category === 'Uncategorized')).length;
  if (unsorted && doc) {
    const line = el('div', 'notice-row');
    const b = el('button', 'btn small active', 'Sort them…');
    b.addEventListener('click', () => editRulesFrom(doc));
    line.append(`${unsorted.toLocaleString()} transaction${unsorted === 1 ? ' isn’t' : 's aren’t'} in any category yet. `, b);
    msgs.push(line);
  }
  // Rows no account in people.toml claims: offer to assign them right here.
  const orphans = (res?.analysis?.transactions || []).filter((t: any) => t.account === 'unassigned').length;
  if (orphans && doc) {
    const line = el('div', 'notice-row');
    const b = el('button', 'btn small active', 'Assign accounts…');
    b.addEventListener('click', () => editPeopleFrom(doc));
    line.append(`${orphans.toLocaleString()} transaction${orphans === 1 ? '' : 's'} belong to no account (shown as “unassigned”). `, b);
    msgs.push(line);
  }
  if (res) {
    for (const e of res.load_report.errors) {
      msgs.push(/no profile matched/.test(e.error)
        ? `${e.file}: not a statement layout the analyzer knows. Open ⚙ Configuration and use “Map columns…” on it. (${e.error})`
        : `${e.file}: ${e.error}`);
    }
    if (res.load_report.skipped.length) msgs.push(`Skipped (not a known statement format): ${res.load_report.skipped.join(', ')}`);
  }
  let box = document.getElementById('notices');
  if (!msgs.length) { box?.remove(); return; }
  if (!box) {
    box = el('div', 'notices');
    box.id = 'notices';
    document.querySelector('#report-root .wrap header')?.after(box);
  }
  box.replaceChildren(...msgs.map((m) => (typeof m === 'string' ? el('div', undefined, m) : m)));
}

async function boot() {
  try {
    await initEngine();
  } catch (e) {
    $('boot').textContent = `Couldn't start the analysis engine (WebAssembly): ${(e as Error).message}`;
    return;
  }
  await ensureDemo();
  const id = (await getActiveId()) || DEMO_ID;
  const doc = (await getProject(id)) || (await getProject(DEMO_ID));
  if (!doc) { $('boot').textContent = 'No project found.'; return; }

  $('config-open').addEventListener('click', () => openConfig(doc.id));
  $('config-close').addEventListener('click', () => {
    // Project contents may have changed; the report is cheapest to rebuild from scratch.
    location.reload();
  });

  let res: AnalyzeResult;
  try {
    res = analyzeProject(doc);
  } catch (e) {
    $('boot').hidden = true;
    $('report-root').hidden = false;
    $('proj-name').textContent = doc.name;
    renderNotices(null, [`The engine couldn't analyze "${doc.name}": ${(e as Error).message ?? e}`,
      'Open ⚙ Configuration to fix or replace the offending file.']);
    return;
  }
  const { insights, error } = readInsights(doc);
  $('boot').hidden = true;
  $('report-root').hidden = false;
  renderStatic(doc, res, insights);
  const noData = !(res.analysis.transactions || []).length;
  renderNotices(res, [
    ...(error ? [error] : []),
    ...(noData ? ['This project has no transactions yet — add statement CSVs in ⚙ Configuration.'] : []),
  ], doc);
  // CP settings must be loaded before runReport, which may draw the CP view at init.
  let api: { redraw: () => void } | null = null;
  initCpControls(doc.id, () => api?.redraw());
  api = runReport({ analysis: res.analysis, insights }, {
    projectId: doc.id, drawChangePoints,
    onEditPeople: (focus?: string) => editPeopleFrom(doc, focus),
    onCategorise: (ex: RuleExample) => editRulesFrom(doc, ex),
  });
  initYank($('report-root'));
}

boot();
