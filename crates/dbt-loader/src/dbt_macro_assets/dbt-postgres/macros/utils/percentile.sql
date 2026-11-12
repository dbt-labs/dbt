{#- Postgres doesn't support percentile_cont as a window function, so this is an aggregate.
    The caller must group by `partition_field`. -#}
{% macro postgres__percentile(percentile_field, partition_field, percent) -%}

    percentile_cont({{ percent }})
        within group (order by {{ percentile_field }})

{%- endmacro %}
