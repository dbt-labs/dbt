/**
 * DuckDB-WASM over the site's parquet artifacts.
 *
 * The engine is fetched from a CDN at runtime and is never bundled: it is ~6.8 MB
 * brotli, it is versioned independently of this app, and jsDelivr serves it
 * `immutable` so it is cached once per visitor. Loading is lazy — nothing here
 * touches the network until the first query — which is what lets the shell paint
 * from the hyparquet bootstrap while this streams in behind it.
 *
 * Artifacts are fetched whole and handed to `registerFileBuffer` rather than read
 * over HTTP range requests. The whole artifact set for a 6,472-node project is
 * under 5 MB, so ranges buy nothing, and they are actively hostile here: GitLab
 * Pages only serves them when artifacts are stored uncompressed, and duckdb-wasm
 * has a known Firefox × range × compression failure on GitHub Pages. Registration
 * is per-artifact and on demand, so a cold load never pulls column lineage.
 *
 * **The view SQL is not written here.** It comes from the `views.sql` the
 * information schema ships beside its parquet, parsed by `viewsSql.ts`. This module
 * decides *when* to register a view and fetches what it needs; the statement itself
 * is the generator's. Authoring `CREATE VIEW` here is what made this app a second
 * definition of the schema.
 */

import type * as duckdb from '@duckdb/duckdb-wasm';

import type { ViewsDocument } from './viewsSql';
import { fetchViewsSql } from './viewsSql';

/** Logical relation name, e.g. `dbt.models`. A key into `views.sql`. */
export type TableName = string;

/**
 * Whether `bytes` are actually a parquet file.
 *
 * Checked before registering, because "the artifact is absent" does not reliably
 * arrive as a 404. A host that answers unknown paths with `index.html` — which
 * `dbt docs serve` does, and which most SPA hosts do by default — returns 200 and a
 * document, and registering that leaves DuckDB to fail on it with `No magic bytes
 * found at end of file`. The bytes are the only thing every host agrees on, so they
 * decide, and an expected empty relation stays an empty relation rather than
 * becoming a page that renders nothing.
 *
 * Parquet brackets the file with `PAR1` at both ends. The trailing copy is the one
 * that matters here — it is what DuckDB reads first, and what a truncated or
 * substituted file loses — but both are checked, since a document that happens to
 * end in `PAR1` is no more readable than one that does not. 12 bytes is the smallest
 * possible file: two magics plus the 4-byte footer length between them.
 */
export function isParquetBytes(bytes: Uint8Array): boolean {
  if (bytes.byteLength < 12) return false;
  const magic = [0x50, 0x41, 0x52, 0x31]; // 'PAR1'
  return magic.every(
    (byte, i) =>
      bytes[i] === byte && bytes[bytes.byteLength - magic.length + i] === byte,
  );
}

export interface EngineOptions {
  /** Absolute URL of the `data/` directory, with a trailing slash. */
  dataBaseUrl: string;
  /**
   * Package root the engine is loaded from, e.g.
   * `https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.32.0`.
   *
   * A mirror must serve the same layout: `/+esm` for the ESM entry and
   * `/dist/*.wasm` plus `/dist/*.worker.js` for the bundles.
   */
  cdnBase: string;
}

export interface DuckDbEngine {
  /**
   * Register `tables` if they are not already, then run `sql`.
   *
   * Rows come back as plain objects with the parquet's own snake_case column
   * names, so the existing `fromRest` mappers consume them unchanged.
   */
  query<T = Record<string, unknown>>(sql: string, tables: TableName[]): Promise<T[]>;
  /** Whether `table`'s artifact exists. Only meaningful after a `query` that asked for it. */
  hasTable(table: TableName): boolean;
  /** Resolve once the engine is usable. Callers that only need readiness, not a query. */
  ready(): Promise<void>;
}

interface Loaded {
  db: duckdb.AsyncDuckDB;
  conn: duckdb.AsyncDuckDBConnection;
  /** The view surface, as generated beside the parquet. */
  doc: ViewsDocument;
}

export function createEngine(options: EngineOptions): DuckDbEngine {
  // Single-flight: many hooks fire at once on first paint and must share one
  // engine, one worker, and one fetch per artifact.
  let loading: Promise<Loaded> | null = null;
  const registered = new Map<TableName, Promise<boolean>>();
  const present = new Set<TableName>();

  async function load(): Promise<Loaded> {
    // `@vite-ignore` keeps Rollup from trying to resolve and inline the CDN URL,
    // which is the whole point: the wasm must not end up in `web/dist/`.
    // In parallel with the engine: both are needed before the first query, and
    // the document is a few kilobytes next to the engine's ~6.8 MB.
    const docPromise = fetchViewsSql(options.dataBaseUrl);

    const wasm: typeof duckdb = await import(
      /* @vite-ignore */ `${options.cdnBase}/+esm`
    );

    // Build the bundle map from `cdnBase` rather than calling
    // `getJsDelivrBundles()`, whose URLs are hardcoded to jsDelivr — otherwise
    // `--duckdb-cdn-base` would move the loader but not the wasm it fetches.
    // Only `mvp` and `eh` are offered, deliberately: the `coi` bundle needs
    // COOP/COEP cross-origin-isolation headers, which a static host generally
    // cannot set.
    const bundle = await wasm.selectBundle({
      mvp: {
        mainModule: `${options.cdnBase}/dist/duckdb-mvp.wasm`,
        mainWorker: `${options.cdnBase}/dist/duckdb-browser-mvp.worker.js`,
      },
      eh: {
        mainModule: `${options.cdnBase}/dist/duckdb-eh.wasm`,
        mainWorker: `${options.cdnBase}/dist/duckdb-browser-eh.worker.js`,
      },
    });

    if (!bundle.mainWorker) {
      throw new Error('duckdb-wasm returned a bundle with no worker entry');
    }

    // The worker script is cross-origin, so it cannot be a `new Worker(url)`
    // directly. Wrapping it in a same-origin blob that `importScripts` the real
    // one is duckdb-wasm's own documented workaround.
    const workerUrl = URL.createObjectURL(
      new Blob([`importScripts("${bundle.mainWorker}");`], { type: 'text/javascript' }),
    );
    try {
      const worker = new Worker(workerUrl);
      const db = new wasm.AsyncDuckDB(
        new wasm.ConsoleLogger(wasm.LogLevel.WARNING),
        worker,
      );
      await db.instantiate(bundle.mainModule, bundle.pthreadWorker);
      const conn = await db.connect();
      // The schemas the document declares, not a list of our own: it knows about
      // `dbt_internal`, which the index layout had no equivalent of.
      const doc = await docPromise;
      for (const schema of doc.schemas) {
        await conn.query(schema);
      }
      return { db, conn, doc };
    } finally {
      URL.revokeObjectURL(workerUrl);
    }
  }

  function loaded(): Promise<Loaded> {
    loading ??= load();
    return loading;
  }

  /**
   * Create one view from the shipped document, fetching whatever it needs.
   *
   * A base view needs its parquet: it is fetched whole and handed to
   * `registerFileBuffer` under the *same* file name the statement reads, which is
   * what lets the generated `read_parquet('dbt.models.parquet')` run unchanged in
   * a browser that has no filesystem.
   *
   * A derived view needs the views it reads, so those register first. Registering
   * them in dependency order rather than executing the whole document is what
   * keeps a cold load from pulling every artifact.
   *
   * Resolves `false` when the view could not be created, which the caller lets
   * surface as a query error. Unlike the index, the information schema writes
   * every table even at zero rows — precisely so `views.sql` always resolves — so
   * a missing artifact is a broken site, not the empty-relation case the index
   * needed a stand-in for.
   *
   * A non-parquet body is treated as absent rather than registered, by
   * {@link isParquetBytes} rather than by the status code: a host that rewrites
   * unknown paths to `index.html` reports a missing artifact as 200 and a
   * document, and registering that leaves DuckDB to fail on it with "No magic
   * bytes found at end of file". A status that is neither — a 500, a timeout — is
   * still an error and still throws; those are worth surfacing.
   */
  function register(table: TableName): Promise<boolean> {
    const existing = registered.get(table);
    if (existing) return existing;

    const attempt = (async () => {
      const { db, conn, doc } = await loaded();
      const view = doc.views.get(table);
      if (!view) {
        // The app asked for a relation the generated document does not define.
        // That is this app and the artifacts disagreeing, so it is loud.
        throw new Error(
          `${table} is not defined in views.sql — the site's data was written by ` +
            'a dbt version whose information schema does not carry it',
        );
      }

      if (view.kind === 'derived') {
        // Prerequisites first: a derived view binds at creation, so one whose
        // base is absent fails there rather than at query time.
        const ready = await Promise.all(view.dependsOn.map(register));
        if (ready.some((ok) => !ok)) return false;
        await conn.query(view.sql);
        present.add(table);
        return true;
      }

      const res = await fetch(new URL(view.file, options.dataBaseUrl).href);
      if (!res.ok && res.status !== 404) {
        throw new Error(`${res.status} ${res.statusText} fetching ${view.file}`);
      }
      const bytes = res.ok ? new Uint8Array(await res.arrayBuffer()) : null;
      if (!bytes || !isParquetBytes(bytes)) return false;

      await db.registerFileBuffer(view.file, bytes);
      await conn.query(view.sql);
      present.add(table);
      return true;
    })();

    registered.set(table, attempt);
    return attempt;
  }

  return {
    async query<T>(sql: string, tables: TableName[]): Promise<T[]> {
      const { conn } = await loaded();
      await Promise.all(tables.map(register));
      const result = await conn.query(sql);
      return result.toArray().map((row) => normalizeRow(row.toJSON())) as T[];
    },

    hasTable(table: TableName): boolean {
      return present.has(table);
    },

    async ready(): Promise<void> {
      await loaded();
    },
  };
}

/**
 * Make one Arrow row consumable by the `fromRest` mappers.
 *
 * DuckDB returns `BIGINT` as `BigInt`, which `JSON.stringify` throws on and which
 * compares unequal to a number literal. Counts and row totals are all well within
 * `Number`'s safe range here, so widening is lossless in practice. Nested values
 * (`LIST`, `STRUCT`) arrive as Arrow vectors; flattening them to plain arrays and
 * objects is what keeps the mappers protocol-agnostic.
 */
function normalizeRow(row: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(row)) {
    out[key] = normalizeValue(value);
  }
  return out;
}

function normalizeValue(value: unknown): unknown {
  if (typeof value === 'bigint') return Number(value);
  if (value === null || value === undefined) return null;
  if (Array.isArray(value)) return value.map(normalizeValue);
  // Arrow vectors and struct proxies both expose `toJSON`.
  if (
    typeof value === 'object' &&
    'toJSON' in value &&
    typeof value.toJSON === 'function'
  ) {
    return normalizeValue((value as { toJSON(): unknown }).toJSON());
  }
  return value;
}
