{% macro datediff(first_date, second_date, datepart) %}
  {{ return(adapter.dispatch('datediff', 'dbt')(first_date, second_date, datepart)) }}
{% endmacro %}


{% macro default__datediff(first_date, second_date, datepart) -%}

    datediff(
        {{ datepart }},
        {{ first_date }},
        {{ second_date }}
        )

{%- endmacro %}
