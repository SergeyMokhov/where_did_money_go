// The privacy note (what leaves the browser — nothing — and who could read the
// stored data), plus the passphrase dialogs used to protect and unlock projects.
import { KDF_ITERATIONS } from './crypto';

function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) { if (k === 'class') e.className = v; else e.setAttribute(k, v); }
  e.append(...kids);
  return e;
}
function modal(label: string) {
  const overlay = h('div', { class: 'map-overlay pv-overlay', role: 'dialog', 'aria-modal': 'true', 'aria-label': label });
  const box = h('div', { class: 'map-box panel pv-box' });
  overlay.append(box);
  document.body.append(overlay);
  const close = () => { overlay.remove(); document.removeEventListener('keydown', onKey, true); };
  const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.stopPropagation(); close(); } };
  document.addEventListener('keydown', onKey, true);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
  return { box, close };
}

/** Where the app is served from, for the "same address" caveat. */
function origin(): string {
  return location.protocol === 'file:' ? 'a file on your computer' : location.host;
}

export function openPrivacyNote() {
  const { box, close } = modal('Privacy');
  const body = h('div', { class: 'pv-body' });
  const where = origin();
  body.innerHTML = `
<h4>Nothing leaves this browser</h4>
<p>The analysis engine runs inside this page. The page carries a <b>Content-Security-Policy</b> that forbids it every way of
reaching a server — no requests, no external scripts, images or fonts — so the browser itself would block any attempt.
You can check: the browser's developer tools (Network tab) stay empty, and the policy is the first line of the page source.</p>
<h4>Where your projects are kept</h4>
<ul>
<li>In this browser's storage for this site (IndexedDB), on this computer only — not synced, not backed up by the app.</li>
<li>Clearing this site's data, or a private window closing, deletes them. <b>⇩ Export (.zip)</b> in ⚙ Configuration is your backup.</li>
</ul>
<h4>Who else could read them</h4>
<ul>
<li><b>Other websites can't</b> — browsers keep each site's storage separate.</li>
<li><b>Other pages at the same address</b> (${escapeHtml(where)}) share this storage. Any page published there could read it.</li>
<li><b>Browser extensions</b> allowed to “read and change data on all websites” can see what pages show and store.</li>
<li><b>Anyone or anything with access to your computer account</b> can read the browser's files.</li>
</ul>
<p>For the last three, <b>protect a project with a passphrase</b> (⚙ Configuration → the project → 🔒 Protect with a passphrase),
and <b>press Lock now when you're done</b> — protection only fully applies while the project is locked (see below).</p>
<h4>Passphrase protection</h4>
<ul>
<li>The project's statements, configuration and insights are stored only encrypted: AES-GCM with a 256-bit key derived from
your passphrase (PBKDF2-SHA256, ${KDF_ITERATIONS.toLocaleString()} iterations, a random salt per project). Its name and dates stay readable.</li>
<li>On disk the project stays encrypted even while you work on it. But once you unlock it, its key is kept in
<b>this tab</b> (so the app can reopen it after saving) until you press <b>Lock now</b> (⚙ Configuration → the project) or close the tab.
While it's unlocked, an extension, another page opened in this same tab, or someone at your computer could open the project
without the passphrase. A browser that restores tabs after a restart can also keep the key — so <b>lock it when you're done</b>.</li>
<li><b>A forgotten passphrase can't be recovered</b> — the data is gone except for exported backups.</li>
<li>Exported .zip files are <i>not</i> encrypted; keep them somewhere safe.</li>
</ul>
<h4>AI insights</h4>
<p>The optional AI step is the only time your data goes anywhere — and only when you export it and hand it to an AI yourself.</p>`;
  box.append(h('div', { class: 'map-head' }, h('h3', {}, '🔒 Privacy'), btnEl('✕', close, 'btn small')), body,
    h('div', { class: 'map-actions' }, btnEl('Close', close, 'btn active')));
}

const escapeHtml = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]!));
function btnEl(label: string, onClick: () => void, cls = 'btn', title = '') {
  const b = h('button', { type: 'button', class: cls, ...(title ? { title } : {}) }, label) as HTMLButtonElement;
  b.addEventListener('click', onClick);
  return b;
}

/**
 * Ask for a new passphrase (twice). Resolves with it, or null on cancel.
 */
export function askNewPassphrase(projectName: string): Promise<string | null> {
  return new Promise((resolve) => {
    const { box, close } = modal('Protect with a passphrase');
    const p1 = h('input', { type: 'password', class: 'pe-in', autocomplete: 'new-password', 'aria-label': 'Passphrase' }) as HTMLInputElement;
    const p2 = h('input', { type: 'password', class: 'pe-in', autocomplete: 'new-password', 'aria-label': 'Repeat the passphrase' }) as HTMLInputElement;
    const ack = h('input', { type: 'checkbox', 'aria-label': 'I understand' }) as HTMLInputElement;
    const msg = h('div', { class: 'mg-verdict warn' });
    const ok = btnEl('🔒 Encrypt', () => { if (check()) { close(); resolve(p1.value); } }, 'btn active');
    const check = () => {
      const bad = p1.value.length < 8 ? 'At least 8 characters — a few unrelated words work well.'
        : p1.value !== p2.value ? 'The two entries differ.'
          : !ack.checked ? 'Tick the box to confirm you understand.' : '';
      msg.textContent = bad || 'Ready.';
      msg.className = 'mg-verdict ' + (bad ? 'warn' : 'ok');
      ok.disabled = !!bad;
      return !bad;
    };
    for (const e of [p1, p2, ack]) e.addEventListener('input', check);
    p2.addEventListener('keydown', (e) => { if (e.key === 'Enter' && check()) { close(); resolve(p1.value); } });
    box.append(
      h('div', { class: 'map-head' }, h('h3', {}, `🔒 Protect “${projectName}” with a passphrase`)),
      h('p', {}, 'Its statements, configuration and insights will be stored only encrypted in this browser. You’ll enter the passphrase once per browser tab; it stays unlocked in that tab until you press Lock now or close the tab.'),
      h('div', { class: 'pv-form' }, h('label', { class: 'pv-field' }, h('span', {}, 'Passphrase'), p1), h('label', { class: 'pv-field' }, h('span', {}, 'Repeat it'), p2),
        h('label', { class: 'pv-ack' }, ack, h('span', {}, 'I understand a forgotten passphrase can’t be recovered — only an exported backup would bring the data back.'))),
      msg,
      h('div', { class: 'map-actions' }, btnEl('Cancel', () => { close(); resolve(null); }, 'btn'), ok));
    check();
    p1.focus();
  });
}

/**
 * An unlock form (inline, not a dialog). `unlock` throws on a wrong passphrase;
 * `onDone` runs after a successful unlock.
 */
export function unlockForm(projectName: string, unlock: (pass: string) => Promise<unknown>, onDone: () => void): HTMLElement {
  const pass = h('input', { type: 'password', class: 'pe-in pv-pass', autocomplete: 'current-password', 'aria-label': `Passphrase for ${projectName}` }) as HTMLInputElement;
  const msg = h('div', { class: 'muted pv-msg' }, 'The passphrase is checked here, in this browser.');
  const go = async () => {
    if (!pass.value) { pass.focus(); return; }
    b.disabled = true; msg.textContent = 'Unlocking…'; msg.className = 'muted pv-msg';
    try { await unlock(pass.value); onDone(); } catch (e) {
      msg.textContent = (e as Error).message; msg.className = 'neg pv-msg'; b.disabled = false; pass.select();
    }
  };
  const b = btnEl('Unlock', go, 'btn active');
  pass.addEventListener('keydown', (e) => { if (e.key === 'Enter') void go(); });
  setTimeout(() => pass.focus(), 0);
  return h('div', { class: 'pv-unlock' },
    h('div', { class: 'pv-unlock-t' }, `🔒 “${projectName}” is encrypted`),
    h('div', { class: 'pv-unlock-row' }, pass, b), msg);
}
