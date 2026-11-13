# Databricks `use_materialization_v2` config tests

Verifies the three model config flags added for Databricks under `use_materialization_v2`.
Each flag only changes behavior against an **existing** relation, so `macros/setup.sql`
(`set_up` / `databricks__set_up`) pre-creates each relation in the run's `target.schema`
before the model run. The Rust test runs two turns:

```
run-operation set_up   # create v_view, t_incremental, t_table (pre-existing relations)
run                    # build all three models -> each exercises its config
```

## Configs

### `view_update_via_alter` (default: false) — `models/v_view.sql`
When true, a changed view is updated in place with `ALTER VIEW ... SET TBLPROPERTIES` /
`ALTER VIEW ... AS (...)` instead of `CREATE OR REPLACE VIEW`.

- `set_up` creates `v_view` as `select 1 as id, 'a' as val` with no tblproperties.
- The model keeps the same SELECT and adds `tblproperties={'test_property': 'test_value'}`.
- **Expect:** the run takes the alter branch and emits
  `ALTER VIEW ... SET tblproperties ('test_property' = 'test_value')` — no create-or-replace.

> Note: the view *definition* (SELECT body) component is not yet implemented in the Rust
> adapter (`query.rs` loaders return empty), so the SELECT is held constant and the change is
> driven through `tblproperties` (an implemented component).

### `incremental_apply_config_changes` (default: true) — `models/t_incremental.sql`
When true, an incremental run applies pending config diffs (tblproperties, tags, etc.) to the
existing table via `ALTER`. When false, those config changes are skipped.

- `set_up` creates `t_incremental` as `select 1 as id` with no tblproperties.
- The model is `materialized='incremental'`, `incremental_strategy='append'`, and adds
  `tblproperties={'test_property': 'test_value'}`.
- **Expect:** the incremental run applies the data change *and* emits
  `ALTER TABLE ... SET TBLPROPERTIES (...)` for the added property.

### `use_safer_relation_operations` (default: false) — `models/t_table.sql`
When true, replacing an existing relation uses a rename-based swap (create staging → rename
existing to backup → rename staging to target → drop backup) instead of `CREATE OR REPLACE`.

- `set_up` creates `t_table` as `select 1 as id`.
- The model is `materialized='table'` with `use_safer_relation_operations=true`.
- **Expect:** the run replaces the table via the rename-swap sequence rather than a single
  `CREATE OR REPLACE TABLE`.

## Defaults

All three defaults match the dbt-databricks (core) macros: `view_update_via_alter=false`,
`incremental_apply_config_changes=true`, `use_safer_relation_operations=false`.
