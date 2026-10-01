{% macro redshift__get_relation_last_modified(information_schema, relations) -%}

    {%- call statement('last_modified', fetch_result=True) -%}
        select
            ns.nspname as "schema",
            c.relname as identifier,
            max(qd.start_time) as last_modified,
            {{ current_timestamp() }} as snapshotted_at
        from pg_class c
        join pg_namespace ns
            on ns.oid = c.relnamespace
        join sys_query_detail qd
            on qd.table_id = c.oid
        where qd.step_name = 'insert'
        and (
            {%- for relation in relations -%}
                (
                    upper(ns.nspname) = upper('{{ relation.schema }}')
                and upper(c.relname) = upper('{{ relation.identifier }}')
                )
                {%- if not loop.last %} or {% endif -%}
            {%- endfor -%}
        )
        group by 1, 2, 4
    {%- endcall -%}

    {{ return(load_result('last_modified')) }}

{% endmacro %}
