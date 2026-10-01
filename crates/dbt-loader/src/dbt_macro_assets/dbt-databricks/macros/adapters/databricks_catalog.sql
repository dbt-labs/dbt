{% macro current_catalog() -%}
  {{ return(adapter.dispatch('current_catalog', 'dbt')()) }}
{% endmacro %}

{% macro databricks__current_catalog() -%}
  {{ return(run_query_as(current_catalog_sql(), 'current_catalog')) }}
{% endmacro %}

{% macro current_catalog_sql() %}
SELECT current_catalog()
{% endmacro %}

{% macro use_catalog(catalog) -%}
  {{ adapter.dispatch('use_catalog', 'dbt')(catalog) }}
{% endmacro %}

{% macro databricks__use_catalog(catalog) -%}
  {{ run_query_as(use_catalog_sql(catalog), 'use_catalog', fetch_result=False) }}
{% endmacro %}

{% macro use_catalog_sql(catalog) %}
USE CATALOG {{ adapter.quote(catalog)|lower }}
{% endmacro %}