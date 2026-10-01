{% macro date_trunc(datepart, date) -%}
  {{ return(adapter.dispatch('date_trunc', 'dbt') (datepart, date)) }}
{%- endmacro %}

{% macro default__date_trunc(datepart, date) -%}
    date_trunc('{{datepart}}', {{date}})
{%- endmacro %}
