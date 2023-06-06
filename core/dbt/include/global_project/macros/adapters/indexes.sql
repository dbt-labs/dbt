{% macro get_create_index_sql(relation, index_dict) -%}
  {{ return(adapter.dispatch('get_create_index_sql', 'dbt')(relation, index_dict)) }}
{% endmacro %}

{% macro default__get_create_index_sql(relation, index_dict) -%}
  {% do return(None) %}
{% endmacro %}


{% macro create_indexes(relation) -%}
  {{ adapter.dispatch('create_indexes', 'dbt')(relation) }}
{%- endmacro %}

{% macro default__create_indexes(relation) -%}
  {%- set _indexes = config.get('indexes', default=[]) -%}

  {% for _index_dict in _indexes %}
    {% set create_index_sql = get_create_index_sql(relation, _index_dict) %}
    {% if create_index_sql %}
      {% do run_query(create_index_sql) %}
    {% endif %}
  {% endfor %}
{% endmacro %}


{% macro get_drop_index_sql(relation, index_name) -%}
    {{ adapter.dispatch('get_drop_index_sql', 'dbt')(relation, index_name) }}
{%- endmacro %}

{% macro default__get_drop_index_sql(relation, index_name) -%}
    {{ exceptions.raise_compiler_error("`get_drop_index_sql has not been implemented for this adapter.") }}
{%- endmacro %}


{% macro get_show_indexes_sql(relation) -%}
    {{ adapter.dispatch('get_show_indexes_sql', 'dbt')(relation) }}
{%- endmacro %}

{% macro default__get_show_indexes_sql(relation) -%}
    {{ exceptions.raise_compiler_error("`get_show_indexes_sql has not been implemented for this adapter.") }}
{%- endmacro %}
