{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}

{% do set_up(db_schema, table_name) %}

{#- grant_config omits `test.user`, so apply_grants revokes it from the
    pg_user path. The recorded `revoke ... from "test.user"` is the assertion. -#}
{% set grant_config = {'select': ['dbt_test_user_1']} %}
{% do apply_grants(ref('a'), grant_config, should_revoke=True) %}

SELECT 1 as id
