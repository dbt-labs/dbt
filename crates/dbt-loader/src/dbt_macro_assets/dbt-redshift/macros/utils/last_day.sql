{% macro redshift__last_day(date, datepart) %}
    cast(
        {{dbt.dateadd('day', '-1',
        dbt.dateadd(datepart, '1', dbt.date_trunc(datepart, date))
        )}}
        as date)
{% endmacro %}
