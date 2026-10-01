{% macro bigquery__get_empty_subquery_sql(select_sql, select_sql_header=none) %}
    {%- if select_sql_header is not none -%}
    {{ select_sql_header }}
    {%- endif -%}
    select * from (
        {{ select_sql }}
    ) as __dbt_sbq
    where false and current_timestamp() = current_timestamp()
    limit 0
{% endmacro %}


{# DIVERGENCE #}
{% macro bigquery__get_columns_in_relation(relation) -%}
  -- TODO: in this sql, look into if we get/derive "mode" field for BigqueryColumn
  {% call statement('get_columns_in_relation', fetch_result=True) %}
    SELECT
                column_name,
                data_type
             FROM `{{ relation.database }}.{{ relation.schema }}.INFORMATION_SCHEMA.COLUMNS`
             WHERE table_schema = '{{ relation.schema }}' and table_name = '{{ relation.identifier }}'
             ORDER BY ordinal_position

  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {{ return(sql_convert_columns_in_relation(table)) }}
{% endmacro %}