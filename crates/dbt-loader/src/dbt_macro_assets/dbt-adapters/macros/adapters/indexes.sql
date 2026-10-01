-- funcsign: (relation, dict[string, string]) -> optional[string]
{% macro get_create_index_sql(relation, index_dict) -%}
  {{ return(adapter.dispatch('get_create_index_sql', 'dbt')(relation, index_dict)) }}
{% endmacro %}

-- funcsign: (relation, dict[string, string]) -> optional[string]
{% macro default__get_create_index_sql(relation, index_dict) -%}
  {% do return(None) %}
{% endmacro %}


-- funcsign: (relation) -> string
{% macro create_indexes(relation) -%}
  {{ adapter.dispatch('create_indexes', 'dbt')(relation) }}
{%- endmacro %}

-- funcsign: (relation) -> string
{% macro default__create_indexes(relation) -%}
  {#- DIVERGENCE: FIXME: v2-parser stores unset `indexes` as null in the manifest, so
      `config.get('indexes', default=[])` returns None under dbt-core (1.x) and the
      for-loop blows up with "NoneType is not iterable". Coalesce to []. -#}
  {%- set _indexes = config.get('indexes', default=[]) or [] -%}

  {% for _index_dict in _indexes %}
    {% set create_index_sql = get_create_index_sql(relation, _index_dict) %}
    {% if create_index_sql %}
      {% do run_query(create_index_sql) %}
    {% endif %}
  {% endfor %}
{% endmacro %}


-- funcsign: (relation, string) -> string
{% macro get_drop_index_sql(relation, index_name) -%}
    {{ adapter.dispatch('get_drop_index_sql', 'dbt')(relation, index_name) }}
{%- endmacro %}

-- funcsign: (relation, string) -> string
{% macro default__get_drop_index_sql(relation, index_name) -%}
    {{ exceptions.raise_compiler_error("`get_drop_index_sql has not been implemented for this adapter.") }}
{%- endmacro %}


-- funcsign: (relation) -> string
{% macro get_show_indexes_sql(relation) -%}
    {{ adapter.dispatch('get_show_indexes_sql', 'dbt')(relation) }}
{%- endmacro %}

-- funcsign: (relation) -> string
{% macro default__get_show_indexes_sql(relation) -%}
    {{ exceptions.raise_compiler_error("`get_show_indexes_sql has not been implemented for this adapter.") }}
{%- endmacro %}
