# redshift_incremental_merge_schema_change

E2E coverage for two Redshift adapter macros that lack existing tests:

1. **`redshift__get_merge_sql`** — refactored to call `_update_predicates`
   for `incremental_predicates` that reference `DBT_INTERNAL_DEST.`.
2. **`redshift__alter_column_type`** (new) and
   **`redshift__alter_relation_add_remove_columns`** — the former dispatches
   on varchar widening; the latter now emits `column.quoted` instead of
   `column.name`.

## Turns

| Run | Command | File override | What it exercises |
| --- | --- | --- | --- |
| `run_0` | `run` | — | Creates `source` table + initial `target` (`create table as`). |
| `run_1` | `run` | `models/target.sql` ← `_project_overrides/target_v2.sql` | Under `on_schema_change=sync_all_columns`, widens `name` and adds `amount`. Then merges with `incremental_predicates=['DBT_INTERNAL_DEST.id > 0']`, whose rewrite is visible in the recorded `merge into ... on (...)`. |

## Recording

```
cargo xtask test -u --adapter-record adapters_redshift_incremental_merge_schema_change
```
