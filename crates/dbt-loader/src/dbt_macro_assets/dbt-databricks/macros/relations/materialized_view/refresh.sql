{% macro databricks__refresh_materialized_view(relation) -%}
  refresh materialized view {{ relation.render() }}
{% endmacro %}
