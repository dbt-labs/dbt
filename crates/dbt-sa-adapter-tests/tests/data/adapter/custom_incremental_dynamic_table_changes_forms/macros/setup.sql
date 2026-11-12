{% macro set_up() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% set customers = target.database ~ '.' ~ target.schema ~ '.customers' %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ target.database ~ '.' ~ target.schema) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ orders ~ ' (order_id int, customer_id int, amount number(10,2)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (1, 10, 10.00), (2, 20, 20.00)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ customers ~ ' (customer_id int, region varchar(8)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute("INSERT INTO " ~ customers ~ " VALUES (10, 'EU'), (20, 'US'), (30, 'APAC')") %}
{% endmacro %}

{% macro apply_changes() %}
  {% set orders = target.database ~ '.' ~ target.schema ~ '.orders' %}
  {% do adapter.execute('UPDATE ' ~ orders ~ ' SET amount = 15.00 WHERE order_id = 1') %}
  {% do adapter.execute('INSERT INTO ' ~ orders ~ ' VALUES (3, 30, 30.00)') %}
{% endmacro %}
