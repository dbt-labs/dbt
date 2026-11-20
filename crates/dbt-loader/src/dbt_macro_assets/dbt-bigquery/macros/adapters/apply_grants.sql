{% macro bigquery__get_show_grant_sql(relation) %}
    {% set location = adapter.get_dataset_location(relation) %}
    {% set relation = relation.incorporate(location=location) %}

    select privilege_type, grantee
    from {{ relation.information_schema("OBJECT_PRIVILEGES") }}
    where object_schema = "{{ relation.dataset }}"
      and object_name = "{{ relation.identifier }}"
      -- filter out current user
      and split(grantee, ':')[offset(1)] != session_user()
{% endmacro %}


{%- macro bigquery__get_grant_sql(relation, privilege, grantee) -%}
    {%- if relation.type == 'function' -%}
        {{ exceptions.raise_compiler_error("BigQuery GRANT and REVOKE statements do not support functions. Remove `grants` from function `" ~ relation.identifier ~ "` and manage access to the routine with IAM instead.") }}
    {%- endif -%}
    grant `{{ privilege }}` on {{ relation.type }} {{ relation }} to {{ '\"' + grantee|join('\", \"') + '\"' }}
{%- endmacro -%}

{%- macro bigquery__get_revoke_sql(relation, privilege, grantee) -%}
    {%- if relation.type == 'function' -%}
        {{ exceptions.raise_compiler_error("BigQuery GRANT and REVOKE statements do not support functions. Remove `grants` from function `" ~ relation.identifier ~ "` and manage access to the routine with IAM instead.") }}
    {%- endif -%}
    revoke `{{ privilege }}` on {{ relation.type }} {{ relation }} from {{ '\"' + grantee|join('\", \"') + '\"' }}
{%- endmacro -%}
