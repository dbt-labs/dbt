-- funcsign: () -> string
{%- macro get_table_types_sql() -%}
  {{ return(adapter.dispatch('get_table_types_sql')()) }}
{%- endmacro -%}

-- funcsign: () -> string
{% macro default__get_table_types_sql() %}
            case table_type
                when 'BASE TABLE' then 'table'
                when 'EXTERNAL TABLE' then 'external'
                when 'MATERIALIZED VIEW' then 'materializedview'
                else lower(table_type)
            end as {{ adapter.quote('table_type') }}
{% endmacro %}

-- funcsign: () -> string
{% macro postgres__get_table_types_sql() %}
            case table_type
                when 'BASE TABLE' then 'table'
                when 'FOREIGN' then 'external'
                when 'MATERIALIZED VIEW' then 'materializedview'
                else lower(table_type)
            end as {{ adapter.quote('table_type') }}
{% endmacro %}

-- funcsign: () -> string
{% macro databricks__get_table_types_sql() %}
            case table_type
                when 'MANAGED' then 'table'
                when 'BASE TABLE' then 'table'
                when 'MATERIALIZED VIEW' then 'materializedview'
                else lower(table_type)
            end as {{ adapter.quote('table_type') }}
{% endmacro %}
