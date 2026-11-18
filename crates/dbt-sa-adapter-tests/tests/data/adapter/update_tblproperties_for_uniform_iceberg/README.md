# `adapter.update_tblproperties_for_uniform_iceberg`

## Relevant adapters

- `dbt-databricks`

## Run

```shell
dbt build
```

**Note:**
This does **not** test `adapter.update_tblproperties_for_uniform_iceberg` directly, but rather indirectly via the `table_format` property that is used with the [`tblproperties_clause` macro](https://github.com/search?q=repo%3Adatabricks%2Fdbt-databricks+tblproperties_clause&type=code) used within table/view/etc materializations in `dbt-databricks`.

## Expected output

We expect to see something like the following within the executed statements (e.g. `logs/dbt.log`):

```sql
create or replace table `hive_metastore`.`dbt_dbeatty`.`a`
using delta
tblproperties (
    'delta.enableIcebergCompatV2' = 'true',
    'delta.universalFormat.enabledFormats' = 'iceberg'
)

as

select 1 as id, 'hello iceberg' as intro
```

## Source for adapter method

https://github.com/databricks/dbt-databricks/blob/ab6065b54222254fb59c35effa95da1f7070d625/dbt/adapters/databricks/impl.py#L187

## Usage of adapter method

This adapter method is _only_ in `dbt-databricks` (even though other) adapters support the Iceberg format:
https://github.com/databricks/dbt-databricks/blob/ab6065b54222254fb59c35effa95da1f7070d625/dbt/include/databricks/macros/relations/tblproperties.sql#L6

:point_right: This appears to be an internal implementation detail chosen by the maintainers of `dbt-databricks` rather than an adapter interface established by dbt Labs.

## Jinja analog

None.

## Relevant pytest

- https://github.com/databricks/dbt-databricks/blob/ab6065b54222254fb59c35effa95da1f7070d625/tests/unit/macros/relations/test_table_macros.py#L47
- https://github.com/databricks/dbt-databricks/blob/ab6065b54222254fb59c35effa95da1f7070d625/tests/unit/macros/relations/test_table_macros.py#L213
- https://github.com/databricks/dbt-databricks/blob/ab6065b54222254fb59c35effa95da1f7070d625/tests/unit/macros/relations/test_table_macros.py#L241

## Original PR

https://github.com/databricks/dbt-databricks/pull/815

## Additional context

https://docs.getdbt.com/blog/icebeg-is-an-implementation-detail
