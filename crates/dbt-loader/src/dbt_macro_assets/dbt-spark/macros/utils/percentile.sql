{% macro spark__percentile(percentile_field, partition_field, percent) -%}

    percentile({{ percentile_field }}, {{ percent }})
        over (partition by {{ partition_field }})

{%- endmacro %}
