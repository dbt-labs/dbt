# dbt-docs-lib

## Browser DuckDB API

`dbt-docs-lib/duckdb` is a standalone browser data-layer entry. It loads
DuckDB-WASM lazily from the configured CDN, reads generator-owned `views.sql`,
and registers whole parquet files only when a query needs them. It does not
import React components or styles.

```ts
import { createEngine } from 'dbt-docs-lib/duckdb';

const engine = createEngine({
  dataBaseUrl: 'https://example.test/info_schema/v1/',
  cdnBase: 'https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.32.0',
});
try {
  const rows = await engine.query<{ unique_id: string }>(
    'SELECT unique_id FROM dbt.models',
    ['dbt.models'],
  );
} finally {
  await engine.dispose();
}
```

An engine belongs to one artifact snapshot. Dispose it and create a new engine
when the snapshot changes. Disposal is idempotent; queries/readiness calls after
disposal reject. `ready()` can initialize the worker ahead of the first query,
and `hasTable(name)` reports successful registration after a query requested it.
Query rows are normalized to plain objects, arrays, and numbers.

`bundlePreference` is optional. Omitting it or setting it to `'auto'` preserves
DuckDB-WASM's capability-based bundle selection. Set it to `'mvp'` when a browser
embedding needs the conservative compatibility build:

```ts
const engine = createEngine({
  dataBaseUrl: 'https://example.test/info_schema/v1/',
  cdnBase: 'https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.32.0',
  bundlePreference: 'mvp',
});
```

The worker and WASM binary always come from the same bundle and configured CDN
root. The docs app omits this option and retains automatic selection. MVP can
trade performance for compatibility; opting in affects only that engine instance.

Linked consumers resolve `dbt-docs-lib/duckdb` to `dist/`, which is build output
ignored by Git, so this package must be installed and built before a consumer
typechecks, tests, or builds. The docs app depends on it through the pnpm
workspace (`workspace:*`) and does that itself: turbo builds this package before
the app's tasks, and the app's scripts run `scripts/build-deps.mjs` when invoked
outside turbo. Any other linked consumer needs the same step before its own build.

Focused validation:

```sh
pnpm typecheck
pnpm test lib/data-sources/duckdb
```

The browser consumer test lives in the docs app (`web/tests/`), since it exercises
this package as a linked consumer would. From `web/`:

```sh
pnpm exec playwright install chromium
pnpm test:duckdb-browser
```

It checks omitted, explicit auto, and MVP preferences,
including the selected worker/WASM asset pair. It compiles against built public
declarations, bundles only
the public entry, and uses a real Chromium worker with generated view SQL and
parquet fixtures. It covers overlapping lazy registration, row normalization,
disposal during initialization, failed initialization cleanup and retry, disposal
during a stalled artifact fetch, and HTML artifact fallbacks. A local mirror serves the installed DuckDB-WASM runtime for this test.

## React + TypeScript + Vite

This template provides a minimal setup to get React working in Vite with HMR and some ESLint rules.

Currently, two official plugins are available:

- [@vitejs/plugin-react](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react) uses [Oxc](https://oxc.rs)
- [@vitejs/plugin-react-swc](https://github.com/vitejs/vite-plugin-react/blob/main/packages/plugin-react-swc) uses [SWC](https://swc.rs/)

## React Compiler

The React Compiler is not enabled on this template because of its impact on dev & build performances. To add it, see [this documentation](https://react.dev/learn/react-compiler/installation).

## Expanding the ESLint configuration

If you are developing a production application, we recommend updating the configuration to enable type-aware lint rules:

```js
export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      // Other configs...

      // Remove tseslint.configs.recommended and replace with this
      tseslint.configs.recommendedTypeChecked,
      // Alternatively, use this for stricter rules
      tseslint.configs.strictTypeChecked,
      // Optionally, add this for stylistic rules
      tseslint.configs.stylisticTypeChecked,

      // Other configs...
    ],
    languageOptions: {
      parserOptions: {
        project: ['./tsconfig.node.json', './tsconfig.app.json'],
        tsconfigRootDir: import.meta.dirname,
      },
      // other options...
    },
  },
])

```

You can also install [eslint-plugin-react-x](https://npmx.dev/package/eslint-plugin-react-x) and [eslint-plugin-react-dom](https://npmx.dev/package/eslint-plugin-react-dom) for React-specific lint rules:

```js
// eslint.config.js
import reactX from 'eslint-plugin-react-x'
import reactDom from 'eslint-plugin-react-dom'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      // Other configs...
      // Enable lint rules for React
      reactX.configs['recommended-typescript'],
      // Enable lint rules for React DOM
      reactDom.configs.recommended,
    ],
    languageOptions: {
      parserOptions: {
        project: ['./tsconfig.node.json', './tsconfig.app.json'],
        tsconfigRootDir: import.meta.dirname,
      },
      // other options...
    },
  },
])

```
