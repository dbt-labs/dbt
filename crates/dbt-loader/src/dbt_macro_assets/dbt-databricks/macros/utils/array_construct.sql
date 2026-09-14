{% macro databricks__array_construct(inputs, data_type) -%}
    {% if inputs|length > 0 %}
    array( {{ inputs|join(' , ') }} )
    {% else %}
    cast(array() as array<{{data_type}}>)
    {% endif %}
{%- endmacro %}
