{% macro date(year, month, day) %}
  {{ return(adapter.dispatch('date', 'dbt') (year, month, day)) }}
{% endmacro %}


{% macro default__date(year, month, day) -%}
    {%- set dt = modules.datetime.date(year, month, day) -%}
    {%- set iso_8601_formatted_date = dt.strftime('%Y-%m-%d') -%}
    to_date('{{ iso_8601_formatted_date }}', 'YYYY-MM-DD')
{%- endmacro %}
