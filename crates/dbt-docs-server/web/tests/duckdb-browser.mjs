import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { build } from 'vite';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);
const runtimeDist = dirname(require.resolve('@duckdb/duckdb-wasm'));
const fixtures = resolve(root, 'src/test/fixtures');
const publicEntry = fileURLToPath(import.meta.resolve('dbt-docs-lib/duckdb'));
const lib = resolve(root, '../dbt-docs-lib');
assert.equal(publicEntry, resolve(lib, 'dist/duckdb.es.js'));
assert.equal(typeof require('dbt-docs-lib/duckdb').createEngine, 'function');

async function bundle(entry) {
  const result = await build({
    configFile: false,
    logLevel: 'error',
    build: { write: false, lib: { entry, formats: ['es'], fileName: 'consumer' } },
  });
  const output = Array.isArray(result) ? result[0] : result;
  return output.output.find((chunk) => chunk.type === 'chunk').code;
}

const consumer = await bundle(resolve(root, 'tests/duckdb-consumer.ts'));
const runtime = await bundle(resolve(runtimeDist, 'duckdb-browser.mjs'));
assert(!/react|\.css/.test(consumer), 'The data entry must not import UI or styles');
const views = await readFile(
  resolve(lib, 'lib/data-sources/duckdb/fixtures/views.sql'),
);
const runtimeRequests = [];
let flakyViewsRequests = 0;
const server = createServer(async (request, response) => {
  const url = new URL(request.url, 'http://localhost');
  try {
    if (url.pathname === '/') {
      response.setHeader('Content-Type', 'text/html');
      response.end('<!doctype html><title>Public DuckDB consumer</title>');
    } else if (url.pathname === '/consumer.js' || url.pathname === '/duckdb/+esm') {
      response.setHeader('Content-Type', 'text/javascript');
      response.end(url.pathname === '/consumer.js' ? consumer : runtime);
    } else if (
      /^\/duckdb\/dist\/duckdb-(mvp|eh)\.(wasm)$/.test(url.pathname) ||
      /^\/duckdb\/dist\/duckdb-browser-(mvp|eh)\.worker\.js$/.test(url.pathname)
    ) {
      runtimeRequests.push(url.pathname.split('/').pop());
      response.setHeader(
        'Content-Type',
        url.pathname.endsWith('.wasm') ? 'application/wasm' : 'text/javascript',
      );
      response.end(await readFile(resolve(runtimeDist, url.pathname.split('/').pop())));
    } else if (url.pathname === '/flaky/views.sql' && flakyViewsRequests++ % 2 === 0) {
      // Fails every other request, so each engine's first load fails and its retry succeeds.
      response.writeHead(503).end();
    } else if (url.pathname === '/stalled/dbt.models.parquet') {
      // Never answers: an artifact fetch that disposal must not wait for.
    } else if (
      /^\/(data|slow|broken|missing|flaky|stalled)\/views\.sql$/.test(url.pathname)
    ) {
      if (url.pathname.startsWith('/slow/'))
        await new Promise((r) => setTimeout(r, 100));
      response.end(url.pathname.startsWith('/broken/') ? 'INVALID DDL;' : views);
    } else if (/^\/(data|flaky)\/dbt\.(models|seeds)\.parquet$/.test(url.pathname)) {
      response.end(
        await readFile(resolve(fixtures, 'parquet', url.pathname.split('/').pop())),
      );
    } else if (url.pathname.startsWith('/missing/')) {
      response.end('<!doctype html><html>Missing artifact</html>');
    } else {
      response.writeHead(404).end();
    }
  } catch (error) {
    response.writeHead(500).end(String(error));
  }
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser;
try {
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(origin);
  const detected = await page.evaluate(async () => {
    const wasm = await import('/duckdb/+esm');
    const selected = await wasm.selectBundle(wasm.getJsDelivrBundles());
    return {
      module: selected.mainModule.split('/').pop(),
      worker: selected.mainWorker.split('/').pop(),
    };
  });
  const results = {};
  for (const [label, preference] of [
    ['omitted', undefined],
    ['auto', 'auto'],
    ['mvp', 'mvp'],
  ]) {
    runtimeRequests.length = 0;
    results[label] = await page.evaluate(
      async ({ base, preference }) => {
        const { run } = await import('/consumer.js');
        return run(base, preference);
      },
      { base: origin, preference },
    );
    const expected =
      preference === 'mvp'
        ? ['duckdb-mvp.wasm', 'duckdb-browser-mvp.worker.js']
        : [detected.module, detected.worker];
    assert.deepEqual(
      [...new Set(runtimeRequests)].sort(),
      expected.sort(),
      `${label}: matching worker/WASM pair`,
    );
  }
  assert.deepEqual(
    results.auto,
    results.omitted,
    'Explicit auto preserves the default',
  );
  assert.deepEqual(
    results.mvp,
    results.omitted,
    'MVP preserves query and lifecycle results',
  );
  assert.deepEqual(errors, [], 'No unhandled browser errors');
  process.stdout.write(`${JSON.stringify({ status: 'success', detected, results })}\n`);
} finally {
  await browser?.close();
  // The stalled artifact's request is never answered.
  server.closeAllConnections();
  await new Promise((r, reject) =>
    server.close((error) => (error ? reject(error) : r())),
  );
}
