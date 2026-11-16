import {
  createEngine,
  type DuckDbEngine,
  type EngineOptions,
  isParquetBytes,
  type TableName,
} from 'dbt-docs-lib/duckdb';

function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function rejects(operation: Promise<unknown>, message?: string) {
  try {
    await operation;
  } catch (error) {
    if (message) check(String(error).includes(message), String(error));
    return;
  }
  throw new Error('Expected rejection');
}

/** Consumes only the built public package, with a real browser worker and parquet. */
export async function run(
  origin: string,
  bundlePreference?: EngineOptions['bundlePreference'],
) {
  const liveWorkers = new Set<Worker>();
  const OriginalWorker = globalThis.Worker;
  let started = 0;
  globalThis.Worker = class extends OriginalWorker {
    constructor(url: string | URL, options?: WorkerOptions) {
      super(url, options);
      started++;
      liveWorkers.add(this);
    }
    override terminate() {
      liveWorkers.delete(this);
      super.terminate();
    }
  };
  const options: EngineOptions = {
    dataBaseUrl: `${origin}/data/`,
    cdnBase: `${origin}/duckdb`,
    ...(bundlePreference === undefined ? {} : { bundlePreference }),
  };
  const tables: TableName[] = ['dbt.models', 'dbt.seeds'];
  const engine: DuckDbEngine = createEngine(options);
  try {
    check(started === 0, 'Construction must be lazy');
    await Promise.all([engine.ready(), engine.ready()]);
    check(Number(started) === 1, 'Readiness must share one worker');
    const [models, seeds, combined] = await Promise.all([
      engine.query<{ count: number }>(
        'SELECT count(*) AS count FROM dbt.models',
        tables,
      ),
      engine.query<{ count: number }>('SELECT count(*) AS count FROM dbt.seeds', [
        'dbt.seeds',
      ]),
      engine.query<{ count: number }>(
        'SELECT (SELECT count(*) FROM dbt.models) + (SELECT count(*) FROM dbt.seeds) AS count',
        tables,
      ),
    ]);
    check(models[0].count > 0, 'Real model parquet should contain rows');
    check(
      combined[0].count === models[0].count + seeds[0].count,
      'Concurrent query results',
    );
    check(
      engine.hasTable('dbt.models') && engine.hasTable('dbt.seeds'),
      'Registered tables',
    );
    const [normalized] = await engine.query<{ n: number; xs: number[] }>(
      'SELECT 42::BIGINT AS n, [1::BIGINT, 2::BIGINT] AS xs',
      [],
    );
    check(
      JSON.stringify(normalized) === '{"n":42,"xs":[1,2]}',
      'Normalized Arrow values',
    );
    await rejects(engine.query('SELECT * FROM nonexistent', []));
    await engine.query('SELECT 1', []);
    await Promise.all([engine.dispose(), engine.dispose()]);
    check(liveWorkers.size === 0, 'Disposal must terminate the worker');
    await rejects(engine.ready(), 'disposed');
    await rejects(engine.query('SELECT 1', []), 'disposed');

    const pending = createEngine({ ...options, dataBaseUrl: `${origin}/slow/` });
    const readiness = pending.ready();
    await new Promise((resolve) => setTimeout(resolve, 20));
    await Promise.all([rejects(readiness, 'disposed'), pending.dispose()]);
    check(liveWorkers.size === 0, 'Dispose during initialization must clean up');

    const flaky = createEngine({ ...options, dataBaseUrl: `${origin}/flaky/` });
    try {
      await rejects(flaky.ready(), '503');
      const [retried] = await flaky.query<{ count: number }>(
        'SELECT count(*) AS count FROM dbt.models',
        ['dbt.models'],
      );
      check(retried.count === models[0].count, 'A failed load must be retried');
    } finally {
      await flaky.dispose();
    }
    check(liveWorkers.size === 0, 'Retried initialization must release every worker');

    const stalled = createEngine({ ...options, dataBaseUrl: `${origin}/stalled/` });
    const stalledQuery = stalled.query('SELECT * FROM dbt.models', ['dbt.models']);
    await stalled.ready();
    await stalled.dispose();
    await rejects(stalledQuery, 'disposed');
    check(liveWorkers.size === 0, 'Dispose must not wait for a stalled fetch');

    const failed = createEngine({ ...options, dataBaseUrl: `${origin}/broken/` });
    await rejects(failed.ready(), 'unrecognized statement');
    await failed.dispose();
    check(liveWorkers.size === 0, 'Failed initialization must clean up');

    const missing = createEngine({ ...options, dataBaseUrl: `${origin}/missing/` });
    try {
      await rejects(missing.query('SELECT * FROM dbt.models', ['dbt.models']));
      check(
        !missing.hasTable('dbt.models'),
        'HTML fallback must not register as parquet',
      );
    } finally {
      await missing.dispose();
    }
    const bytes = new Uint8Array(
      await (await fetch(`${origin}/data/dbt.models.parquet`)).arrayBuffer(),
    );
    check(isParquetBytes(bytes), 'Public parquet validation');
    check(liveWorkers.size === 0, 'All workers released');
    return {
      modelCount: models[0].count,
      seedCount: seeds[0].count,
      workersStarted: started,
    };
  } finally {
    await engine.dispose();
    for (const worker of liveWorkers) worker.terminate();
    globalThis.Worker = OriginalWorker;
  }
}
