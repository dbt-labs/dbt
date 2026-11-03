{% macro singlestore__get_show_grant_sql(relation) %}
  {%- set target_schema = relation.database | default(relation.schema, true) | trim -%}
  {%- if not target_schema -%}
    {%- set target_schema = relation.schema | trim -%}
  {%- endif -%}
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
   where table_schema = '{{ escape_single_quotes(target_schema) }}'
     and table_name = '{{ escape_single_quotes(relation.identifier) }}'

  union all

  -- role_privileges stores role privileges for both tables and views
  select privileges as privilege_type,
         concat('ROLE ', role) as grantee
    from information_schema.role_privileges
   where `database` = '{{ escape_single_quotes(target_schema) }}'
     and `table` = '{{ escape_single_quotes(relation.identifier) }}'
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
