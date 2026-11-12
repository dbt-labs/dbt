# custom_incremental_dynamic_table

Merge custom incremental dynamic tables (`refresh_mode='CUSTOM_INCREMENTAL'`) reading their
sources through `{{ changes() }}`.

- `orders_merge`: single `unique_key`, deletes propagated through `delete_condition`.
- `stock_merge`: composite `unique_key`, model starts with a CTE.
- `orders_transient`: a transient CI DT reading `changes(..., mode='append_only')`.

Turns (see `adapters_snowflake_custom_incremental_dynamic_table`):

1. set up the sources;
2. create (columns discovered through a temporary table);
3. apply source changes;
4. refresh (no config change: skip, then the manual refresh that `scheduler = 'DISABLE'` implies);
5. check the rows;
6. full refresh (`create or replace`);
7. check the rows again. Creating a CI DT reads the source's current rows as inserts rather than
   replaying its change history, which only shows in the APPEND_ONLY `orders_transient`;
8. change `stock_merge`'s `target_lag` (altered in place, like a regular dynamic table);
9. check `stock_merge`;
10. switch `orders_merge` to a regular dynamic table;
11. check it;
12. switch it back to a CI DT;
13. check it.
