import { describe, expect, it } from 'vitest';

// The real document, copied verbatim out of a `--generate-info-schema` run.
// Parsing a hand-written approximation would prove nothing: the point of
// reading `views.sql` at all is that the generator owns its shape.
import realViewsSql from '../../../test/fixtures/info-schema/views.sql?raw';
import { parseViewsSql, splitStatements } from './viewsSql';

describe('splitStatements', () => {
  it('splits on semicolons and drops comments', () => {
    expect(splitStatements('-- a comment\nSELECT 1;\nSELECT 2;\n')).toEqual([
      'SELECT 1',
      'SELECT 2',
    ]);
  });

  it('leaves a comment marker inside a string literal alone', () => {
    expect(splitStatements("SELECT '-- not a comment';")).toEqual([
      "SELECT '-- not a comment'",
    ]);
  });

  it('does not let an escaped quote close the string', () => {
    // `''` is one escaped quote, so the string is still open at `--` and the
    // whole literal survives. Treating the pair as two delimiters would strip
    // from `--` onwards and produce unbalanced SQL.
    expect(splitStatements("SELECT 'it''s -- fine';")).toEqual([
      "SELECT 'it''s -- fine'",
    ]);
  });
});

describe('parseViewsSql', () => {
  const doc = parseViewsSql(realViewsSql);

  it('collects the schema statements', () => {
    expect(doc.schemas).toEqual([
      'CREATE SCHEMA IF NOT EXISTS dbt',
      'CREATE SCHEMA IF NOT EXISTS dbt_rt',
      'CREATE SCHEMA IF NOT EXISTS dbt_internal',
    ]);
  });

  it('reads each base view as its view name plus the artifact it needs', () => {
    const models = doc.views.get('dbt.models');
    expect(models).toMatchObject({
      kind: 'base',
      name: 'dbt.models',
      file: 'dbt.models.parquet',
    });
    // Renamed relative to the index, which is the whole reason the app cannot
    // keep guessing these names.
    expect(doc.views.get('dbt.docs_blocks')?.kind).toBe('base');
    expect(doc.views.get('dbt_rt.freshness')?.kind).toBe('base');
    expect(doc.views.has('dbt.nodes')).toBe(false);
  });

  it('registers dbt_internal views too', () => {
    expect(doc.views.get('dbt_internal.node_input_files')?.kind).toBe('base');
  });

  it('resolves a derived view to the base views it reads', () => {
    const latest = doc.views.get('dbt_rt.run_results_latest');
    expect(latest?.kind).toBe('derived');
    // Exactly its one prerequisite: `dbt_rt.run_results`, and not itself.
    expect(latest?.kind === 'derived' && latest.dependsOn).toEqual([
      'dbt_rt.run_results',
    ]);
  });

  it('resolves the resource union to every branch it unions', () => {
    const resources = doc.views.get('dbt_internal.resources');
    expect(resources?.kind).toBe('derived');
    const dependsOn = resources?.kind === 'derived' ? resources.dependsOn : [];
    expect([...dependsOn].sort()).toEqual([
      'dbt.analyses',
      'dbt.checks',
      'dbt.data_tests',
      'dbt.functions',
      'dbt.hooks',
      'dbt.models',
      'dbt.seeds',
      'dbt.snapshots',
      'dbt.sources',
    ]);
  });

  it('rejects a statement it does not recognize', () => {
    // The document is generated, so an unknown statement means the generator
    // and this parser have diverged. Skipping it would surface much later as a
    // missing relation with nothing pointing at the cause.
    expect(() => parseViewsSql('INSERT INTO dbt.models VALUES (1);')).toThrow(
      /unrecognized statement/,
    );
  });
});
