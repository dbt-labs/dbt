{% macro snowflake__bool_or(expression) -%}

    boolor_agg({{ expression }})

{%- endmacro %}
