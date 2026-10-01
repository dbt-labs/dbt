{% macro salesforce__last_day(date, datepart) -%}
    cast(
        date_add('day', -1, date_add({{datepart}}, 1, date_trunc({{datepart}}, {{date}}))
    as date)
{%- endmacro %}
