{% macro _fq() -%}
  {{ target.database }}.{{ target.schema }}
{%- endmacro %}

{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% for name in ['orders_merge', 'stock_merge', 'orders_transient', 'orders_with_region', 'orders_attrs', 'orders_switch', 'orders_empty'] %}
    {% do adapter.execute('DROP DYNAMIC TABLE IF EXISTS ' ~ _fq() ~ '.' ~ name) %}
  {% endfor %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.orders (order_id int, customer_id int, amount number(10,2)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.orders VALUES (1, 10, 10.00), (2, 20, 20.00), (3, 30, 30.00)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.stock (warehouse_id int, sku varchar(16), quantity int) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".stock VALUES (1, 'A', 100), (1, 'B', 50), (2, 'A', 10)") %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.customers (customer_id int, region varchar(16))') %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".customers VALUES (10, 'east'), (20, 'west'), (30, 'east'), (40, 'north')") %}
  {% do adapter.execute('CREATE OR REPLACE TAG ' ~ _fq() ~ '.mytag') %}
  {% do adapter.execute('CREATE OR REPLACE ROW ACCESS POLICY ' ~ _fq() ~ '.rap AS (order_id number) RETURNS BOOLEAN -> true') %}
{% endmacro %}

{# Round 1: an update, a delete and an insert per source, and a grant dbt doesn't manage. #}
{% macro apply_changes() %}
  {% do adapter.execute('UPDATE ' ~ _fq() ~ '.orders SET amount = 15.00 WHERE order_id = 1') %}
  {% do adapter.execute('DELETE FROM ' ~ _fq() ~ '.orders WHERE order_id = 2') %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.orders VALUES (4, 40, 40.00)') %}
  {% do adapter.execute("UPDATE " ~ _fq() ~ ".stock SET quantity = 80 WHERE warehouse_id = 1 AND sku = 'A'") %}
  {% do adapter.execute("DELETE FROM " ~ _fq() ~ ".stock WHERE warehouse_id = 1 AND sku = 'B'") %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".stock VALUES (2, 'C', 5)") %}
  {% do adapter.execute('GRANT SELECT ON DYNAMIC TABLE ' ~ _fq() ~ '.orders_attrs TO ROLE PUBLIC') %}
{% endmacro %}

{# Round 2: re-insert the order deleted in round 1, update the one it inserted. #}
{% macro apply_more_changes() %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.orders VALUES (2, 20, 25.00)') %}
  {% do adapter.execute('UPDATE ' ~ _fq() ~ '.orders SET amount = 45.00 WHERE order_id = 4') %}
{% endmacro %}

{#- Each expectation is printed as `[PASS]`/`[FAIL]`; the stdout goldie pins the outcome. -#}
{% macro _check(label, ok) %}
  {% do log(("[PASS] " if ok else "[FAIL] ") ~ label, info=True) %}
{% endmacro %}

{% macro _scalar(sql) %}
  {% set t = run_query(sql) %}
  {% if t.rows | length == 0 %}{% do return(none) %}{% endif %}
  {% do return(t.rows[0][0]) %}
{% endmacro %}

{# Whether `actual` and `expected` (two queries) hold the same rows. #}
{% macro _same_rows(actual, expected) %}
  {% do return(_scalar(
      "select count(*) from ((" ~ actual ~ " minus " ~ expected ~ ") union all (" ~ expected ~ " minus " ~ actual ~ "))"
  ) | int == 0) %}
{% endmacro %}

{% macro _show_dt(name) %}
  {% do run_query("show dynamic tables like '" ~ name | upper ~ "' in schema " ~ _fq()) %}
  {% do return(run_query('select "refresh_mode", "cluster_by", "initialization_warehouse", "target_lag" from table(result_scan(last_query_id()))').rows[0]) %}
{% endmacro %}

{#- `stage`: after_round_1 (round 1 refreshed, orders_switch a regular DT, orders_attrs altered),
    after_round_2 (round 2 refreshed, orders_switch a CI DT again), after_full_refresh. -#}
{% macro verify(stage) %}
  {% set fq = _fq() %}
  {% set orders = "select order_id, customer_id, amount, false from " ~ fq ~ ".orders" %}
  {{ _check(stage ~ ": orders_merge matches the source", _same_rows(
      "select order_id, customer_id, amount, is_deleted from " ~ fq ~ ".orders_merge", orders)) }}
  {{ _check(stage ~ ": stock_merge matches the source", _same_rows(
      "select warehouse_id, sku, quantity, is_deleted from " ~ fq ~ ".stock_merge",
      "select warehouse_id, sku, quantity, false from " ~ fq ~ ".stock")) }}
  {{ _check(stage ~ ": orders_with_region matches the source joined to the ephemeral model", _same_rows(
      "select order_id, amount, region, is_deleted from " ~ fq ~ ".orders_with_region",
      "select o.order_id, o.amount, c.region, false from " ~ fq ~ ".orders o join " ~ fq ~ ".customers c on c.customer_id = o.customer_id")) }}

  {#- APPEND_ONLY sees inserts only. Creating it read the source's current rows as inserts. -#}
  {% set transient_expected = {
      'after_round_1': "(1, 10.00), (2, 20.00), (3, 30.00), (4, 40.00)",
      'after_round_2': "(1, 10.00), (2, 25.00), (3, 30.00), (4, 40.00)",
      'after_full_refresh': "(1, 15.00), (2, 25.00), (3, 30.00), (4, 45.00)",
  }[stage] %}
  {{ _check(stage ~ ": orders_transient holds the inserts it saw", _same_rows(
      "select order_id, amount from " ~ fq ~ ".orders_transient",
      "select column1, column2 from values " ~ transient_expected)) }}

  {% set switch = _show_dt('orders_switch') %}
  {% if stage == 'after_round_1' %}
    {{ _check(stage ~ ": orders_switch is a regular dynamic table", switch[0] != 'CUSTOM_INCREMENTAL') }}
  {% else %}
    {{ _check(stage ~ ": orders_switch is a CI DT again", switch[0] == 'CUSTOM_INCREMENTAL') }}
  {% endif %}
  {{ _check(stage ~ ": orders_switch matches the source", _same_rows(
      "select order_id, amount from " ~ fq ~ ".orders_switch", "select order_id, amount from " ~ fq ~ ".orders")) }}

  {% if stage == 'after_full_refresh' %}
    {{ _check(stage ~ ": orders_empty holds the source after --full-refresh", _same_rows(
        "select order_id, amount from " ~ fq ~ ".orders_empty", "select order_id, amount from " ~ fq ~ ".orders")) }}
  {% else %}
    {{ _check(stage ~ ": orders_empty, built by --empty, stays empty across refreshes",
        _scalar("select count(*) from " ~ fq ~ ".orders_empty") | int == 0) }}
  {% endif %}

  {% set attrs = _show_dt('orders_attrs') %}
  {% set table = fq ~ '.orders_attrs' %}
  {% do run_query("show grants on dynamic table " ~ table) %}
  {% set public_select = _scalar('select count(*) from table(result_scan(last_query_id())) where "privilege" = \'SELECT\' and "grantee_name" = \'PUBLIC\'') %}
  {% set policies = run_query("select policy_name from table(" ~ target.database ~ ".information_schema.policy_references(ref_entity_name => '" ~ table ~ "', ref_entity_domain => 'table'))").rows | map(attribute=0) | list %}
  {{ _check(stage ~ ": orders_attrs cluster by amount (ALTER)", (attrs[1] | lower) == 'linear(amount)') }}
  {{ _check(stage ~ ": orders_attrs target_lag 2 hours (ALTER)", attrs[3] == '2 hours') }}
  {{ _check(stage ~ ": orders_attrs initialization warehouse", attrs[2] == 'FUSION_ADAPTER_TESTING') }}
  {{ _check(stage ~ ": orders_attrs tag mytag = v1", _scalar("select system$get_tag('" ~ fq ~ ".mytag', '" ~ table ~ "', 'table')") == 'v1') }}
  {{ _check(stage ~ ": orders_attrs row access policy", 'RAP' in policies) }}
  {{ _check(stage ~ ": orders_attrs comment from persist_docs", _scalar(
      "select comment from " ~ target.database ~ ".information_schema.tables where table_schema = upper('" ~ target.schema ~ "') and table_name = 'ORDERS_ATTRS'") == 'orders merged from their changes') }}
  {{ _check(stage ~ ": orders_attrs keeps the out-of-band grant", public_select | int == 1) }}
{% endmacro %}
