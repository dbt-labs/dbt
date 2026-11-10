import { describe, expect, it } from 'vitest';

import { isParquetBytes } from './engine';

/**
 * The engine itself is not unit-testable — it dynamically imports DuckDB-WASM from a
 * runtime-computed CDN URL and needs a Worker — so what is tested here is the decision
 * that made a missing artifact look present: whether the fetched bytes are a parquet
 * file at all.
 */
describe('isParquetBytes', () => {
  const bytes = (...parts: (string | number[])[]): Uint8Array =>
    new Uint8Array(
      parts.flatMap((part) =>
        typeof part === 'string' ? [...part].map((c) => c.charCodeAt(0)) : part,
      ),
    );

  /** `PAR1` + a 4-byte footer length + `PAR1` — the smallest well-formed shape. */
  const minimal = bytes('PAR1', [0, 0, 0, 0], 'PAR1');

  it('accepts a file bracketed by the parquet magic', () => {
    expect(isParquetBytes(minimal)).toBe(true);
    expect(isParquetBytes(bytes('PAR1', 'some footer bytes here', 'PAR1'))).toBe(true);
  });

  it('rejects the SPA fallback document served for a missing artifact', () => {
    // What `dbt docs serve` — and any host that rewrites unknown paths — answers with.
    // Registering it is what produced `No magic bytes found at end of file`.
    const html = bytes(
      '<!doctype html>\n<html lang="en">\n  <head>\n  </head>\n</html>',
    );
    expect(isParquetBytes(html)).toBe(false);
  });

  it('rejects a file that lost its trailing magic', () => {
    // The end is what DuckDB reads first, so a truncated artifact must not register.
    expect(isParquetBytes(bytes('PAR1', 'footer bytes but no closing magic'))).toBe(
      false,
    );
  });

  it('rejects an empty or too-short response', () => {
    expect(isParquetBytes(new Uint8Array(0))).toBe(false);
    expect(isParquetBytes(bytes('PAR1'))).toBe(false);
    // Both magics present but overlapping: 8 bytes cannot hold a footer length.
    expect(isParquetBytes(bytes('PAR1', 'PAR1'))).toBe(false);
  });
});
