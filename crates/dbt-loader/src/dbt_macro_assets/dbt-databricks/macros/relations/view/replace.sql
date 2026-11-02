{% macro databricks__get_replace_view_sql(target_relation, sql) %}
  {{ get_create_view_as_sql(target_relation, sql) }}
{% endmacro %}
