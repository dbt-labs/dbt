Mirrors `standardize_grants_dict/models/c.sql`, but enables `datasharing: true` via `_profiles.override.yml` so `redshift__get_show_grant_sql` emits `SHOW GRANTS ON TABLE <db>.<schema>.<identifier>` and the Rust `standardize_grants_dict` Redshift arm takes the datasharing branch.

The setup macro seeds grants to `dbt_test_user_1`, role `tester`, and `PUBLIC`. Only the user grant survives the dict — the role and PUBLIC grants are filtered by the `identity_type == "user"` check, matching the legacy `pg_user + has_table_privilege()` behavior.

`dbt_test_user_1` and the `tester` role must already exist in the target cluster.
