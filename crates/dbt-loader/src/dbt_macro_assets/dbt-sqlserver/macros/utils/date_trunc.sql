{% macro sqlserver__date_trunc(datepart, date) %}
    {%- set part = datepart | lower -%}
    {#- DATEADD(day, 0, x) passes date and datetime values through unchanged and reads an int
        or a string as datetime, like DATEDIFF from 0 in the last branch. Cast to date or
        datetime2, a datetimeoffset keeps its local time, which the last branch uses too. -#}
    {%- set value = "DATEADD(day, 0, " ~ date ~ ")" -%}
    {%- if part in ["hour", "hh", "minute", "mi", "n", "second", "ss", "s", "millisecond", "ms"] -%}
        {#- Counting from midnight of the same day keeps DATEDIFF within int. -#}
        {%- set time = "CAST(" ~ value ~ " AS DATETIME2)" -%}
        {%- set day_start = "CAST(CAST(" ~ value ~ " AS DATE) AS DATETIME2)" -%}
        DATEADD({{ datepart }}, DATEDIFF({{ datepart }}, {{ day_start }}, {{ time }}), {{ day_start }})
    {%- elif part in ["week", "wk", "ww"] -%}
        {#- 0001-01-01 is a Monday and the earliest date, so whole weeks counted from it start on
            Monday and / 7 rounds down. DATEDIFF(week) would count Sundays. -#}
        DATEADD(day, DATEDIFF(day, DATEFROMPARTS(1, 1, 1), CAST({{ value }} AS DATE)) / 7 * 7, DATEFROMPARTS(1, 1, 1))
    {%- else -%}
        CAST(DATEADD({{ datepart }}, DATEDIFF({{ datepart }}, 0, {{ date }}), 0) AS DATE)
    {%- endif -%}
{% endmacro %}
