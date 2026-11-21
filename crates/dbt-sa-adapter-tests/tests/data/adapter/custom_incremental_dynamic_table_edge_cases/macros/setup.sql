{% macro _fq() -%}
  {{ target.database }}.{{ target.schema }}
{%- endmacro %}

{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% for name in ['no_change_tracking', 'self_reference', 'duplicate_keys', 'widened_column', 'over_replaced_table'] %}
    {% do adapter.execute('DROP DYNAMIC TABLE IF EXISTS ' ~ _fq() ~ '.' ~ name) %}
  {% endfor %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.orders (order_id int, amount number(10,2)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.orders VALUES (1, 10.00), (2, 20.00)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.untracked (order_id int, amount number(10,2))') %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.untracked VALUES (1, 10.00)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.notes (order_id int, note varchar(8)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".notes VALUES (1, 'short')") %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.raw_orders (order_id int, amount number(10,2))') %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.raw_orders VALUES (1, 10.00), (2, 20.00)') %}
{% endmacro %}

{#- An update for duplicate_keys, a widened column and a longer value for widened_column, and an
    update, a delete and an insert on raw_orders, which the next run copies into stg_orders. -#}
{% macro apply_changes() %}
  {% do adapter.execute('UPDATE ' ~ _fq() ~ '.orders SET amount = 15.00 WHERE order_id = 1') %}
  {% do adapter.execute('ALTER TABLE ' ~ _fq() ~ '.notes ALTER COLUMN note SET DATA TYPE varchar(32)') %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".notes VALUES (2, 'a much longer note value')") %}
  {% do adapter.execute('UPDATE ' ~ _fq() ~ '.raw_orders SET amount = 15.00 WHERE order_id = 1') %}
  {% do adapter.execute('DELETE FROM ' ~ _fq() ~ '.raw_orders WHERE order_id = 2') %}
  {% do adapter.execute('INSERT INTO ' ~ _fq() ~ '.raw_orders VALUES (3, 30.00)') %}
{% endmacro %}

{#- Each expectation is printed as `[PASS]`/`[FAIL]`; the stdout goldie pins the outcome. -#}
{% macro _check(label, ok) %}
  {% do log(("[PASS] " if ok else "[FAIL] ") ~ label, info=True) %}
{% endmacro %}

{% macro _same_rows(actual, expected) %}
  {% set t = run_query(
      "select count(*) from ((" ~ actual ~ " minus " ~ expected ~ ") union all (" ~ expected ~ " minus " ~ actual ~ "))"
  ) %}
  {% do return(t.rows[0][0] | int == 0) %}
{% endmacro %}

{% macro verify() %}
  {% set fq = _fq() %}
  {{ _check("duplicate_keys: the failed refresh left its rows", _same_rows(
      "select order_id, amount from " ~ fq ~ ".duplicate_keys", "select column1, column2 from values (1, 10.00), (2, 20.00)")) }}
  {{ _check("widened_column: the failed refresh left its rows", _same_rows(
      "select order_id, note from " ~ fq ~ ".widened_column", "select column1, column2 from values (1, 'short')")) }}
  {{ _check("over_replaced_table: update and insert arrive, the deleted order 2 stays", _same_rows(
      "select order_id, amount from " ~ fq ~ ".over_replaced_table", "select column1, column2 from values (1, 15.00), (2, 20.00), (3, 30.00)")) }}
{% endmacro %}
