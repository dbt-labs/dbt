{% macro athena__array_append(array, new_element) -%}
    {{ array }} || {{ new_element }}
{%- endmacro %}
