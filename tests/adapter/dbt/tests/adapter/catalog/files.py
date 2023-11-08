MODELS__VIEW = """
select 1 as id
"""


MACROS__GET_CATALOG = """
{% macro default__get_catalog(information_schema, schemas) -%}

  {% set typename = adapter.type() %}
  {% set msg -%}
    get_catalog not implemented for {{ typename }}
  {%- endset %}

  {{ exceptions.raise_compiler_error(msg) }}
{% endmacro %}
"""
