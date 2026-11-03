# BigQuery snapshot `hard_deletes='new_record'` with nested columns

Record/replay coverage for the snapshot fix ported from
[dbt-adapters#1781](https://github.com/dbt-labs/dbt-adapters/pull/1781)
(tracked by [dbt-labs/fs#11850](https://github.com/dbt-labs/fs/issues/11850)).

It validates that a snapshot configured with `hard_deletes='new_record'` over a
source containing `STRUCT` and `ARRAY` columns builds correctly on BigQuery,
specifically that the `deletion_records` CTE projects nested columns by their
top-level names.

## What it does

The project has one source model and one snapshot:

- `models/source_data.sql`: a table with a `contact` `STRUCT`, a `tags` `ARRAY`,
  and an `updated_at` timestamp. A `drop_rows` var removes ids 4-5.
- `models/snapshots.yml`: `strategy: timestamp`, `hard_deletes: new_record`.

The test runs four commands:

1. `run` builds the source (ids 1-5).
2. `snapshot` creates the snapshot table (build path).
3. `run --vars {{ drop_rows: true }}` rebuilds the source without ids 4-5,
   hard-deleting them.
4. `snapshot` merges. Because ids 4-5 disappeared, the snapshot renders the
   `deletion_records` CTE over the nested columns.

## Why it matters

On BigQuery a `hard_deletes='new_record'` snapshot over `STRUCT`/`ARRAY` columns
fails without the fix:

- `get_column_schema_from_query` flattens `STRUCT` fields into names that cannot
  be projected from `source_data`.
- a bare `NULL` for a new column is inferred as `INT64`, incompatible with
  `STRUCT`/`ARRAY` columns in the snapshot `union all`.

Fusion has a third, adapter-layer manifestation: zero-row schema probes flatten
nested columns, so `get_columns_in_query` returns names like `contact/first` and
`tags.0` that are not real top-level columns. The macro then builds references to
columns that do not exist and BigQuery rejects the staging query.

The fix ports the upstream macro (use `get_columns_in_query` and select new
columns from `source_data` instead of bare `NULL`) and adds a Fusion-only agate
adjustment so zero-row probes keep nested columns as single top-level names.

## What the recording proves

In `golden/bigquery/run_3/recordings.yaml`, the second snapshot's
`deletion_records` CTE projects the nested columns by their top-level names:

```sql
snapshotted_data.`contact`, snapshotted_data.`tags`
```

If the agate flattening or the ported macro regressed, `get_columns_in_query`
would yield `contact/first` / `tags.0`, the generated SQL would change, and
replay's SQL comparison would fail the test. The recording was captured against
real BigQuery, which accepted the staging query, and the stdout goldie shows the
snapshot finishing successfully.

## Relevant adapters

- bigquery

## Running

```sh
cargo xtask test -p dbt-adapter-tests adapters_bigquery_snapshot_new_record
```

## Re-recording (requires BigQuery credentials)

```sh
cargo xtask init-creds
cargo xtask test -u --adapter-record adapters_bigquery_snapshot_new_record
```
