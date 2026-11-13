{% macro trino__any_value(expression) -%}
    min({{ expression }})
{%- endmacro %}
