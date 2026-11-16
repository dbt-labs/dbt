# custom_incremental_dynamic_table_create_or_alter_errors

Column changes on existing custom incremental dynamic tables with
`snowflake_dynamic_table_create_or_alter` on, like `dynamic_table_create_or_alter_errors` for
regular dynamic tables. A CI DT declares its columns, so CREATE OR ALTER re-declares them:

- `err_reorder`, `err_type_narrow`, `err_type_change`: Snowflake rejects the change (002830,
  040050, 002108), and dbt appends its `--full-refresh` hint. `--full-refresh` recovers.
- `ok_drop_column`: Snowflake drops the column in place, with its data.

`show_columns` prints each table's columns after the build and after the recovery.
