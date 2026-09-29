import path from 'path';
import { fileURLToPath } from 'url';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

import { devSite } from './vite-dev-site';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

export default defineConfig(({ mode }) => ({
  // Relative base, so the built directory works unmodified at any path on any
  // host — a GitLab Pages project site lives under `/<group>/<project>/`, and
  // nothing there rewrites URLs. This is sound only because routing is hash-based:
  // the document's own path never changes as the user navigates, so relative asset
  // and `data/` URLs keep resolving after a deep link.
  base: './',
  plugins: [
    // Dev only: injects the bootstrap the exporter would and serves `data/` from a
    // generated site, since there is no REST fallback to develop against.
    devSite(),
    react({
      babel: {
        plugins: [['babel-plugin-react-compiler']],
      },
    }),
  ],
  resolve: {
    dedupe: ['@tanstack/react-query', 'react', 'react-dom'],
  },
  build: {
    target: 'esnext',
    // Sourcemaps are never shipped: the bundle is committed to this repo and
    // embedded in the binary. `web/.gitignore` also excludes `dist/**/*.map`.
    sourcemap: mode === 'production' ? false : true,
    rollupOptions: {
      input: {
        index: path.resolve(__dirname, 'index.html'),
      },
      output: {
        dir: 'dist',
        // First paint now matters more than total bytes: the shell renders from a
        // ~350 KB parquet read while DuckDB-WASM streams in from a CDN, so the
        // app's own JS should not be one blocking megabyte. Splitting the heavy,
        // rarely-changing vendor code out also keeps it cached across releases.
        manualChunks: {
          react: ['react', 'react-dom', 'react-router-dom'],
          query: ['@tanstack/react-query', '@tanstack/react-table'],
          // The pure-JS parquet reader and its decompressors. Needed on the
          // critical path, but separate so a change to app code does not
          // invalidate them.
          parquet: ['hyparquet', 'hyparquet-compressors'],
        },
      },
    },
  },
  server: {
    port: 3002,
  },
}));
