{%- macro databricks__get_create_sql(relation, sql) -%}
    {%- if relation.is_view -%}
        {{ get_create_view_as_sql(relation, sql) }}

    {%- elif relation.is_table -%}
        {{ get_create_table_as_sql(False, relation, sql) }}

    {%- elif relation.is_materialized_view -%}
        {{ get_create_materialized_view_as_sql(relation, sql) }}

    {%- elif relation.is_streaming_table -%}
        {{ get_create_streaming_table_as_sql(relation, sql) }}

    {%- elif relation.is_metric_view -%}
        {{ get_create_metric_view_as_sql(relation, sql) }}

    {%- else -%}
        {{- exceptions.raise_compiler_error("`get_create_sql` has not been implemented for: " ~ relation.type ) -}}

    {%- endif -%}

{%- endmacro -%}
