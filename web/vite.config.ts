import { defineConfig } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

// Single-file build: JS, CSS and the WebAssembly engine are all inlined into
// dist/index.html, so the app opens from file:// or any static host and never
// talks to a network. The demo project is bundled from ../playground.
export default defineConfig({
  base: './',
  plugins: [viteSingleFile()],
  build: {
    // Inline the .wasm (imported with ?url) as a data: URL instead of a file.
    assetsInlineLimit: Number.MAX_SAFE_INTEGER,
    chunkSizeWarningLimit: 8000,
  },
  server: { fs: { allow: ['..'] } },
});
