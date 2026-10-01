{% macro athena__bool_or(expression) -%}
    bool_or({{ expression }})
{%- endmacro %}
