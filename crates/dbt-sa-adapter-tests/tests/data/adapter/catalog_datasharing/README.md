Mirrors `test_project_jimmy`'s `run --write-catalog`, but enables `datasharing: true` via `_profiles.override.yml` so the Redshift catalog macros (`get_catalog` / `get_catalog_relations`) take the `SHOW TABLES FROM SCHEMA` + `svv_redshift_columns` path, joined in Rust by `build_catalog_from_show_tables_and_svv_columns`, instead of the legacy `pg_catalog` queries.

`run` materializes a table and a view; `--write-catalog` then generates `catalog.json` for them, so the recording captures the `SHOW TABLES` rows (TABLE and VIEW subtypes) and the `svv_redshift_columns` rows that the join consumes.
