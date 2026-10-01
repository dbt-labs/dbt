{% macro fabric__array_construct(inputs, data_type) -%}
    JSON_ARRAY({{ inputs|join(' , ') }})
{%- endmacro %}
