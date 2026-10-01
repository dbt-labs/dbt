{% macro bigquery__array_construct(inputs, data_type) -%}
    {% if inputs|length > 0 %}
    [ {{ inputs|join(' , ') }} ]
    {% else %}
    ARRAY<{{data_type}}>[]
    {% endif %}
{%- endmacro %}
