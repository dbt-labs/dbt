{% macro bigquery__array_concat(array_1, array_2) -%}
    array_concat({{ array_1 }}, {{ array_2 }})
{%- endmacro %}
