{% macro spark__date(year, month, day) -%}
    {%- set dt = modules.datetime.date(year, month, day) -%}
    {%- set iso_8601_formatted_date = dt.strftime('%Y-%m-%d') -%}
    to_date('{{ iso_8601_formatted_date }}', 'yyyy-MM-dd')
{%- endmacro %}
