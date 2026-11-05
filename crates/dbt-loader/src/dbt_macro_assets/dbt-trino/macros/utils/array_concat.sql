{% macro trino__array_concat(array_1, array_2) -%}
    concat({{ array_1 }}, {{ array_2 }})
{%- endmacro %}
