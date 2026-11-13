## What this tests

This test project verifies that the `snowflake_warehouse` configuration option works for:
1. **Models** - via `custom_wh.sql` which sets `snowflake_warehouse='DBT_TESTING_ALT'` in the model config
2. **Data tests** - via `schema.yml` which sets `snowflake_warehouse: DBT_TESTING_ALT` in the data test config

Related issues:
- https://github.com/dbt-labs/fs/issues/4021 (models)
- https://github.com/dbt-labs/dbt-fusion/issues/1088 (data tests)

## How verification works

The recorded SQL sequence in `recordings.yaml` captures the `USE WAREHOUSE` statements, proving that:
1. `custom_wh` model switches to the alt warehouse before DDL and back to default after
2. `default_wh` model uses the default warehouse (no switch)
3. The `not_null` test on `custom_wh` switches to the alt warehouse before running