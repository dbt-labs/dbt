{% macro fabric__dateadd(datepart, interval, from_date_or_timestamp) %}

    dateadd(
        {{ datepart }},
        {{ interval }},
        cast({{ from_date_or_timestamp }} as datetime2(6))
        )

{% endmacro %}
