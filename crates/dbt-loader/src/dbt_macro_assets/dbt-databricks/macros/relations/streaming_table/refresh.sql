{% macro refresh_streaming_table(relation, sql) -%}
  {{ adapter.dispatch('refresh_streaming_table', 'dbt')(relation, sql) }}
{%- endmacro %}

{% macro databricks__refresh_streaming_table(relation, sql) -%}
  refresh streaming table {{ relation.render() }}
{% endmacro %}
