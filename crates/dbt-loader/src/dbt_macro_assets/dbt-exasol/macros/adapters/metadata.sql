{% macro exasol__get_relation_last_modified(information_schema, relations) -%}
    {#- Metadata-based source freshness. SYS.EXA_USER_OBJECTS.LAST_COMMIT is the
        last committed change to the object. Aliases are quoted uppercase because
        the Rust freshness reader looks up IDENTIFIER/SCHEMA/LAST_MODIFIED and
        SCHEMA is a reserved word in Exasol. -#}
    {%- call statement('last_modified', fetch_result=True) -%}
        select root_name as "SCHEMA",
               object_name as "IDENTIFIER",
               last_commit as "LAST_MODIFIED",
               {{ current_timestamp() }} as "SNAPSHOTTED_AT"
        from sys.exa_user_objects
        where (
        {%- for relation in relations -%}
            (upper(root_name) = upper('{{ relation.schema }}') and
             upper(object_name) = upper('{{ relation.identifier }}') and
             object_type in ('TABLE', 'VIEW')){%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
        )
    {%- endcall -%}

    {{ return(load_result('last_modified')) }}

{% endmacro %}
