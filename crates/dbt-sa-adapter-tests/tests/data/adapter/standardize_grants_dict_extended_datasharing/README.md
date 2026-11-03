End-to-end test of the `redshift_grants_extended` feature (fs #11872) on the
datasharing (`SHOW GRANTS`) code path.

`dbt_project.yml` sets `flags.redshift_grants_extended: true` and
`_profiles.override.yml` sets `datasharing: true`. `set_up` creates a clean
table; the prefixed grants live in `schema.yml` and are passed to `apply_grants`,
driving the full chain: `redshift__apply_grants` -> `normalize_grants_dict` ->
read via `SHOW GRANTS` (`standardize_grants_dict` `ShowIdentities` branch) ->
`redshift__get_grant_sql` -> `redshift__format_grantees`. The recorded GRANT is
the assertion:

```
grant select on <a> to dbt_test_user_1, GROUP dbt_test_group_1, ROLE tester
```

This is the sibling of `standardize_grants_dict_extended` (the `datasharing: false`
/ `svv_relation_privileges` path); it differs only in the read query apply_grants
issues. The `SHOW GRANTS` `/`-prefix -> `group:` read translation is unit-tested
in `test_redshift_standardize_grants_dict_extended_datasharing`.

`dbt_test_user_1`, group `dbt_test_group_1`, and role `tester` must already exist
in the target cluster.

To re-record against a live Redshift target (with the prerequisites above), run:

```
cargo xtask test -u --adapter-record -p dbt-adapter-tests adapters_redshift_standardize_grants_dict_extended_datasharing
```
