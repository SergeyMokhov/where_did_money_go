import 'fake-indexeddb/auto';
import { describe, expect, it } from 'vitest';
import { DEMO_ID, ensureDemo, resetDemo } from './demo';
import {
  deleteProject, getActiveId, getProject, listProjects, lockProject, newId, protectProject, roleFor, saveProject, setActiveId,
  unlockProject, unprotectProject, upsertFile, type ProjectDoc,
} from './store';
import { get } from 'idb-keyval';
import { decodeFiles, encodeFiles } from './crypto';

const enc = new TextEncoder();

describe('roleFor', () => {
  it('classifies statements, configs and insights', () => {
    expect(roleFor('joint_chequing.CSV')).toBe('data');
    expect(roleFor('Categories.TOML')).toBe('config');
    expect(roleFor('insights.json')).toBe('insights');
  });
  it('rejects anything else', () => {
    expect(() => roleFor('evil.html')).toThrow();
    expect(() => roleFor('random.toml')).toThrow();
    expect(() => roleFor('analysis.json')).toThrow();
  });
});

describe('upsertFile', () => {
  it('replaces a same-named file and canonicalizes config names', () => {
    const doc: ProjectDoc = { id: 'x', name: 'x', files: [], createdAt: 0, savedAt: 0 };
    upsertFile(doc, 'a.csv', enc.encode('1'));
    upsertFile(doc, 'dir/a.csv', enc.encode('2'));
    upsertFile(doc, 'People.toml', enc.encode('3'));
    expect(doc.files.map((f) => f.name)).toEqual(['a.csv', 'people.toml']);
    expect(new TextDecoder().decode(doc.files[0].bytes)).toBe('2');
  });
});

describe('demo project', () => {
  it('is seeded from the playground, cannot be deleted, and resets', async () => {
    await ensureDemo();
    const demo = await getProject(DEMO_ID);
    expect(demo?.demo?.modified).toBe(false);
    expect(demo!.files.filter((f) => f.role === 'data').length).toBeGreaterThan(3);
    expect(demo!.files.some((f) => f.name === 'categories.toml')).toBe(true);
    expect(demo!.files.some((f) => f.name === 'insights.json')).toBe(true);

    await expect(deleteProject(DEMO_ID)).rejects.toThrow(/cannot be deleted/);

    demo!.files = demo!.files.filter((f) => f.role !== 'data');
    await saveProject(demo!);
    expect((await getProject(DEMO_ID))!.demo!.modified).toBe(true);

    // ensureDemo must not clobber a user's edits...
    await ensureDemo();
    expect((await getProject(DEMO_ID))!.files.some((f) => f.role === 'data')).toBe(false);
    // ...but Reset restores the bundled files.
    await resetDemo();
    const back = await getProject(DEMO_ID);
    expect(back!.demo!.modified).toBe(false);
    expect(back!.files.filter((f) => f.role === 'data').length).toBeGreaterThan(3);
  });

  it('lists the demo first and deletes ordinary projects', async () => {
    const id = newId();
    await saveProject({ id, name: 'Mine', files: [], createdAt: 1, savedAt: 1 });
    await setActiveId(id);
    const ids = (await listProjects()).map((p) => p.id);
    expect(ids[0]).toBe(DEMO_ID);
    expect(ids).toContain(id);
    await deleteProject(id);
    expect(await getProject(id)).toBeUndefined();
    expect(await getActiveId()).toBeUndefined();
  });
});

describe('zip round trip', async () => {
  const { exportZip, importZip } = await import('./zip');
  it('restores name and every file under a fresh id', () => {
    const doc: ProjectDoc = { id: 'orig', name: 'Household 2026', files: [], createdAt: 0, savedAt: 0 };
    upsertFile(doc, 'a.csv', enc.encode('Date,Amount\n01/01/2026,1.00\n'));
    upsertFile(doc, 'people.toml', enc.encode('schema_version = 1\n'));
    upsertFile(doc, 'insights.json', enc.encode('{}'));
    const { doc: back, skipped } = importZip(exportZip(doc), 'fallback');
    expect(skipped).toEqual([]);
    expect(back.id).not.toBe('orig');
    expect(back.name).toBe('Household 2026');
    const norm = (d: ProjectDoc) => d.files.map((f) => [f.name, f.role, new TextDecoder().decode(f.bytes)]).sort();
    expect(norm(back)).toEqual(norm(doc));
  });
});

describe('bundles', async () => {
  const { exportBundle, readZipEntries } = await import('./zip');
  it('exports only one role and reads it back', () => {
    const doc: ProjectDoc = { id: 'b', name: 'B', files: [], createdAt: 0, savedAt: 0 };
    upsertFile(doc, 'a.csv', enc.encode('x'));
    upsertFile(doc, 'people.toml', enc.encode('schema_version = 1\n'));
    upsertFile(doc, 'insights.json', enc.encode('{}'));
    const cfg = readZipEntries(exportBundle(doc, 'config')!);
    expect(cfg.entries.map((e) => [e.name, e.role])).toEqual([['people.toml', 'config']]);
    const data = readZipEntries(exportBundle(doc, 'data')!);
    expect(data.entries.map((e) => e.name)).toEqual(['a.csv']);
    expect(exportBundle({ ...doc, files: [] }, 'config')).toBeNull();
  });
});

describe('passphrase encryption', () => {
  it('round-trips files through the encoding', () => {
    const files = [{ name: 'a.csv', role: 'data' as const, addedAt: 5, bytes: enc.encode('x,y\n1,2') }, { name: 'people.toml', role: 'config' as const, addedAt: 6, bytes: new Uint8Array() }];
    expect(decodeFiles(encodeFiles(files))).toEqual(files);
  });
  it('stores a protected project only encrypted, and needs the passphrase to read it', async () => {
    const id = newId();
    const doc: ProjectDoc = { id, name: 'Secret', files: [], createdAt: 1, savedAt: 1 };
    upsertFile(doc, 'bank.csv', enc.encode('Date,Amount\n2999-01-01,-12.34 SLURM'));
    await saveProject(doc);
    await protectProject(doc, 'correct horse battery');
    // At rest: no files, and the ciphertext doesn't contain the statement text.
    const { createStore } = await import('idb-keyval');
    const stored = await get<ProjectDoc>(id, createStore('wdmg-projects', 'projects'));
    expect(stored!.files).toEqual([]);
    expect(new TextDecoder().decode(stored!.sealed!.data)).not.toContain('SLURM');
    // Unlocked in this tab: reads and saves transparently (a new IV each save).
    const open = await getProject(id);
    expect(open!.locked).toBe(false);
    expect(open!.files[0].name).toBe('bank.csv');
    const iv1 = open!.sealed!.iv;
    upsertFile(open!, 'more.csv', enc.encode('a'));
    await saveProject(open!);
    expect((await getProject(id))!.files.map((f) => f.name)).toEqual(['bank.csv', 'more.csv']);
    expect((await getProject(id))!.sealed!.iv).not.toEqual(iv1);
    // Locked: no files, can't be saved, the list says so.
    lockProject(id);
    const shut = await getProject(id);
    expect(shut!.locked).toBe(true);
    expect(shut!.files).toEqual([]);
    await expect(saveProject(shut!)).rejects.toThrow(/locked/);
    expect((await listProjects()).find((p) => p.id === id)!.locked).toBe(true);
    // Wrong passphrase fails; the right one unlocks.
    await expect(unlockProject(id, 'wrong horse battery')).rejects.toThrow(/Wrong passphrase/);
    const back = await unlockProject(id, 'correct horse battery');
    expect(back.files.length).toBe(2);
    // Removing the passphrase stores it in the clear again.
    await unprotectProject(back);
    const plain = await getProject(id);
    expect(plain!.sealed).toBeUndefined();
    expect(plain!.files.length).toBe(2);
  });
  it('refuses short passphrases and the demo', async () => {
    const doc: ProjectDoc = { id: newId(), name: 'x', files: [], createdAt: 1, savedAt: 1 };
    await expect(protectProject(doc, 'short')).rejects.toThrow(/8 characters/);
    await expect(protectProject({ ...doc, demo: { version: 1, modified: false } }, 'long enough pass')).rejects.toThrow(/demo/);
  });
});
