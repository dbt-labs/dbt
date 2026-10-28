# SQL with Trailing Comments Test

This test verifies that SQL queries with trailing comments work correctly across all adapters.

## Background

GitHub issue #468 was initially reported for Snowflake, where dbt-fusion threw "Empty SQL statement" 
errors when running SQL queries with trailing comments, while dbt-core handled them correctly. 

Investigation revealed this was a universal issue affecting all SQL dialects. Each dialect reported 
different but related errors when encountering comment-only statements that were created by SQL 
statement splitting when comments appeared after semicolons.

## What This Tests

1. **Original failing case**: `select 1; /* end comment */` 
2. **Trailing line comments**: `select 1; -- comment`
3. **Multiple trailing comments** with mixed comment types
4. **SQL with leading comments**: `/* comment */ select 1` (should NOT be filtered)
5. **SQL surrounded by comments**: `/* before */ select 1 /* after */` (should NOT be filtered)
6. **Multiple statements with comments**: Ensuring valid SQL is preserved
7. **Comments without semicolons** at the end

## Running the Tests

To run the tests for a specific adapter:

```bash
# Snowflake
ADAPTER_RECORD=1 GOLDIE_UPDATE=1 cargo xtask test -p dbt-adapter-tests adapters_snowflake_sql_with_trailing_comments -q

# BigQuery  
ADAPTER_RECORD=1 GOLDIE_UPDATE=1 cargo xtask test -p dbt-adapter-tests adapters_bigquery_sql_with_trailing_comments -q

# Databricks
ADAPTER_RECORD=1 GOLDIE_UPDATE=1 cargo xtask test -p dbt-adapter-tests adapters_databricks_sql_with_trailing_comments -q
```
