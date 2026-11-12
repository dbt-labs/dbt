# custom_incremental_dynamic_table_create_or_alter

A merge custom incremental dynamic table with `snowflake_dynamic_table_create_or_alter` on.
See `adapters_snowflake_custom_incremental_dynamic_table_create_or_alter` for the turns: create;
source DML; a CREATE OR ALTER sync of the unchanged body plus refresh; a body edit (appends
`amount_cents`) deployed by CREATE OR ALTER, which applies to changes made after it; a switch to a
regular dynamic table and back, which replaces. Rows are checked after each step.
