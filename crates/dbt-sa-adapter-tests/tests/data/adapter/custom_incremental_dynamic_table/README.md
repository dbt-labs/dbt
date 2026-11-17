# custom_incremental_dynamic_table

Merge custom incremental dynamic tables (`refresh_mode='CUSTOM_INCREMENTAL'`) with
`snowflake_dynamic_table_create_or_alter` off. One model per scenario; the turns are shared:

| Model | Scenario |
|---|---|
| `orders_merge` | single `unique_key`, `delete_condition`; a delete then re-insert of the same key, an update of a key it inserted |
| `stock_merge` | composite `unique_key`, a model starting with a CTE |
| `orders_transient` | transient, `append_only`; creating a CI DT reads the source's current rows as inserts |
| `orders_with_region` | `{{ changes() }}` joined to an ephemeral model |
| `orders_attrs` | `cluster_by`, `target_lag`, `table_tag`, `row_access_policy`, initialization warehouse, `copy_grants`, `persist_docs`; ALTER on a config change |
| `orders_switch` | switched to a regular dynamic table and back (replaced) |
| `orders_empty` | built by `dbt run --empty`, stays empty until `--full-refresh` |

Turns (see `adapters_snowflake_custom_incremental_dynamic_table`): set up; `run --empty` for
`orders_empty`; create; round 1 of source changes; a run with config changes; `verify`; round 2; a
run switching back; `verify`; `--full-refresh`; `verify`. `verify` prints `[PASS]`/`[FAIL]` per
expectation.
