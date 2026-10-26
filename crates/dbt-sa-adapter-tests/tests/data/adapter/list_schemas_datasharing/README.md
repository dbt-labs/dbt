Mirrors `list_schemas/`, but enables `datasharing: true` via `_profiles.override.yml` so the Redshift macro takes the `SHOW SCHEMAS FROM DATABASE ...` branch instead of the default `select distinct nspname from pg_namespace` (postgres) path.

The model and setup macro are unchanged — `adapter.list_schemas(target.database)` is what fires the SQL.
