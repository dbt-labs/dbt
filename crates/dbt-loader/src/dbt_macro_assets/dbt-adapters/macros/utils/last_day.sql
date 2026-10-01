{% macro last_day(date, datepart) %}
  {{ return(adapter.dispatch('last_day', 'dbt') (date, datepart)) }}
{% endmacro %}

{%- macro default_last_day(date, datepart) -%}
    cast(
        {{dbt.dateadd('day', '-1',
        dbt.dateadd(datepart, '1', dbt.date_trunc(datepart, date))
        )}}
        as date)
{%- endmacro -%}

{% macro default__last_day(date, datepart) -%}
    {{dbt.default_last_day(date, datepart)}}
{%- endmacro %}
