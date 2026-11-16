import { createStore, del, get, getMany, keys, set } from 'idb-keyval';
import { KDF_ITERATIONS, deriveKey, exportKey, importKey, randomBytes, seal, unseal, type Sealed } from './crypto';

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
  /** When the project was last exported as a .zip (a backup), if ever. */
  backupAt?: number;
  /**
   * Passphrase-protected: the files are stored only encrypted, here. The name,
   * dates and this envelope stay readable (to list the project and unlock it).
   */
  sealed?: Sealed;
  /** Runtime only: protected and not unlocked in this tab, so `files` is empty. */
  locked?: boolean;
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

// ---------- passphrase keys of unlocked projects ----------
// Held in memory and in this tab's sessionStorage (the app reloads itself after
// most saves): they vanish when the tab closes or the project is locked again.
const keyCache = new Map<string, CryptoKey>();
const KEY_PREFIX = 'wdmg.key.';
const b64 = (u: Uint8Array) => btoa(String.fromCharCode(...u));
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
async function keyFor(id: string): Promise<CryptoKey | undefined> {
  const k = keyCache.get(id);
  if (k) return k;
  let raw: string | null = null;
  try { raw = sessionStorage.getItem(KEY_PREFIX + id); } catch { /* storage blocked */ }
  if (!raw) return undefined;
  const key = await importKey(unb64(raw));
  keyCache.set(id, key);
  return key;
}
async function rememberKey(id: string, key: CryptoKey) {
  keyCache.set(id, key);
  try { sessionStorage.setItem(KEY_PREFIX + id, b64(await exportKey(key))); } catch { /* memory only */ }
}
/** Forget a project's key in this tab: it's locked again until the passphrase is entered. */
export function lockProject(id: string) {
  keyCache.delete(id);
  try { sessionStorage.removeItem(KEY_PREFIX + id); } catch { /* ignore */ }
}

/** The stored record (files of a protected project are NOT decrypted). */
const raw = (id: string) => get<ProjectDoc>(id, projectStore);

/** Projects for listing: protected ones come back without files (`locked` when not unlocked here). */
export async function listProjects(): Promise<ProjectDoc[]> {
  const ids = (await keys(projectStore)) as string[];
  const docs = (await getMany<ProjectDoc>(ids, projectStore)).filter((d): d is ProjectDoc => d != null);
  for (const d of docs) if (d.sealed) d.locked = !(await keyFor(d.id));
  return docs.sort((a, b) => (a.demo ? -1 : b.demo ? 1 : b.savedAt - a.savedAt));
}

/** A project with its files — decrypted when it's protected and unlocked; else `locked` with no files. */
export async function getProject(id: string): Promise<ProjectDoc | undefined> {
  const doc = await raw(id);
  if (!doc?.sealed) return doc;
  const key = await keyFor(id);
  if (!key) return { ...doc, files: [], locked: true };
  try {
    return { ...doc, files: await unseal(key, doc.sealed), locked: false };
  } catch {
    lockProject(id); // a stale key (passphrase changed elsewhere)
    return { ...doc, files: [], locked: true };
  }
}

export async function saveProject(doc: ProjectDoc, opts: { touch?: boolean } = {}): Promise<void> {
  if (doc.locked) throw new Error(`“${doc.name}” is locked — unlock it with its passphrase first.`);
  if (opts.touch !== false) {
    doc.savedAt = Date.now();
    if (doc.demo) doc.demo.modified = true;
  }
  const { locked: _l, ...rec } = doc;
  if (doc.sealed) {
    const key = await keyFor(doc.id);
    if (!key) throw new Error(`“${doc.name}” is locked — unlock it with its passphrase first.`);
    rec.sealed = await seal(key, doc.files, doc.sealed.salt, doc.sealed.iter);
    doc.sealed = rec.sealed;
    rec.files = [];
  }
  await set(doc.id, rec, projectStore);
}

/** Protect a project with a passphrase: from now on its files are stored only encrypted. */
export async function protectProject(doc: ProjectDoc, passphrase: string): Promise<void> {
  if (doc.demo) throw new Error('The demo project can’t be encrypted.');
  if (passphrase.length < 8) throw new Error('Use a passphrase of at least 8 characters.');
  const salt = randomBytes(16);
  const key = await deriveKey(passphrase, salt, KDF_ITERATIONS);
  await rememberKey(doc.id, key);
  doc.sealed = { v: 1, kdf: 'PBKDF2-SHA256', iter: KDF_ITERATIONS, salt, iv: new Uint8Array(12), data: new Uint8Array() };
  await saveProject(doc, { touch: false });
}

/** Unlock a protected project in this tab; throws "Wrong passphrase." */
export async function unlockProject(id: string, passphrase: string): Promise<ProjectDoc> {
  const doc = await raw(id);
  if (!doc?.sealed) throw new Error('This project isn’t encrypted.');
  const key = await deriveKey(passphrase, doc.sealed.salt, doc.sealed.iter);
  const files = await unseal(key, doc.sealed);
  await rememberKey(id, key);
  return { ...doc, files, locked: false };
}

/** Remove the passphrase: the (unlocked) project is stored unencrypted again. */
export async function unprotectProject(doc: ProjectDoc): Promise<void> {
  if (doc.locked) throw new Error('Unlock it first.');
  delete doc.sealed;
  await saveProject(doc, { touch: false });
  lockProject(doc.id);
}

export async function deleteProject(id: string): Promise<void> {
  const doc = await raw(id);
  if (doc?.demo) throw new Error('The demo project cannot be deleted (use Reset instead).');
  await del(id, projectStore);
  lockProject(id);
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
