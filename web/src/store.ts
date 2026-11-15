import { createStore, del, get, getMany, keys, set } from 'idb-keyval';

/**
 * A project = one data set (bank CSV exports) + its configuration (the TOML files
 * the Rust engine reads) + an optional `insights.json` written by the AI step.
 * Everything lives in this browser's IndexedDB; nothing is ever uploaded.
 */
export type FileRole = 'data' | 'config' | 'insights';

export interface ProjectFile {
  name: string;
  role: FileRole;
  bytes: Uint8Array;
  addedAt: number;
}

export interface ProjectDoc {
  id: string;
  name: string;
  files: ProjectFile[];
  createdAt: number;
  savedAt: number;
  /** Set on the bundled demo: it can be reset but never deleted. */
  demo?: { version: number; modified: boolean };
}

/** Config file names the engine understands (anything else is rejected). */
export const CONFIG_NAMES = [
  'profiles.toml',
  'categories.toml',
  'people.toml',
  'events.toml',
  'settings.toml',
  'learned.toml',
] as const;

export const INSIGHTS_NAME = 'insights.json';

const projectStore = createStore('wdmg-projects', 'projects');
const metaStore = createStore('wdmg-meta', 'meta');

export async function listProjects(): Promise<ProjectDoc[]> {
  const ids = (await keys(projectStore)) as string[];
  const docs = await getMany<ProjectDoc>(ids, projectStore);
  return docs
    .filter((d): d is ProjectDoc => d != null)
    .sort((a, b) => (a.demo ? -1 : b.demo ? 1 : b.savedAt - a.savedAt));
}

export function getProject(id: string): Promise<ProjectDoc | undefined> {
  return get<ProjectDoc>(id, projectStore);
}

export async function saveProject(doc: ProjectDoc, opts: { touch?: boolean } = {}): Promise<void> {
  if (opts.touch !== false) {
    doc.savedAt = Date.now();
    if (doc.demo) doc.demo.modified = true;
  }
  await set(doc.id, doc, projectStore);
}

export async function deleteProject(id: string): Promise<void> {
  const doc = await getProject(id);
  if (doc?.demo) throw new Error('The demo project cannot be deleted (use Reset instead).');
  await del(id, projectStore);
  if ((await getActiveId()) === id) await del('active', metaStore);
}

export function getActiveId(): Promise<string | undefined> {
  return get<string>('active', metaStore);
}

export function setActiveId(id: string): Promise<void> {
  return set('active', id, metaStore);
}

export function newId(): string {
  return crypto.randomUUID ? crypto.randomUUID() : `p-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/** Classify a file by name; throws on names the engine would not understand. */
export function roleFor(name: string): FileRole {
  const lower = name.toLowerCase();
  if ((CONFIG_NAMES as readonly string[]).includes(lower)) return 'config';
  if (lower === INSIGHTS_NAME) return 'insights';
  if (lower.endsWith('.csv')) return 'data';
  throw new Error(
    `"${name}" isn't a statement export (.csv), a config file (${CONFIG_NAMES.join(', ')}) or ${INSIGHTS_NAME}.`,
  );
}

/** Add or replace (same name) a file in a project document (not yet saved). */
export function upsertFile(doc: ProjectDoc, name: string, bytes: Uint8Array): ProjectFile {
  const base = name.split(/[\\/]/).pop() || name;
  const role = roleFor(base);
  const canonical = role === 'data' ? base : base.toLowerCase();
  const file: ProjectFile = { name: canonical, role, bytes, addedAt: Date.now() };
  const i = doc.files.findIndex((f) => f.name === canonical);
  if (i >= 0) doc.files[i] = file;
  else doc.files.push(file);
  return file;
}

export function removeFile(doc: ProjectDoc, name: string): void {
  doc.files = doc.files.filter((f) => f.name !== name);
}

const dec = new TextDecoder();
export const fileText = (f: ProjectFile): string => dec.decode(f.bytes);
