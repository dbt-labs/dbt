# custom_incremental_dynamic_table

Merge custom incremental dynamic tables (`refresh_mode='CUSTOM_INCREMENTAL'`) reading their
sources through `{{ changes() }}`.

- `orders_merge`: single `unique_key`, deletes propagated through `delete_condition`.
- `stock_merge`: composite `unique_key`, model starts with a CTE.
- `orders_transient`: a transient CI DT reading `changes(..., mode='append_only')`.

Turns (see `adapters_snowflake_custom_incremental_dynamic_table`): set up the sources, create
(columns discovered through a temporary table), apply source changes, refresh (no config change:
skip, then the manual refresh `scheduler = 'DISABLE'` implies), check the rows, full refresh
(`create or replace`) and check again, change `stock_merge`'s `target_lag` (replaced), switch `orders_merge` to a regular dynamic table and back.
