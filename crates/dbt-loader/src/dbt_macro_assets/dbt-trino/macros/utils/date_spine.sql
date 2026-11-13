{% macro trino__date_spine(datepart, start_date, end_date) %}


    {# call as follows:

    date_spine(
        "day",
        "to_date('01/01/2016', 'mm/dd/yyyy')",
        "dbt.dateadd(week, 1, current_date)"
    ) #}


    with rawdata as (

        {{dbt.generate_series(
            dbt.get_intervals_between(start_date, end_date, datepart)
        )}}

    ),

    all_periods as (

        select (
            {{
                dbt.dateadd(
                    datepart,
                    "row_number() over (order by 1) - 1",
                "cast(" ~ start_date ~ " as date)"
                )
            }}
        ) as date_{{datepart}}
        from rawdata

    ),

    filtered as (

        select *
        from all_periods
    where date_{{datepart}} <= cast({{ end_date }} as date)

    )

    select * from filtered

{% endmacro %}
