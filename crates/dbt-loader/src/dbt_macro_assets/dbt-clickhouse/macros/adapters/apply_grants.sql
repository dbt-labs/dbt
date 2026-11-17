{% macro clickhouse__get_show_grant_sql(relation) %}
    SELECT toString(access_type) as privilege_type, COALESCE(user_name, role_name) as grantee from system.grants where table = '{{ relation.name }}'
    AND database = '{{ relation.schema }}'
{%- endmacro %}

{% macro clickhouse__call_dcl_statements(dcl_statement_list) %}
    {% for dcl_statement in dcl_statement_list %}
      {% call statement('dcl') %}
        {{ dcl_statement }};
      {% endcall %}
    {% endfor %}
{% endmacro %}


{%- macro clickhouse__get_grant_sql(relation, privilege, grantees) -%}
    grant {{ on_cluster_clause(relation)}} {{ privilege }} on {{ relation }} to {{ grantees | join(', ') }}
{%- endmacro -%}

{%- macro clickhouse__get_revoke_sql(relation, privilege, grantees) -%}
    revoke {{ on_cluster_clause(relation)}} {{ privilege }} on {{ relation }} from {{ grantees | join(', ') }}
{%- endmacro -%}
