{% macro bigquery__datediff(first_date, second_date, datepart) -%}

  {% if dbt_version[0] == 1 and dbt_version[2] >= 2 %}
    {{ return(dbt.datediff(first_date, second_date, datepart)) }}
  {% else %}

    datetime_diff(
        cast({{second_date}} as datetime),
        cast({{first_date}} as datetime),
        {{datepart}}
    )

  {% endif %}

{%- endmacro %}
