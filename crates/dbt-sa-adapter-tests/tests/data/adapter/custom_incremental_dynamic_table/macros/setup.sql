{% macro set_up() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% set stock = target.database ~ '.' ~ target.schema ~ '.stock' %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ target.database ~ '.' ~ target.schema) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ orders ~ ' (order_id int, customer_id int, amount number(10,2)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (1, 10, 10.00), (2, 20, 20.00), (3, 30, 30.00)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ stock ~ ' (warehouse_id int, sku varchar(16), quantity int) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute("INSERT INTO " ~ stock ~ " VALUES (1, 'A', 100), (1, 'B', 50), (2, 'A', 10)") %}
{% endmacro %}

{# An update, a delete and an insert on each source, for the next refresh to pick up. #}
{% macro apply_changes() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% set stock = target.database ~ '.' ~ target.schema ~ '.stock' %}
  {% do adapter.execute('UPDATE ' ~ orders ~ ' SET amount = 15.00 WHERE order_id = 1') %}
  {% do adapter.execute('DELETE FROM ' ~ orders ~ ' WHERE order_id = 2') %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (4, 40, 40.00)') %}
  {% do adapter.execute("UPDATE " ~ stock ~ " SET quantity = 80 WHERE warehouse_id = 1 AND sku = 'A'") %}
  {% do adapter.execute("DELETE FROM " ~ stock ~ " WHERE warehouse_id = 1 AND sku = 'B'") %}
  {% do adapter.execute("INSERT INTO " ~ stock ~ " VALUES (2, 'C', 5)") %}
{% endmacro %}
