# custom_incremental_dynamic_table_no_change_tracking

A merge custom incremental dynamic table over a source without change tracking. Column discovery
queries the source's CHANGES before the dynamic table exists, so the first run fails with
Snowflake's change tracking error (091930). The goldie pins the message users see.
