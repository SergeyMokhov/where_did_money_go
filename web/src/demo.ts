import { getProject, saveProject, upsertFile, type ProjectDoc } from './store';

/** Fixed id so the demo can be recognized (Reset instead of Delete). */
export const DEMO_ID = 'demo';
/** Bump to refresh an *unmodified* demo in existing browsers. */
const DEMO_VERSION = 1;
const DEMO_NAME = 'Demo — Planet Express household';

// The playground data set + its config, bundled into the page at build time.
const DATA = import.meta.glob('../../playground/data/*.csv', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;
const CONFIG = import.meta.glob('../../playground/config/*.{toml,json}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const enc = new TextEncoder();

function buildDemo(): ProjectDoc {
  const now = Date.now();
  const doc: ProjectDoc = {
    id: DEMO_ID,
    name: DEMO_NAME,
    files: [],
    createdAt: now,
    savedAt: now,
    demo: { version: DEMO_VERSION, modified: false },
  };
  for (const [path, text] of Object.entries({ ...DATA, ...CONFIG })) {
    upsertFile(doc, path.split('/').pop() as string, enc.encode(text));
  }
  doc.files.sort((a, b) => a.role.localeCompare(b.role) || a.name.localeCompare(b.name));
  return doc;
}

/** Create the demo on first run; refresh it when the bundled version changed and the user hasn't edited it. */
export async function ensureDemo(): Promise<void> {
  const cur = await getProject(DEMO_ID);
  if (cur && cur.demo && (cur.demo.modified || cur.demo.version >= DEMO_VERSION)) return;
  await saveProject(buildDemo(), { touch: false });
}

/** Restore the demo to its bundled state (discarding edits). */
export async function resetDemo(): Promise<void> {
  await saveProject(buildDemo(), { touch: false });
}
