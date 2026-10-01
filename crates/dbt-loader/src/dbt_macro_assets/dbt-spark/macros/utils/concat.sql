{% macro spark__concat(fields) -%}
    concat({{ fields|join(', ') }})
{%- endmacro %}
