{% macro athena__any_value(expression) -%}
    arbitrary({{ expression }})
{%- endmacro %}
