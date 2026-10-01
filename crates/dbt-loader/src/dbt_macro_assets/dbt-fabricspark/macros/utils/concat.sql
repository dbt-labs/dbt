{% macro fabricspark__concat(fields) -%}
    concat({{ fields|join(', ') }})
{%- endmacro %}
