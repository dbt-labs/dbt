{% macro _fq() -%}
  {{ target.database }}.{{ target.schema }}
{%- endmacro %}

{% macro _models() %}
  {% do return(['body_edit', 'attrs_kept', 'err_reorder', 'err_type_narrow', 'err_type_change', 'ok_widen', 'ok_drop']) %}
{% endmacro %}

{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% for name in _models() %}
    {% do adapter.execute('DROP DYNAMIC TABLE IF EXISTS ' ~ _fq() ~ '.' ~ name) %}
  {% endfor %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.orders (order_id int, amount number(10,2), note varchar(10)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".orders VALUES (1, 10.00, 'a'), (2, 20.00, 'b')") %}
  {% do adapter.execute('CREATE OR REPLACE TAG ' ~ _fq() ~ '.mytag') %}
  {% do adapter.execute('CREATE OR REPLACE ROW ACCESS POLICY ' ~ _fq() ~ '.rap AS (order_id number) RETURNS BOOLEAN -> true') %}
{% endmacro %}

{#- Record each table's created_on (to tell an in-place sync from a rebuild), and set a tag, a
    policy and a grant on attrs_kept out of band. -#}
{% macro prepare() %}
  {% do adapter.execute("SHOW DYNAMIC TABLES IN SCHEMA " ~ _fq()) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.created_on AS SELECT lower("name") AS name, "created_on" AS created_on FROM TABLE(RESULT_SCAN(LAST_QUERY_ID()))') %}
  {% set attrs = _fq() ~ '.attrs_kept' %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ attrs ~ " SET TAG " ~ _fq() ~ ".mytag = 'v1'") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ attrs ~ " ADD ROW ACCESS POLICY " ~ _fq() ~ ".rap ON (order_id)") %}
  {% do adapter.execute("GRANT SELECT ON DYNAMIC TABLE " ~ attrs ~ " TO ROLE PUBLIC") %}
{% endmacro %}

{# Changes after the evolved body is deployed: an update of order 1 and a new order 3. #}
{% macro apply_changes() %}
  {% do adapter.execute('UPDATE ' ~ _fq() ~ '.orders SET amount = 15.00 WHERE order_id = 1') %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".orders VALUES (3, 30.00, 'c')") %}
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

{% macro _columns(name) %}
  {% set t = run_query(
      "select column_name || ' ' || data_type || coalesce('(' || character_maximum_length || ')', '') from "
      ~ target.database ~ ".information_schema.columns where table_schema = upper('" ~ target.schema
      ~ "') and table_name = upper('" ~ name ~ "') order by ordinal_position"
  ) %}
  {% do return(t.rows | map(attribute=0) | join(', ')) %}
{% endmacro %}

{# Whether `name` kept the created_on `prepare` recorded, i.e. was synced in place, not rebuilt. #}
{% macro _not_rebuilt(name) %}
  {% do run_query("show dynamic tables like '" ~ name | upper ~ "' in schema " ~ _fq()) %}
  {% set now = _scalar('select "created_on" from table(result_scan(last_query_id()))') %}
  {% set before = _scalar("select created_on from " ~ _fq() ~ ".created_on where name = '" ~ name ~ "'") %}
  {% do return(now == before) %}
{% endmacro %}

{% macro verify() %}
  {% set fq = _fq() %}
  {{ _check("body_edit: synced in place", _not_rebuilt('body_edit')) }}
  {{ _check("body_edit: only rows changed after the edit have amount_cents", _same_rows(
      "select order_id, amount_cents from " ~ fq ~ ".body_edit",
      "select column1, column2 from values (1, 1500.00), (2, null), (3, 3000.00)")) }}

  {% set attrs = fq ~ '.attrs_kept' %}
  {% do run_query("show dynamic tables like 'ATTRS_KEPT' in schema " ~ fq) %}
  {{ _check("attrs_kept: cluster by amount", (_scalar('select "cluster_by" from table(result_scan(last_query_id()))') | lower) == 'linear(amount)') }}
  {{ _check("attrs_kept: synced in place", _not_rebuilt('attrs_kept')) }}
  {{ _check("attrs_kept: keeps the tag", _scalar("select system$get_tag('" ~ fq ~ ".mytag', '" ~ attrs ~ "', 'table')") == 'v1') }}
  {{ _check("attrs_kept: keeps the row access policy", 'RAP' in (run_query("select policy_name from table(" ~ target.database ~ ".information_schema.policy_references(ref_entity_name => '" ~ attrs ~ "', ref_entity_domain => 'table'))").rows | map(attribute=0) | list)) }}
  {% do run_query("show grants on dynamic table " ~ attrs) %}
  {{ _check("attrs_kept: keeps the grant", _scalar('select count(*) from table(result_scan(last_query_id())) where "privilege" = \'SELECT\' and "grantee_name" = \'PUBLIC\'') | int == 1) }}

  {{ _check("ok_widen: note widened in place", _not_rebuilt('ok_widen') and _columns('ok_widen') == 'ORDER_ID NUMBER, AMOUNT NUMBER, NOTE TEXT(50)') }}
  {{ _check("ok_drop: note dropped in place", _not_rebuilt('ok_drop') and _columns('ok_drop') == 'ORDER_ID NUMBER, AMOUNT NUMBER') }}
  {{ _check("err_reorder: --full-refresh applied the new order", _columns('err_reorder') == 'ORDER_ID NUMBER, NOTE TEXT(10), AMOUNT NUMBER') }}
  {{ _check("err_type_narrow: --full-refresh applied the narrower type", _columns('err_type_narrow') == 'ORDER_ID NUMBER, AMOUNT NUMBER, NOTE TEXT(5)') }}
  {{ _check("err_type_change: --full-refresh applied the new type", _columns('err_type_change') == 'ORDER_ID NUMBER, AMOUNT TEXT(20), NOTE TEXT(10)') }}
{% endmacro %}
