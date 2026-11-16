# custom_incremental_dynamic_table_widened_column

A merge custom incremental dynamic table whose source column is widened after the CI DT exists
(`varchar(8)` to `varchar(32)`). Column discovery declared `VARCHAR(8)`.

- With `snowflake_dynamic_table_create_or_alter` off, the build is a no-op and the refresh fails:
  `002742 ... String '...' is too long and would be truncated`. The CI DT keeps its rows;
  `--full-refresh` rebuilds it with the new type.
- With the flag on, the next build rediscovers `VARCHAR(32)` and CREATE OR ALTER widens the column
  in place; the refresh then brings the long value, without a rebuild.
