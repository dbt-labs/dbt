# `nest_column_data_types()`

## overview
Today we dbt Core Model Contracts do not support complex/nested types because complex type support is not robust across data platforms (see [dbt-core#7759](https://github.com/dbt-labs/dbt-core/issues/7759)).

However they are implemented for dbt-bigquery(see [dbt-bigquery#673](https://github.com/dbt-labs/dbt-bigquery/issues/673))

`BigQueryAdapter.nest_column_data_types()` is really a wrapper around `BigQueryCoulmn.get_nested_column_data_types()` (https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/column.py#L131)


For all the test cases actually needed, please see [dbt-adapters/dbt-bigquery/tests/unit/test_column.py](https://github.com/dbt-labs/dbt-adapters/blob/ed59a9a45749ce4e143038c91325947b8881edcf/dbt-bigquery/tests/unit/test_column.py).

## to test
1. setup
    1. define a model `a.sql` with at least one column type that's nested
    1. in YAML document `a.sql`'s nested column
2. at the top of a dummy model A: include jinja at the top that
    1. fetches the models column documentation from YAML
    2. invokes `adapter.nest_column_data_types`
    3. logs the result
3. rinse and repeat for all the test cases

## relevant adapters
- bigquery

## run

```shell
dbt build --full-refresh
```

## expected output

Log output that includes the following:

```
columns_nested: {'full_name': {'name': 'full_name', 'data_type': 'struct<firstname, lastname>'}}
```

## example output

```
$ dbt build --profile bigquery --full-refresh
16:09:04  Running with dbt=1.9.1
16:09:05  Registered adapter: bigquery=1.9.1
16:09:05  Found 1 model, 489 macros
16:09:05  
16:09:05  Concurrency: 10 threads (target='bigquery')
16:09:05  
16:10:08  1 of 1 START sql view model dbt_dbeatty_dev.a .................................. [RUN]
16:10:08  columns_nested: {'full_name': {'name': 'full_name', 'data_type': 'struct<firstname, lastname>'}}
16:10:10  1 of 1 OK created sql view model dbt_dbeatty_dev.a ............................. [CREATE VIEW (0 processed) in 1.81s]
16:10:10  
16:10:10  Finished running 1 view model in 0 hours 1 minutes and 4.70 seconds (64.70s).
16:10:10  
16:10:10  Completed successfully
16:10:10  
16:10:10  Done. PASS=1 WARN=0 ERROR=0 SKIP=0 TOTAL=1
```
