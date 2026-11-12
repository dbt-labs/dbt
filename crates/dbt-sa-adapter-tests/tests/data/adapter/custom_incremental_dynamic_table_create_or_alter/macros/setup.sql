{% macro set_up() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ target.database ~ '.' ~ target.schema) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ orders ~ ' (order_id int, amount number(10,2)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (1, 10.00), (2, 20.00), (3, 30.00)') %}
{% endmacro %}

{# An update, a delete and an insert, for the next refresh to pick up. #}
{% macro apply_changes() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% do adapter.execute('UPDATE ' ~ orders ~ ' SET amount = 15.00 WHERE order_id = 1') %}
  {% do adapter.execute('DELETE FROM ' ~ orders ~ ' WHERE order_id = 2') %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (4, 40.00)') %}
{% endmacro %}

{# A change after the body edit: only rows changed from here on see the new body. #}
{% macro apply_more_changes() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (5, 50.00)') %}
{% endmacro %}
