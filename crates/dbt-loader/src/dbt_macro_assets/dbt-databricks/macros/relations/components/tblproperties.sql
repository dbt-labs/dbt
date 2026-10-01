{% macro get_create_sql_tblproperties(tblproperties) %}
  {{ databricks__tblproperties_clause(tblproperties)}}
{% endmacro %}
