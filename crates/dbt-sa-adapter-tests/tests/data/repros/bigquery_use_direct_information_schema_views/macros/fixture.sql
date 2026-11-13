{% macro setup() %}
  {% do run_query('CREATE SCHEMA IF NOT EXISTS `dbt-test-env`.`bigquery_use_direct_information_schema_views_src`') %}
  {% do run_query('CREATE OR REPLACE TABLE `dbt-test-env`.`bigquery_use_direct_information_schema_views_src`.`source_table` (id INT64)') %}
{% endmacro %}
