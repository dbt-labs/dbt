{#- Calculates a continuous percentile of `percentile_field` within each `partition_field` partition,
    as a window function. Postgres doesn't support percentile_cont as a window function, so
    postgres__percentile is an aggregate and the caller must group by `partition_field`. -#}
{% macro percentile(percentile_field, partition_field, percent) -%}
    {{ return(adapter.dispatch('percentile', 'dbt') (percentile_field, partition_field, percent)) }}
{% endmacro %}

{% macro default__percentile(percentile_field, partition_field, percent) -%}

    percentile_cont({{ percent }})
        within group (order by {{ percentile_field }})
        over (partition by {{ partition_field }})

{%- endmacro %}
