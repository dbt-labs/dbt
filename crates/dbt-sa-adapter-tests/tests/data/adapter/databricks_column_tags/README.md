# Databricks Column Tags Test

This test validates that column-level `databricks_tags` are properly applied to Databricks tables, views, and materialized views.

## Test Coverage

This test covers:
1. **Table materialization** - Column tags on a regular table
2. **View materialization** - Column tags on a view
3. **Materialized View materialization** - Column tags on a materialized view
4. **Tag updates** - Verifying that tag values can be updated on subsequent runs
