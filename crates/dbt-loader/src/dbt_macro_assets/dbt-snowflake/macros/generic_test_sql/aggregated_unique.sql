-- funcsign: (string) -> string
{% macro snowflake__aggregated_unique_field(column_name) %}
  {#- Plain cast, not safe_cast: snowflake__safe_cast emits try_cast, which
      accepts only a string source expression. -#}
  {{ dbt.cast(column_name, dbt.type_string()) }}
{% endmacro %}
