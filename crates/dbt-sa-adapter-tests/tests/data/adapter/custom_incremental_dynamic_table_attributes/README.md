# custom_incremental_dynamic_table_attributes

A merge custom incremental dynamic table with the dynamic table attributes dbt sets at creation:
`cluster_by`, `table_tag`, `row_access_policy`, `snowflake_initialization_warehouse`, `copy_grants`,
and a comment from `persist_docs`. Plus a SELECT grant dbt doesn't manage.

1. create;
2. a `cluster_by` change with `snowflake_dynamic_table_create_or_alter` off: ALTER;
3. `--full-refresh`: CREATE OR REPLACE ... COPY GRANTS keeps the grant and re-applies tag and policy;
4. a `cluster_by` change with the flag on: CREATE OR ALTER changes it and keeps the tag, the policy
   and the grant (Snowflake rejects tag and policy clauses in CREATE OR ALTER, so dbt omits them).

`verify` prints a `[PASS]`/`[FAIL]` line per attribute after each step.
