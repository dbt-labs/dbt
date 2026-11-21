{# Databricks rejects `array()` in a Delta table: it is typed array<void>, so an empty array needs a cast. #}
-- funcsign: (list[any], string) -> list[any]
{% macro databricks__array_construct(inputs, data_type) -%}
    {% if inputs|length > 0 %}
    array( {{ inputs|join(' , ') }} )
    {% else %}
    cast(array() as array<{{data_type}}>)
    {% endif %}
{%- endmacro %}
