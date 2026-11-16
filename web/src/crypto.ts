// Passphrase encryption for projects at rest (IndexedDB), with the browser's
// own WebCrypto: AES-GCM-256 with a key derived from the passphrase by
// PBKDF2-SHA256 (600,000 iterations, per-project random salt), a fresh random
// IV on every save. A wrong passphrase fails GCM's authentication — there is no
// separate "check" value to attack. Nothing here can recover a forgotten
// passphrase: the data is gone, except for exported backups.
import type { FileRole, ProjectFile } from './store';

export const KDF_ITERATIONS = 600_000;

/** What a locked project stores instead of its files. */
export interface Sealed {
  v: 1;
  kdf: 'PBKDF2-SHA256';
  iter: number;
  salt: Uint8Array;
  iv: Uint8Array;
  /** AES-GCM ciphertext (with tag) of the encoded files. */
  data: Uint8Array;
}

const subtle = () => {
  const s = globalThis.crypto?.subtle;
  if (!s) throw new Error('This browser has no WebCrypto (a page opened over plain http? use https or the file).');
  return s;
};
export const randomBytes = (n: number) => globalThis.crypto.getRandomValues(new Uint8Array(n));

/** Derive the AES key. Extractable, so an unlocked key can survive this tab's reloads (see store). */
export async function deriveKey(passphrase: string, salt: Uint8Array, iter = KDF_ITERATIONS): Promise<CryptoKey> {
  const base = await subtle().importKey('raw', new TextEncoder().encode(passphrase.normalize('NFC')), 'PBKDF2', false, ['deriveKey']);
  return subtle().deriveKey({ name: 'PBKDF2', hash: 'SHA-256', salt: salt as BufferSource, iterations: iter },
    base, { name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);
}
export const exportKey = async (k: CryptoKey) => new Uint8Array(await subtle().exportKey('raw', k));
export const importKey = (raw: Uint8Array) =>
  subtle().importKey('raw', raw as BufferSource, { name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);

/** Files ⇄ bytes: a 4-byte header length, a JSON header, then each file's bytes. */
export function encodeFiles(files: ProjectFile[]): Uint8Array {
  const header = new TextEncoder().encode(JSON.stringify(files.map((f) => ({ name: f.name, role: f.role, addedAt: f.addedAt, len: f.bytes.length }))));
  const total = 4 + header.length + files.reduce((a, f) => a + f.bytes.length, 0);
  const out = new Uint8Array(total);
  new DataView(out.buffer).setUint32(0, header.length);
  out.set(header, 4);
  let at = 4 + header.length;
  for (const f of files) { out.set(f.bytes, at); at += f.bytes.length; }
  return out;
}
export function decodeFiles(bytes: Uint8Array): ProjectFile[] {
  const hl = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(0);
  const header = JSON.parse(new TextDecoder().decode(bytes.subarray(4, 4 + hl))) as { name: string; role: FileRole; addedAt: number; len: number }[];
  let at = 4 + hl;
  return header.map((h) => {
    const f = { name: h.name, role: h.role, addedAt: h.addedAt, bytes: bytes.slice(at, at + h.len) };
    at += h.len;
    return f;
  });
}

/** Encrypt `files` with `key`, keeping the salt/iterations the key was derived with. */
export async function seal(key: CryptoKey, files: ProjectFile[], salt: Uint8Array, iter: number): Promise<Sealed> {
  const iv = randomBytes(12);
  const data = new Uint8Array(await subtle().encrypt({ name: 'AES-GCM', iv: iv as BufferSource }, key, encodeFiles(files) as BufferSource));
  return { v: 1, kdf: 'PBKDF2-SHA256', iter, salt, iv, data };
}

/** Decrypt; throws "Wrong passphrase" when the key doesn't fit. */
export async function unseal(key: CryptoKey, s: Sealed): Promise<ProjectFile[]> {
  let plain: ArrayBuffer;
  try {
    plain = await subtle().decrypt({ name: 'AES-GCM', iv: s.iv as BufferSource }, key, s.data as BufferSource);
  } catch {
    throw new Error('Wrong passphrase.');
  }
  return decodeFiles(new Uint8Array(plain));
}
