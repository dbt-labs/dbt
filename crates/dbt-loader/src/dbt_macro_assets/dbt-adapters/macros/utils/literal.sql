-- funcsign: (string) -> string
{%- macro string_literal(value) -%}
  {{ return(adapter.dispatch('string_literal', 'dbt') (value)) }}
{%- endmacro -%}

-- funcsign: (string) -> string
{% macro default__string_literal(value) -%}
    '{{ value }}'
{%- endmacro %}
