{# A source without change tracking: column discovery reads its CHANGES before the dynamic table
   (whose creation would enable tracking) exists. #}
{% macro set_up() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ target.database ~ '.' ~ target.schema) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ orders ~ ' (order_id int, amount number(10,2))') %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (1, 10.00)') %}
{% endmacro %}
