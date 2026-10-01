{% macro redshift__datediff(first_date, second_date, datepart) -%}

    datediff(
        {{ datepart }},
        {{ first_date }},
        {{ second_date }}
        )

{%- endmacro %}
