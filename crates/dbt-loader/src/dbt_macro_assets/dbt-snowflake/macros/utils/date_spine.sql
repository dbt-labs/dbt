{% macro snowflake__date_spine(datepart, start_date, end_date) %}
    select (
        {{
            dbt.dateadd(
                datepart,
                "row_number() over (order by 1) - 1",
                start_date
            )
        }}
    ) as date_{{datepart}}
    from table(flatten(input => array_generate_range(0, {{ dbt.datediff(start_date, end_date, datepart) }} )))
{% endmacro %}
