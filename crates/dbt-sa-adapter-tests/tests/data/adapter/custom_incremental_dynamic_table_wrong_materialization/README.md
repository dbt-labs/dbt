# custom_incremental_dynamic_table_wrong_materialization

`refresh_mode='CUSTOM_INCREMENTAL'` on a `table` model. Only the `dynamic_table` materialization
builds a custom incremental dynamic table, so dbt warns when it resolves the model
(`CustomIncrementalOutsideDynamicTable`, dbt1111) and builds a regular table. The goldie pins the
message.
