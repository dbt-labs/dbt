This project exercises the Redshift `datasharing` profile config end-to-end. It verifies both public surfaces exposed by the flag:

1. **`adapter.verify_database(...)`** — with `datasharing: true`, cross-database references to non-target databases are permitted (same relaxation as `ra3_node: true`), and the project compiles successfully.
2. **`adapter.has_feature("datasharing")`** — returns `True` in Jinja macros when the flag is set in the profile, so macros can gate datashare-aware behavior.

See [the Redshift setup docs](https://docs.getdbt.com/docs/core/connect-data-platform/redshift-setup#configurations) for background on `datasharing` and `ra3_node`.

### Expected output

| dbt command                 | Expected output        | Exit Code |
|-----------------------------|------------------------|-----------|
| `dbt run --target redshift` | Completed successfully | 0         |
