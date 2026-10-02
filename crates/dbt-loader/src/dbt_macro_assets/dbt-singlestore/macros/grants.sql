{% macro singlestore__get_show_grant_sql(relation) %}
  -- table_privileges stores user privileges for both tables and views.
  -- Usernames are formatted as 'user'@'host'.
  select privilege_type,
         case
           when locate('@', grantee) > 0 then
             substring(grantee, 2, length(grantee) - length(substring_index(grantee, '@', -1)) - 3)
           else
             replace(grantee, '`', '')
         end as grantee
    from information_schema.table_privileges
   where table_schema = '{{ relation.schema }}'
     and table_name = '{{ relation.identifier }}'

  union all

  -- role_privileges stores role privileges for both tables and views
  select privileges as privilege_type,
         concat('ROLE ', role) as grantee
    from information_schema.role_privileges
   where `database` = '{{ relation.schema }}'
     and `table` = '{{ relation.identifier }}'
{% endmacro %}

{% macro singlestore__format_grantee(grantee) %}
  {%- if grantee.upper().startswith('ROLE ') -%}
    {{ return(grantee) }}
  {%- elif grantee.startswith("'") or grantee.startswith("`") -%}
    {{ return(grantee) }}
  {%- else -%}
    {{ return("'" ~ grantee ~ "'") }}
  {%- endif -%}
{% endmacro %}

{% macro singlestore__get_grant_sql(relation, privilege, grantees) %}
  {%- set formatted_grantees = [] -%}
  {%- for g in grantees -%}
    {%- do formatted_grantees.append(singlestore__format_grantee(g)) -%}
  {%- endfor -%}
  grant {{ privilege }} on {{ relation.render() }} to {{ formatted_grantees | join(', ') }}
{% endmacro %}

{% macro singlestore__get_revoke_sql(relation, privilege, grantees) %}
  {%- set formatted_grantees = [] -%}
  {%- for g in grantees -%}
    {%- do formatted_grantees.append(singlestore__format_grantee(g)) -%}
  {%- endfor -%}
  revoke {{ privilege }} on {{ relation.render() }} from {{ formatted_grantees | join(', ') }}
{% endmacro %}

{% macro singlestore__call_dcl_statements(dcl_statement_list) %}
    {% for dcl_statement in dcl_statement_list %}
        {% call statement('grants') %}
            {{ dcl_statement }}
        {% endcall %}
    {% endfor %}
{% endmacro %}

{% macro singlestore__support_multiple_grantees_per_dcl_statement() %}
    {{ return(False) }}
{% endmacro %}
