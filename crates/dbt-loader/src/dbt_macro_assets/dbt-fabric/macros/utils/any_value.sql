{% macro fabric__any_value(expression) -%}

    min({{ expression }})

{%- endmacro %}
