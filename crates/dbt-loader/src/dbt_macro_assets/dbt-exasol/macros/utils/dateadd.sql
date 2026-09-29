{% macro exasol__dateadd(datepart, interval, from_date_or_timestamp) -%}
    {%- set datepart = datepart | lower -%}
    {%- if datepart == 'quarter' -%}
        {# Exasol has no ADD_QUARTERS; a quarter is three months. #}
        add_months({{ from_date_or_timestamp }}, ({{ interval }}) * 3)
    {%- elif datepart in ['year', 'month', 'week', 'day', 'hour', 'minute', 'second'] -%}
        add_{{ datepart }}s({{ from_date_or_timestamp }}, {{ interval }})
    {%- else -%}
        {{ exceptions.raise_compiler_error("dateadd: unsupported datepart '" ~ datepart ~ "' on Exasol") }}
    {%- endif -%}
{%- endmacro %}
