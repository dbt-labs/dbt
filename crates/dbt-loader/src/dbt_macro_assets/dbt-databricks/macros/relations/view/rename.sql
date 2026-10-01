{% macro databricks__get_rename_view_sql(relation, new_name) %}
  ALTER VIEW {{ relation.render() }} RENAME TO {{ new_name }}
{% endmacro %}