# Snowflake Dynamic Table Materialization Test

This test ensures that dbt models with the `dynamic_table` materialization are created, altered, and replaced correctly across a range of configuration options.

Models are organized into subfolders by the config property under test. Runs 2 and 3 use `FileOverride` entries in the test to swap in a different `dbt_project.yml` from `_project_overrides/`.

## Runs

```shell
dbt run-operation set_up   # run_0: create source table
dbt run                    # run_1: CREATE all dynamic tables
dbt run                    # run_2: ALTER / implicit REPLACE via _project_overrides/run_2.yml
dbt run --full-refresh     # run_3: explicit REPLACE via _project_overrides/run_3.yml
```

## Models

### `initialization_warehouse/`
| Model | run_1 | run_2 | run_3 |
|-------|-------|-------|-------|
| `init_wh` | CREATE with init warehouse = FUSION_SLT_WAREHOUSE | ALTER init warehouse: FUSION_SLT_WAREHOUSE -> FUSION_SLT_WAREHOUSE | CREATE OR REPLACE with init warehouse = FUSION_SLT_WAREHOUSE |
| `no_init_wh` | CREATE without init warehouse | ALTER init warehouse: unset -> FUSION_SLT_WAREHOUSE | CREATE OR REPLACE without init warehouse |
| `same_wh` | CREATE with init warehouse set (to be unset) | ALTER init warehouse: set -> unset | CREATE OR REPLACE with init warehouse set |

### `target_lag/`
| Model | run_1 | run_2 | run_3 |
|-------|-------|-------|-------|
| `lag` | CREATE with `target_lag = 1 hour` | ALTER target lag: 1 hour -> downstream | CREATE OR REPLACE with `target_lag = 1 hour` |

### `refresh_mode/`
| Model | run_1 | run_2 | run_3 |
|-------|-------|-------|-------|
| `refresh_auto` | CREATE with `refresh_mode = AUTO` | AUTO -> FULL: triggers implicit CREATE OR REPLACE | CREATE OR REPLACE with `refresh_mode = AUTO` |
| `refresh_full` | CREATE with `refresh_mode = FULL` | FULL -> AUTO: no-op (no change detected by design) | CREATE OR REPLACE with `refresh_mode = FULL` |

### `initialize/`
| Model | run_1 | run_2 | run_3 |
|-------|-------|-------|-------|
| `init_default` | CREATE with `initialize = ON_CREATE` | no-op (initialize is one-time only) | CREATE OR REPLACE with `initialize = ON_CREATE` |
| `init_on_schedule` | CREATE with `initialize = ON_SCHEDULE` | no-op (initialize is one-time only) | CREATE OR REPLACE with `initialize = ON_SCHEDULE` |

### `iceberg/`
| Model | run_1 | run_2 | run_3 |
|-------|-------|-------|-------|
| `dt_iceberg_managed` | CREATE with `external_volume = 'SNOWFLAKE_MANAGED'`, no `BASE_LOCATION` | ALTER target_lag: 1 hour -> downstream, no `BASE_LOCATION` | CREATE OR REPLACE with `external_volume = 'SNOWFLAKE_MANAGED'`, no `BASE_LOCATION` |
| `dt_iceberg_external_volume` | CREATE with `BASE_LOCATION` | ALTER target_lag: 1 hour -> downstream, `BASE_LOCATION` unchanged | CREATE OR REPLACE with `BASE_LOCATION` |
| `dt_iceberg_explicit_snowflake_managed` | CREATE with `external_volume = 'SNOWFLAKE_MANAGED'`, no `BASE_LOCATION` | ALTER target_lag: 1 hour -> downstream, no `BASE_LOCATION` | CREATE OR REPLACE with `external_volume = 'SNOWFLAKE_MANAGED'`, no `BASE_LOCATION` |
