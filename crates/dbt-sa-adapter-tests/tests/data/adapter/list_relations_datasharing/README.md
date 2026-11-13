Mirrors `get_relation/`, but enables `datasharing: true` via `_profiles.override.yml` so the Rust `redshift::list_relations` takes the `SHOW TABLES FROM SCHEMA <db>.<schema>` branch instead of the default `information_schema.tables` path.

The model calls `adapter.get_relation(...)`, which on cache miss invokes `list_relations` to populate the cache. The setup macro creates both a TABLE and a VIEW so the recording captures `table_subtype` rows for both kinds.
