# custom_incremental_dynamic_table_edge_cases

Snowflake behaviors a CI DT user runs into, one model each, pinned by the goldies (errors in stderr)
and `verify` (`[PASS]`/`[FAIL]` in stdout):

| Model | Turn | Behavior |
|---|---|---|
| `no_change_tracking` | first `run` | column discovery fails: 091930, change tracking not enabled |
| `self_reference` | first `run` | `{{ this }}`: column discovery fails, 002003, the table doesn't exist yet |
| `duplicate_keys` | second `run` | no per-key dedup: the refresh fails, 002742 "Duplicate row detected"; rows kept |
| `widened_column` | second `run` | source column widened (flag off): the refresh fails, 002742, value too long; rows kept |
| `over_replaced_table` | second `run` | reads `stg_orders`, a `table` model dbt replaces: an upstream delete is missed |
