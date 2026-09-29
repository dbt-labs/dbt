{% macro exasol__get_show_grant_sql(relation) %}
    {#- EXA_DBA_OBJ_PRIVS lists all object privileges; aliases match what
        standardize_grants_dict expects ("privilege_type", "grantee").
        Grants held by the current user (the owner) are filtered out so dbt
        does not try to revoke them. -#}
    select privilege as "privilege_type", grantee as "grantee"
    from sys.exa_dba_obj_privs
    where upper(object_schema) = upper('{{ relation.schema }}')
      and upper(object_name) = upper('{{ relation.identifier }}')
      and grantee != current_user
{% endmacro %}

{% macro exasol__call_dcl_statements(dcl_statement_list) %}
    {#- Exasol does not allow multiple statements per execute; run each
        GRANT/REVOKE individually. -#}
    {%- for dcl_statement in dcl_statement_list %}
        {%- call statement('grants') -%}
            {{ dcl_statement }}
        {%- endcall %}
    {%- endfor %}
{% endmacro %}
