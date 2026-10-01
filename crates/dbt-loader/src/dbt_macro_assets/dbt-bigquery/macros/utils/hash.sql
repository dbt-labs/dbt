{% macro bigquery__hash(field) -%}
    to_hex({{dbt.default__hash(field)}})
{%- endmacro %}
