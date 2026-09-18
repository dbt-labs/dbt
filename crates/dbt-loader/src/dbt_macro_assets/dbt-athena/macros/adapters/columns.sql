{#- Fusion runs this macro from AdapterImpl::get_columns_in_relation, so
    delegating back to adapter.get_columns_in_relation would recurse. Trino's
    information_schema.columns has no length/precision/scale columns. -#}
{% macro athena__get_columns_in_relation(relation) -%}
  {% call statement('get_columns_in_relation', fetch_result=True) %}
    select
        column_name,
        data_type,
        cast(null as integer) as character_maximum_length,
        cast(null as integer) as numeric_precision,
        cast(null as integer) as numeric_scale
    from information_schema.columns
    where lower(table_name) = '{{ relation.identifier | lower }}'
      {% if relation.schema %} and lower(table_schema) = '{{ relation.schema | lower }}' {% endif %}
    order by ordinal_position
  {% endcall %}
  {% set table = load_result('get_columns_in_relation').table %}
  {{ return(sql_convert_columns_in_relation(table)) }}
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
