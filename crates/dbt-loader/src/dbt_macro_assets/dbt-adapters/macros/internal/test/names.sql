-- funcsign: () -> string
{% macro t_database_name() %}
  {{ return (adapter.dispatch("t_database_name")()) }}
{% endmacro %}

-- funcsign: () -> string
{% macro default__t_database_name() %}
  {{ return(env_var('DBT_DB_NAME')) }}
{% endmacro %}

-- funcsign: () -> string
{% macro bigquery__t_database_name() %}
  {{ return(env_var('GOOGLE_CLOUD_PROJECT')) }}
{% endmacro %}

-- funcsign: () -> string
{% macro databricks__t_database_name() %}
  {{ return(env_var('DATABRICKS_CATALOG')) }}
{% endmacro %}

-- funcsign: () -> string
{% macro redshift__t_database_name() %}
  {{ return(env_var('REDSHIFT_DATABASE')) }}
{% endmacro %}

-- funcsign: () -> string
{% macro t_schema_name() %}
  {{ return(target.schema) }}
{% endmacro %}
