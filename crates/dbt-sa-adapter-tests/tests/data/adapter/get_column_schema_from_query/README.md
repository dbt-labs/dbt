# `adapter.get_column_schema_from_query`

## Relevant adapters

- all adapters

## Run

```shell
dbt show --select a --quiet
```

## Expected output

We expect to see something like the following in standard out:

```
| data_type |
| --------- |
| INTEGER   |
```

## Source for adapter method

This is a metadata method ([source](https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/impl.py#L418)) that is **not** designed to be used directly within a dbt project.

## Usage of adapter method

- https://github.com/dbt-labs/dbt-adapters/blob/01055d709544ff06793af3c4dca37d653147be44/dbt-adapters/src/dbt/include/global_project/macros/relations/column/columns_spec_ddl.sql#L44
- https://github.com/dbt-labs/dbt-adapters/blob/01055d709544ff06793af3c4dca37d653147be44/dbt-adapters/src/dbt/include/global_project/macros/adapters/timestamps.sql#L21
- https://github.com/dbt-labs/dbt-adapters/blob/01055d709544ff06793af3c4dca37d653147be44/dbt-adapters/src/dbt/include/global_project/macros/relations/column/columns_spec_ddl.sql#L44
- https://github.com/dbt-labs/dbt-adapters/blob/01055d709544ff06793af3c4dca37d653147be44/dbt-adapters/src/dbt/include/global_project/macros/materializations/snapshots/helpers.sql#L158
- https://github.com/dbt-labs/dbt-adapters/blob/01055d709544ff06793af3c4dca37d653147be44/dbt-adapters/src/dbt/include/global_project/macros/materializations/snapshots/helpers.sql#L239

## Jinja analog

Its usage lives in a `macro get_column_schema_from_query(select_sql, select_sql_header=none)` ([source](https://github.com/dbt-labs/dbt-adapters/blob/01055d709544ff06793af3c4dca37d653147be44/dbt-adapters/src/dbt/include/global_project/macros/adapters/columns.sql#L65)), that wraps a input sql to query the schema of it.

## Relevant pytest

None known.
