{% macro set_up() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ target.database ~ '.' ~ target.schema) %}
  {% do adapter.execute('DROP DYNAMIC TABLE IF EXISTS ' ~ target.database ~ '.' ~ target.schema ~ '.orders_merge') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ orders ~ ' (order_id int, note varchar(8)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute("INSERT INTO " ~ orders ~ " VALUES (1, 'short')") %}
{% endmacro %}

{# Widen the source column past the type the CI DT declared, and insert a value only the wider
   type holds. #}
{% macro widen_note() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% do adapter.execute('ALTER TABLE ' ~ orders ~ ' ALTER COLUMN note SET DATA TYPE varchar(32)') %}
  {% do adapter.execute("INSERT INTO " ~ orders ~ " VALUES (2, 'a much longer note value')") %}
{% endmacro %}
