{% macro get_create_metric_view_as_sql(relation, sql) -%}
  {{ adapter.dispatch('get_create_metric_view_as_sql', 'dbt')(relation, sql) }}
{%- endmacro %}

{% macro databricks__get_create_metric_view_as_sql(relation, sql) %}
{%- set yaml_body = adapter.yaml_quote_backtick_values(sql) -%}
create or replace view {{ relation.render() }}
with metrics
language yaml
as $$
{{ yaml_body }}
$$
{% endmacro %}