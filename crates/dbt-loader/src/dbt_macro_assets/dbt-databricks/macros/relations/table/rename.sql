-- funcsign: (relation, string) -> string
{% macro databricks__get_rename_table_sql(relation, new_name) %}
  ALTER TABLE {{ relation.render() }} RENAME TO `{{ new_name }}`
{% endmacro %}