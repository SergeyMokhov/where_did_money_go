// Project <-> .zip (data/*.csv, config/*.toml, insights.json, project.json).
import { strFromU8, strToU8, unzipSync, zipSync } from 'fflate';
import { newId, roleFor, upsertFile, type FileRole, type ProjectDoc } from './store';

export const MAX_FILE_BYTES = 50 * 1024 * 1024;
const MAX_ZIP_TOTAL = 200 * 1024 * 1024;

export function exportZip(doc: ProjectDoc): Uint8Array {
  const files: Record<string, Uint8Array> = {
    'project.json': strToU8(JSON.stringify({ name: doc.name, format: 'wdmg-project', version: 1 }, null, 2)),
  };
  for (const f of doc.files) {
    const dir = f.role === 'data' ? 'data/' : f.role === 'config' ? 'config/' : '';
    files[dir + f.name] = f.bytes;
  }
  return zipSync(files, { level: 6 });
}

export type BundleKind = 'data' | 'config';

/** Just one part of a project: its statements (data/*.csv) or its config (config/*.toml). */
export function exportBundle(doc: ProjectDoc, kind: BundleKind): Uint8Array | null {
  const files: Record<string, Uint8Array> = {};
  for (const f of doc.files) if (f.role === kind) files[`${kind}/${f.name}`] = f.bytes;
  if (!Object.keys(files).length) return null;
  files['bundle.json'] = strToU8(JSON.stringify({ format: `wdmg-${kind}`, version: 1, project: doc.name }, null, 2));
  return zipSync(files, { level: 6 });
}

/** Unpack a zip into {name, role, bytes} entries (folders ignored; unknown files skipped). */
export function readZipEntries(bytes: Uint8Array): { entries: { name: string; role: FileRole; bytes: Uint8Array }[]; skipped: string[] } {
  const raw = unzipChecked(bytes);
  const entries: { name: string; role: FileRole; bytes: Uint8Array }[] = [];
  const skipped: string[] = [];
  for (const [path, data] of Object.entries(raw)) {
    const base = path.split('/').pop() || path;
    if (base === 'project.json' || base === 'bundle.json' || base.startsWith('.') || path.startsWith('__MACOSX/')) continue;
    try { entries.push({ name: base, role: roleFor(base), bytes: data }); } catch { skipped.push(path); }
  }
  return { entries, skipped };
}

function unzipChecked(bytes: Uint8Array): Record<string, Uint8Array> {
  let total = 0;
  return unzipSync(bytes, {
    filter: (f) => {
      total += f.originalSize;
      if (f.originalSize > MAX_FILE_BYTES || total > MAX_ZIP_TOTAL) throw new Error('Zip is too large to import.');
      return !f.name.endsWith('/');
    },
  });
}

export function importZip(bytes: Uint8Array, fallbackName: string): { doc: ProjectDoc; skipped: string[] } {
  const raw = unzipChecked(bytes);
  const now = Date.now();
  let name = fallbackName;
  if (raw['project.json']) {
    try { const m = JSON.parse(strFromU8(raw['project.json'])); if (typeof m.name === 'string' && m.name.trim()) name = m.name.trim(); } catch { /* keep fallback */ }
  }
  const doc: ProjectDoc = { id: newId(), name, files: [], createdAt: now, savedAt: now };
  const skipped: string[] = [];
  for (const [path, data] of Object.entries(raw)) {
    const base = path.split('/').pop() || path;
    if (base === 'project.json' || base.startsWith('.') || path.startsWith('__MACOSX/')) continue;
    try { upsertFile(doc, base, data); } catch { skipped.push(path); }
  }
  return { doc, skipped };
}

