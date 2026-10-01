{% macro postgres__last_day(date, datepart) -%}

    {%- if datepart == 'quarter' -%}
    -- postgres dateadd does not support quarter interval.
    cast(
        {{dbt.dateadd('day', '-1',
        dbt.dateadd('month', '3', dbt.date_trunc(datepart, date))
        )}}
        as date)
    {%- else -%}
    {{dbt.default_last_day(date, datepart)}}
    {%- endif -%}

{%- endmacro %}
