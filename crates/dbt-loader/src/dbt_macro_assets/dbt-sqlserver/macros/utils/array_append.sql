{% macro sqlserver__array_append(array, new_element) -%}
    JSON_MODIFY({{ array }}, 'append $', {{ new_element }})
{%- endmacro %}
