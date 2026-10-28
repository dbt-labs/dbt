# `adapter.add_time_ingestion_partition_column`

## Relevant adapters

Only:
- `dbt-bigquery`

## Run

```shell
dbt build
```

Then run a SQL query equivalent to one of these:

```shell
dbt show --inline "select _PARTITIONTIME from {{ ref('a') }}"
dbt show --inline "select _PARTITIONTIME, COUNT(*) AS row_count from {{ ref('a') }} GROUP BY _PARTITIONTIME"
```

**Note:**
This does **not** test `adapter.add_time_ingestion_partition_column` directly, but rather indirectly via the `partition_by.time_ingestion_partitioning` property that is used with the [`incremental` materialization within `dbt-bigquery`](https://github.com/dbt-labs/dbt-adapters/blob/1f8e0b62161230eaf688ff5413c7efa20dfd540e/dbt-bigquery/src/dbt/include/bigquery/macros/materializations/incremental.sql#L152).

## Expected output

A column named `_PARTITIONTIME` should be included in the output.

But there should **NOT** be a column named `_PARTITIONTIME` if you do this instead (because it is a pseudo column):
```shell
dbt show --inline "select * from {{ ref('a') }}"
```

## Source for adapter method

https://github.com/dbt-labs/dbt-adapters/blob/1f8e0b62161230eaf688ff5413c7efa20dfd540e/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L244

## Usage of adapter method

https://github.com/dbt-labs/dbt-adapters/blob/1f8e0b62161230eaf688ff5413c7efa20dfd540e/dbt-bigquery/src/dbt/include/bigquery/macros/materializations/incremental.sql#L152

## Jinja analog

None.

## Relevant pytest

- https://github.com/dbt-labs/dbt-adapters/blob/1f8e0b62161230eaf688ff5413c7efa20dfd540e/dbt-bigquery/tests/functional/adapter/incremental/test_incremental_strategies.py#L103-L109

## Original PR

https://github.com/dbt-labs/dbt-bigquery/pull/496

## Additional context

https://docs.getdbt.com/blog/bigquery-ingestion-time-partitioning-and-partition-copy-with-dbt
