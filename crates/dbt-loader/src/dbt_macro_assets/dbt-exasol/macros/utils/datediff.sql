{% macro exasol__datediff(first_date, second_date, datepart) -%}
    {#- Exasol has no DATEDIFF. dbt datediff counts datepart boundaries crossed,
        so truncate both ends to the datepart and take the *_BETWEEN of those
        (always a whole number); ceil/floor of the raw fractional difference
        would be wrong for same-period times. -#}
    {%- set datepart = datepart | lower -%}
    {%- if datepart == 'day' -%}
        cast(days_between(date_trunc('day', {{ second_date }}), date_trunc('day', {{ first_date }})) as integer)
    {%- elif datepart == 'week' -%}
        cast(days_between(date_trunc('week', {{ second_date }}), date_trunc('week', {{ first_date }})) / 7 as integer)
    {%- elif datepart == 'month' -%}
        cast(months_between(date_trunc('month', {{ second_date }}), date_trunc('month', {{ first_date }})) as integer)
    {%- elif datepart == 'quarter' -%}
        cast(months_between(date_trunc('quarter', {{ second_date }}), date_trunc('quarter', {{ first_date }})) / 3 as integer)
    {%- elif datepart == 'year' -%}
        cast(years_between(date_trunc('year', {{ second_date }}), date_trunc('year', {{ first_date }})) as integer)
    {%- elif datepart == 'hour' -%}
        cast(hours_between(date_trunc('hour', {{ second_date }}), date_trunc('hour', {{ first_date }})) as integer)
    {%- elif datepart == 'minute' -%}
        cast(minutes_between(date_trunc('minute', {{ second_date }}), date_trunc('minute', {{ first_date }})) as integer)
    {%- elif datepart == 'second' -%}
        cast(seconds_between(date_trunc('second', {{ second_date }}), date_trunc('second', {{ first_date }})) as integer)
    {%- else -%}
        {{ exceptions.raise_compiler_error("datediff: unsupported datepart '" ~ datepart ~ "' on Exasol") }}
    {%- endif -%}
{%- endmacro %}
