{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{% set current_grants_table = run_query(get_show_grant_sql(ref('a'))) %}

{% set current_grants_dict = adapter.standardize_grants_dict(current_grants_table) %}
{{ log('current_grants_table:\n' ~ current_grants_table.print_table(), info=True) }}
{{ log('current_grants_table:\n' ~ current_grants_table, info=True) }}

{{ log('current_grants_dict:\n' ~ current_grants_dict, info=True) }}

SELECT 1 as id
