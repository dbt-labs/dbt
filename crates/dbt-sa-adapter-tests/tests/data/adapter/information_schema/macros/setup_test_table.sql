{% macro setup_test_table() %}
  {% do adapter.dispatch("setup_test_table", "dbt")() %}
{% endmacro %}

{% macro default__setup_test_table() %}
  {{ log("Setting up test table for information_schema tests", info=true) }}
  
  {# Create table with some columns #}
  {% set table_name = 'info_schema_test_table' %}
  {% set full_table = target.schema ~ '.' ~ table_name %}
  
  {% set create_sql %}
    CREATE TABLE IF NOT EXISTS {{ full_table }} (
      id INT,
      name VARCHAR(100),
      created_at TIMESTAMP
    )
  {% endset %}
  {% do adapter.execute(create_sql) %}
  
  {# Add the created relation to cache #}
  {%- set database_name = target.catalog if target.type == 'databricks' else target.database -%}
  {% set relation = api.Relation.create(database=database_name, schema=target.schema, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
  
  {# Insert some test data #}
  {% set insert_sql %}
    INSERT INTO {{ full_table }} 
    SELECT 1, 'test', CURRENT_TIMESTAMP
  {% endset %}
  {% do adapter.execute(insert_sql) %}
  
  {{ log("Created test table: " ~ full_table, info=true) }}
{% endmacro %}

{% macro bigquery__setup_test_table() %}
  {{ log("Setting up BigQuery test table for information_schema tests", info=true) }}
  
  {# Create schema if not exists #}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ target.schema) %}
  
  {# Create table #}
  {% set table_name = 'info_schema_test_table' %}
  {% set full_table = target.schema ~ '.' ~ table_name %}
  
  {% set create_sql %}
    CREATE OR REPLACE TABLE {{ full_table }} (
      id INT64,
      name STRING,
      created_at TIMESTAMP
    )
  {% endset %}
  {% do adapter.execute(create_sql) %}
  
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=target.schema, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
  
  {# Insert some test data #}
  {% set insert_sql %}
    INSERT INTO {{ full_table }} VALUES
    (1, 'test', CURRENT_TIMESTAMP())
  {% endset %}
  {% do adapter.execute(insert_sql) %}
  
  {{ log("Created BigQuery test table: " ~ full_table, info=true) }}
{% endmacro %}

{% macro snowflake__setup_test_table() %}
  {{ log("Setting up Snowflake test table for information_schema tests", info=true) }}
  
  {# Snowflake needs to set the database context first #}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ target.schema) %}
  {% do adapter.execute('USE SCHEMA ' ~ target.schema) %}
  
  {# Create table #}
  {% set table_name = 'info_schema_test_table' %}
  {% set full_table = target.schema ~ '.' ~ table_name %}
  
  {% set create_sql %}
    CREATE OR REPLACE TABLE {{ full_table }} (
      id NUMBER,
      name VARCHAR(100),
      created_at TIMESTAMP
    )
  {% endset %}
  {% do adapter.execute(create_sql) %}
  
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=target.schema, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
  
  {# Insert some test data #}
  {% set insert_sql %}
    INSERT INTO {{ full_table }} VALUES
    (1, 'test', CURRENT_TIMESTAMP())
  {% endset %}
  {% do adapter.execute(insert_sql) %}
  
  {{ log("Created Snowflake test table: " ~ full_table, info=true) }}
{% endmacro %}

{% macro databricks__setup_test_table() %}
  {# Databricks implementation will be added in a separate PR #}
  {{ log("Databricks information_schema setup not yet implemented", info=true) }}
{% endmacro %}
