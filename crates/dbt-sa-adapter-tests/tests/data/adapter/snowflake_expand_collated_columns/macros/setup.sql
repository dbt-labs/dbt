{% macro set_up(schema_name) %}
  {% do adapter.dispatch("set_up")(schema_name) %}
{% endmacro %}

{% macro default__set_up(schema_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro snowflake__set_up(schema_name) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}
  {# Source table: larger varchar with collation #}
  {% do adapter.execute("CREATE OR REPLACE TABLE a (id VARCHAR(20) COLLATE 'en-ci')") %}
  {# Target table: smaller varchar with same collation — should be expanded to match source #}
  {% do adapter.execute("CREATE OR REPLACE TABLE b (id VARCHAR(10) COLLATE 'en-ci')") %}
{% endmacro %}
