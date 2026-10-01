-- This macro provides type-safe casting with automatic default values for ClickHouse types.
-- When the literal string 'null' is passed as the field parameter, it returns the ClickHouse
-- default value for the specified type. This is primarily used in unit test fixtures to avoid
-- having to specify all non-nullable columns.

{% macro clickhouse__safe_cast(field, dtype) %}
  {%- if field == 'null' -%}
    CAST(defaultValueOfTypeName('{{ dtype | replace("'", "\\'") }}') AS {{ dtype }})
  {%- else -%}
    CAST({{ field }} AS {{ dtype }})
  {%- endif -%}
{% endmacro %}
