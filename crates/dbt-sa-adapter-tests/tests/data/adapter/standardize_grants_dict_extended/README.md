End-to-end test of the `redshift_grants_extended` feature (fs #11872) on the
non-datasharing (`svv_relation_privileges`) code path.

`dbt_project.yml` sets `flags.redshift_grants_extended: true` and
`_profiles.override.yml` sets `datasharing: false`. `set_up` creates a clean
table; the prefixed grants live in `schema.yml` and are passed to `apply_grants`,
driving the full chain: `redshift__apply_grants` -> `normalize_grants_dict` ->
read via `svv_relation_privileges` (`standardize_grants_dict` `SvvIdentities`
branch) -> `redshift__get_grant_sql` -> `redshift__format_grantees`. The recorded
GRANT is the assertion:

```
grant select on <a> to dbt_test_user_1, GROUP dbt_test_group_1, ROLE tester
```

`user:` is stripped, `group:`/`role:` become `GROUP`/`ROLE`. With the flag off the
same config would emit `to user:dbt_test_user_1, ...` verbatim and fail.

`dbt_test_user_1`, group `dbt_test_group_1`, and role `tester` must already exist
in the target cluster.

To re-record against a live Redshift target (with the prerequisites above), run:

```
cargo xtask test -u --adapter-record -p dbt-adapter-tests adapters_redshift_standardize_grants_dict_extended
```

The datasharing variant (`SHOW GRANTS`) lives in
`standardize_grants_dict_extended_datasharing`.
