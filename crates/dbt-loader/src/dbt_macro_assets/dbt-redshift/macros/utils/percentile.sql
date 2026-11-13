{#- Redshift supports percentile_cont as a window function, unlike Postgres. The field is cast
    to float because percentile_cont needs a numeric type. -#}
{% macro redshift__percentile(percentile_field, partition_field, percent) -%}

    percentile_cont({{ percent }})
        within group (order by cast({{ percentile_field }} as float))
        over (partition by {{ partition_field }})

{%- endmacro %}
