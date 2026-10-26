{% macro set_up(schema_name) %}
  {% do adapter.dispatch("set_up")(schema_name) %}
{% endmacro %}

{% macro default__set_up(schema_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro bigquery__set_up(schema_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}

  -- https://cloud.google.com/bigquery/docs/creating-partitioned-tables
  -- https://cloud.google.com/bigquery/docs/creating-clustered-tables
  {% do adapter.execute('DROP TABLE IF EXISTS ' ~ schema_name ~ '.base_a') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE '
    ~ schema_name ~ '.base_a (user_id INT64)
    PARTITION BY RANGE_BUCKET(user_id, GENERATE_ARRAY(0, 100, 10))
    CLUSTER BY user_id') %}
  {% do adapter.execute('INSERT INTO ' ~ schema_name ~ '.base_a SELECT user_id FROM UNNEST(GENERATE_ARRAY(0, 99)) as user_id') %}

  {% do adapter.execute('CREATE OR REPLACE MATERIALIZED VIEW '
    ~ schema_name ~ '.a
    PARTITION BY RANGE_BUCKET(user_id, GENERATE_ARRAY(0, 100, 10))
    CLUSTER BY user_id
    OPTIONS(
      enable_refresh=true,
      refresh_interval_minutes=30
    )
    AS SELECT user_id FROM ' ~ schema_name ~ '.base_a') %}

  {% do adapter.execute('DROP TABLE IF EXISTS ' ~ schema_name ~ '.base_b') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE '
    ~ schema_name ~ '.base_b (event_date Date, user_id INT64)
    PARTITION BY event_date
    CLUSTER BY user_id') %}
  {% do adapter.execute('INSERT INTO ' ~ schema_name ~ '.base_b SELECT CURRENT_DATE() as event_date, user_id FROM UNNEST(GENERATE_ARRAY(0, 99)) as user_id') %}

  {% do adapter.execute('CREATE OR REPLACE MATERIALIZED VIEW '
    ~ schema_name ~ '.b
    PARTITION BY event_date
    CLUSTER BY user_id
    OPTIONS(
      enable_refresh=true,
      refresh_interval_minutes=30
    )
    AS SELECT event_date, user_id FROM ' ~ schema_name ~ '.base_b') %}
{% endmacro %}