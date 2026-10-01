{% macro fabric__get_tables_by_pattern_sql(schema_pattern, table_pattern, exclude='', database=target.database) %}

    select distinct
        table_schema as {{ adapter.quote('table_schema') }},
        table_name as {{ adapter.quote('table_name') }},
        {{ dbt_utils.get_table_types_sql() }}
    from {{ database }}.INFORMATION_SCHEMA.TABLES
    where table_schema like '{{ schema_pattern }}'
    and table_name like '{{ table_pattern }}'
    and table_name not like '{{ exclude }}'

{% endmacro %}
