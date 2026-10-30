{% macro setup() %}
  {% do run_query('CREATE SCHEMA IF NOT EXISTS `dbt-test-env`.`bigquery_use_directly`') %}
  {% do run_query('CREATE OR REPLACE TABLE `dbt-test-env`.`bigquery_use_directly`.`BigQueryUseDirectlyTest` (id INT64)') %}
{% endmacro %}
