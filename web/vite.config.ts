import { defineConfig, type Plugin } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

// Single-file build: JS, CSS and the WebAssembly engine are all inlined into
// dist/index.html, so the app opens from file:// or any static host and never
// talks to a network. The demo project is bundled from ../playground.
/**
 * The no-network guarantee, enforced by the browser: a Content-Security-Policy
 * that allows only the page's own inline code and styles and forbids every way
 * of reaching a server (fetch/XHR/WebSocket/beacons, external scripts, styles,
 * images, fonts, frames, form posts). Build only — the dev server needs its
 * websocket. `wasm-unsafe-eval` lets the inlined engine compile; it does not
 * allow JavaScript eval.
 */
export const CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline' 'wasm-unsafe-eval'",
  "style-src 'unsafe-inline'",
  'img-src data: blob:',
  'font-src data:',
  "connect-src 'none'",
  "worker-src 'none'",
  "frame-src 'none'",
  "object-src 'none'",
  "media-src 'none'",
  "manifest-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
].join('; ');
const cspPlugin = (): Plugin => ({
  name: 'wdmg-csp',
  apply: 'build',
  transformIndexHtml: (html) => html.replace('<meta charset="utf-8">', `<meta charset="utf-8">\n<meta http-equiv="Content-Security-Policy" content="${CSP}">`),
});

export default defineConfig({
  base: './',
  plugins: [cspPlugin(), viteSingleFile()],
  build: {
    // Inline the .wasm (imported with ?url) as a data: URL instead of a file.
    assetsInlineLimit: Number.MAX_SAFE_INTEGER,
    chunkSizeWarningLimit: 8000,
  },
  server: { fs: { allow: ['..'] } },
});
