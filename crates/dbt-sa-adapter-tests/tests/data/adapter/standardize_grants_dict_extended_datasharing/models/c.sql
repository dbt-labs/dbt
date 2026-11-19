{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}

{% do set_up(db_schema, table_name) %}
{% do apply_grants(ref('a'), config.get('grants'), should_revoke=True) %}

SELECT 1 as id
