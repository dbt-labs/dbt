{% macro bigquery__percentile(percentile_field, partition_field, percent) -%}

    percentile_cont({{ percentile_field }}, {{ percent }})
        over (partition by {{ partition_field }})

{%- endmacro %}
