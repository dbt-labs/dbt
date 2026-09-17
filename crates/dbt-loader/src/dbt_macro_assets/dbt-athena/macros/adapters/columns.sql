{% macro athena__get_columns_in_relation(relation) -%}
  {{ return(adapter.get_columns_in_relation(relation)) }}
{% endmacro %}

{% macro athena__get_empty_schema_sql(columns) %}
    {%- set col_err = [] -%}
    select
    {% for i in columns %}
      {%- set col = columns[i] -%}
      {%- if col['data_type'] is not defined -%}
        {{ col_err.append(col['name']) }}
      {%- else -%}
        {% set col_name = adapter.quote(col['name']) if col.get('quote') else col['name'] %}
        cast(null as {{ dml_data_type(col['data_type']) }}) as {{ col_name }}{{ ", " if not loop.last }}
      {%- endif -%}
    {%- endfor -%}
    {%- if (col_err | length) > 0 -%}
      {{ exceptions.column_type_missing(column_names=col_err) }}
    {%- endif -%}
{% endmacro %}
