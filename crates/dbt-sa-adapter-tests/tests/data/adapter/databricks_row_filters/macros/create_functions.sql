{% macro create_hide_even_numbers() %}
    CREATE FUNCTION IF NOT EXISTS {{ target.database }}.{{ t_schema_name() }}.hide_even_numbers(
        number INT
    )
    RETURNS BOOLEAN
    RETURN number % 2 = 0;
{% endmacro %}
