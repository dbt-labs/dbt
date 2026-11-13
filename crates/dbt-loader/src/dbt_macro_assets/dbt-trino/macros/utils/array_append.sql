{% macro trino__array_append(array, new_element) -%}
    {{ array_concat(array, array_construct([new_element])) }}
{%- endmacro %}
