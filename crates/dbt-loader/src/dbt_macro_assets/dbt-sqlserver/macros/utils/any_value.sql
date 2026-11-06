{% macro sqlserver__any_value(expression) -%}

    min({{ expression }})

{%- endmacro %}
