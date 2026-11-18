# Databricks Materialized View Tests

This directory contains tests for validating the behavior of materialized views. It tests proper creation, altering (apply/fail) and full refresh.

## Test Scenarios

- **Create:**  
  When a materialized view is created, the test expects the view to be present in the target schema with the correct structure and initial data.

- **Configuration Change - Apply:**  
  If a configuration change is detected and `on_configuration_change: apply` is set, the materialized view should be updated to reflect the new configuration. The test expects the view to be altered successfully and the changes to be visible in the metadata and data.

- **Configuration Change - Fail:**  
  If a configuration change is detected and `on_configuration_change: fail` is set, the test expects dbt to raise a compilation or runtime error, and the materialized view should remain unchanged.

- **Full Refresh:**  
  When a full refresh is triggered, the materialized view is replaced in place with `CREATE OR REPLACE MATERIALIZED VIEW`, including with `use_safer_relation_operations: true` and when `partition_by` is added or changed on a view without a Compatibility Mode location. It is dropped and recreated only when `partition_by` is removed (which `CREATE OR REPLACE` silently ignores) and the recreated view has no Compatibility Mode location. A Compatibility Mode location change, or a partitioning change whose recreated view would keep a location, is refused before any DDL so the view is never dropped and lost. The test expects the view to be rebuilt with fresh data and updated metadata.

## Expected Output

- Successful creation and alteration of materialized views when allowed.
- Error messages when configuration changes are set to `fail`.
- Verification of view existence, structure, and data after each operation.
- dbt logs and test assertions confirming the correct behavior for each scenario.

## Usage

This test can be run with the usual test suite like so:
```
cargo xtask test -p dbt-adapter-tests adapters_databricks_materialized_view
```