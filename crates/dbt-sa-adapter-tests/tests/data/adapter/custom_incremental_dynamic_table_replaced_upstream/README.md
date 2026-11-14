# custom_incremental_dynamic_table_replaced_upstream

A merge custom incremental dynamic table reading the changes of `stg_orders`, a `table` model
(`change_tracking=true`) that every dbt run replaces with CREATE OR REPLACE. The replacement has a
fresh change history, so the CI DT's next refresh reads all its rows as inserts. Updates and
inserts upstream arrive, but a row deleted upstream is never seen as deleted and stays in the CI DT.
The data test pins that.

Without `change_tracking=true` on `stg_orders`, the CI DT's first build already fails at column
discovery (see `custom_incremental_dynamic_table_no_change_tracking`), and a refresh after a replace
fails with Snowflake's change tracking error.
