{% macro fabric__last_day(date, datepart) -%}

    {%- if datepart == 'quarter' -%}
    	CAST(DATEADD(QUARTER, DATEDIFF(QUARTER, 0, {{ date }}) + 1, -1) AS DATE)
    {%- elif datepart == 'month' -%}
        EOMONTH ( {{ date }})
    {%- elif datepart == 'year' -%}
        CAST(DATEADD(YEAR, DATEDIFF(year, 0, {{ date }}) + 1, -1) AS DATE)
    {%- else -%}
        {{dbt_utils.default_last_day(date, datepart)}}
    {%- endif -%}

{%- endmacro %}
