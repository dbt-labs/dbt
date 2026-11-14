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
6. apply a second round of source changes (re-insert the deleted order, update the inserted one);
7. refresh;
8. check `orders_merge`;
9. full refresh (`create or replace`);
10. check the rows again. Creating a CI DT reads the source's current rows as inserts rather than
   replaying its change history, which only shows in the APPEND_ONLY `orders_transient`;
11. change `stock_merge`'s `target_lag` (altered in place, like a regular dynamic table);
12. check `stock_merge`;
13. switch `orders_merge` to a regular dynamic table;
14. check it;
15. switch it back to a CI DT;
16. check it.
