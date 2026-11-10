{% macro get_replace_metric_view_sql(target_relation, sql) %}
  {{ adapter.dispatch('get_replace_metric_view_sql', 'dbt')(target_relation, sql) }}
{% endmacro %}

{% macro databricks__get_replace_metric_view_sql(target_relation, sql) %}
  {{ get_create_metric_view_as_sql(target_relation, sql) }}
{% endmacro %}