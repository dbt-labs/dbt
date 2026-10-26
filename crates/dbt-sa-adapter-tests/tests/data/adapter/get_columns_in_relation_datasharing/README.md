Mirrors `get_columns_in_relation/models/b.sql`, but enables `datasharing: true` via `_profiles.override.yml` so `redshift__get_columns_in_relation` takes the `SHOW COLUMNS FROM TABLE <db>.<schema>.<table>` branch instead of the legacy 3-way `information_schema` UNION.

The model calls `adapter.get_columns_in_relation(...)` on a fully qualified relation (`database`, `schema`, `identifier` all set), which is the path gated by `adapter.has_feature("datasharing")`.
