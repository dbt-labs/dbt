{% macro duckdb__any_value(expression) -%}

    arbitrary({{ expression }})

{%- endmacro %}
