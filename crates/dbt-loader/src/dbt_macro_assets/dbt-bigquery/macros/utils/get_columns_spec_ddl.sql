{% macro bigquery__format_column(column) -%}
  {% set data_type = column.data_type %}
  {% set formatted = column.column.lower() ~ " " ~ data_type %}
  {{ return({'name': column.name, 'data_type': data_type, 'formatted': formatted}) }}
{%- endmacro -%}

{% macro bigquery__get_empty_schema_sql(columns) %}
    {%- set col_err = [] -%}
    {% for col in columns.values() %}
      {%- if col['data_type'] is not defined -%}
        {{ col_err.append(col['name']) }}
      {%- endif -%}
    {%- endfor -%}
    {%- if (col_err | length) > 0 -%}
      {{ exceptions.column_type_missing(column_names=col_err) }}
    {%- endif -%}

    {%- set columns = adapter.nest_column_data_types(columns) -%}
    {{ return(dbt.default__get_empty_schema_sql(columns)) }}
{% endmacro %}

{% macro bigquery__get_select_subquery(sql) %}
    select {{ adapter.dispatch('get_column_names')() }}
    from (
        {{ sql }}
    ) as model_subq
{%- endmacro %}

{% macro bigquery__get_column_names() %}
  {#- loop through nested user_provided_columns to get column names -#}
    {%- set user_provided_columns = adapter.nest_column_data_types(model['columns']) -%}
    {%- for i in user_provided_columns %}
      {%- set col = user_provided_columns[i] -%}
      {%- set col_name = adapter.quote(col['name']) if col.get('quote') else col['name'] -%}
      {{ adapter.get_struct_select_expression(col_name, col.get('data_type') or '') }}{{ ", " if not loop.last }}
    {%- endfor -%}
{% endmacro %}
