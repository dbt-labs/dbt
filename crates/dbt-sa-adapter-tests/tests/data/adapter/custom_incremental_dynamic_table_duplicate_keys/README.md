# custom_incremental_dynamic_table_duplicate_keys

A merge custom incremental dynamic table that doesn't deduplicate its changes per key. An update
arrives as a DELETE and an INSERT of the same key, so the refresh after it merges two source rows
into one target row, and Snowflake fails it (002742 "Duplicate row detected during DML action").
The table keeps its rows, and later refreshes fail the same way until the model is fixed. The
goldie pins the message users see.
