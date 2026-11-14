# custom_incremental_dynamic_table_self_reference

A merge custom incremental dynamic table that reads itself through `{{ this }}`, which isn't
supported yet: inside `refresh using`, Snowflake only accepts the table as SELF. dbt doesn't check
for it; the goldies pin the errors Snowflake gives:

- first build: column discovery fails, because the table doesn't exist yet (002003);
- rebuild of an existing CI DT (`--full-refresh`): the DDL fails with "References to the dynamic
  table being created in REFRESH USING must use SELF" (001002).
