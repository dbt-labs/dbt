# Information Schema Test

Tests information_schema functionality across multiple adapters (BigQuery, Snowflake, Databricks).

## Test Coverage

Tests the `relation.information_schema()` method that was previously causing a panic with "not yet implemented: InformationSchema" for BigQuery and Databricks.

### Test Setup

Each test first creates a real table (`info_schema_test_table`) using adapter-specific syntax, then queries information_schema views to verify the table metadata is accessible.

### Scenarios Tested

1. **test_information_schema.sql**: 
   - Creates a test table
   - Queries INFORMATION_SCHEMA.TABLES to verify table exists
   - Returns all tables in the test schema

2. **test_columns_info.sql**:
   - Creates the same test table
   - Queries INFORMATION_SCHEMA.COLUMNS to get column metadata
   - Verifies column count and returns column details

## Running the Tests

To run in capture mode and generate golden files:
```bash
# BigQuery
ADAPTER_RECORD=1 GOLDIE_UPDATE=1 cargo xtask test -p dbt-adapter-tests adapters_bigquery_information_schema

# Snowflake  
ADAPTER_RECORD=1 GOLDIE_UPDATE=1 cargo xtask test -p dbt-adapter-tests adapters_snowflake_information_schema

# Databricks
ADAPTER_RECORD=1 GOLDIE_UPDATE=1 cargo xtask test -p dbt-adapter-tests adapters_databricks_information_schema
```
