{% macro fabric__date_trunc(datepart, date) %}
    CAST(DATEADD({{datepart}}, DATEDIFF({{datepart}}, 0, {{date}}), 0) AS DATE)
{% endmacro %}
