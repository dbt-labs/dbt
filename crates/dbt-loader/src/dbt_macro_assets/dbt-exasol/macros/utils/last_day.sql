{%- macro exasol__last_day(date, datepart) -%}
    {#- default__last_day is built on dbt.dateadd's ANSI dateadd(), which Exasol
        lacks; use ADD_DAYS/ADD_<datepart>S over DATE_TRUNC instead. Quarter has
        no ADD_QUARTERS function, so advance 3 months. -#}
    {%- if datepart | lower != 'quarter' %}
        cast(
            add_days(add_{{ datepart | lower }}s(date_trunc('{{ datepart }}', {{ date }}), 1), -1)
            as date)
    {%- else -%}
        cast(
            add_days(add_months(date_trunc('{{ datepart }}', {{ date }}), 3), -1)
            as date)
    {%- endif -%}
{%- endmacro -%}
