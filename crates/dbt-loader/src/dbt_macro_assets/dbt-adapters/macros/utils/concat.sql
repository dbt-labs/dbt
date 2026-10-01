{% macro concat(fields) -%}
  {{ return(adapter.dispatch('concat', 'dbt')(fields)) }}
{%- endmacro %}

{% macro default__concat(fields) -%}
    {{ fields|join(' || ') }}
{%- endmacro %}
