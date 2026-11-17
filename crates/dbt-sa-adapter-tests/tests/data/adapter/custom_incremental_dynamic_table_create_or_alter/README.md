# custom_incremental_dynamic_table_create_or_alter

Custom incremental dynamic tables with `snowflake_dynamic_table_create_or_alter` on: every build
re-declares the CI DT with CREATE OR ALTER, columns included. One model per scenario; `vars.evolved`
(the `evolved.yml` override) switches each to its changed form:

| Model | Change | Result |
|---|---|---|
| `body_edit` | appends a column | synced in place; applies to changes made after it, rows already merged keep NULL |
| `attrs_kept` | `cluster_by` | changed in place; the tag, policy and grant set out of band are kept |
| `ok_widen`, `ok_drop` | widens / drops a column | in place |
| `err_reorder`, `err_type_narrow`, `err_type_change` | reorders / narrows / changes a type | Snowflake rejects it (002830, 040050, 002108) and dbt appends its `--full-refresh` hint; `--full-refresh` recovers |

`verify` prints `[PASS]`/`[FAIL]` per expectation.
