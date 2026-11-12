# custom_incremental_dynamic_table_changes_forms

Merge custom incremental dynamic tables reading their change source three ways: `{{ changes() }}`
(`orders_macro`), CHANGES written by hand after `source()` (`orders_hand_written`), and
`{{ changes() }}` joined to an ephemeral model (`orders_with_region`).

See `adapters_snowflake_custom_incremental_dynamic_table_changes_forms`: `dbt run --empty` builds
all three (column discovery included), `--full-refresh` rebuilds them with the real bodies, then
source DML, a refresh, and row checks.
