{% macro duckdb__generate_series(upper_bound) %}
    select
        generate_series as generated_number
    from generate_series(1, {{ upper_bound }})
{% endmacro %}
