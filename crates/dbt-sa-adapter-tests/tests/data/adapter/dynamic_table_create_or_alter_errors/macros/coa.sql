{#-
    Helpers for the `snowflake_dynamic_table_create_or_alter` record/replay tests.
    Each expectation is printed as `[PASS]`/`[FAIL]` because a failing `run-operation` is not a test
    failure by itself (the harness compares output); the stdout goldie pins the outcome.
-#}

{% macro _fq() -%}
  {{ target.database }}.{{ target.schema }}
{%- endmacro %}

{% macro _check(label, ok) %}
  {% do log(("[PASS] " if ok else "[FAIL] ") ~ label, info=True) %}
{% endmacro %}

{% macro _scalar(sql) %}
  {% set t = run_query(sql) %}
  {% if t.rows | length == 0 %}{% do return(none) %}{% endif %}
  {% do return(t.rows[0][0]) %}
{% endmacro %}

{# Column order of the row returned by _show_dt. #}
{% macro _show_dt(name) %}
  {% do run_query("show dynamic tables like '" ~ name ~ "' in schema " ~ _fq()) %}
  {% set t = run_query(
      'select "created_on"::varchar, "text", "refresh_mode", "cluster_by", "warehouse", "initialization_warehouse", "configured_refresh_mode" '
      ~ 'from table(result_scan(last_query_id())) where upper("name") = upper(\'' ~ name ~ '\')'
  ) %}
  {% if t.rows | length == 0 %}{% do return(none) %}{% endif %}
  {% do return(t.rows[0]) %}
{% endmacro %}

{% macro _is_transient(name) %}
  {% do run_query("show tables like '" ~ name ~ "' in schema " ~ _fq()) %}
  {% set kind = _scalar('select "kind" from table(result_scan(last_query_id())) where upper("name") = upper(\'' ~ name ~ '\')') %}
  {% do return(kind is not none and 'TRANSIENT' in (kind | string | upper)) %}
{% endmacro %}

{% macro _policies(name) %}
  {% set t = run_query(
      "select policy_name from table(" ~ target.database ~ ".information_schema.policy_references("
      ~ "ref_entity_name => '" ~ _fq() ~ "." ~ name ~ "', ref_entity_domain => 'table'))"
  ) %}
  {% do return(t.rows | map(attribute=0) | map('upper') | sort | list) %}
{% endmacro %}

{% macro _tags(name) %}
  {% set t = run_query(
      "select tag_name from table(" ~ target.database ~ ".information_schema.tag_references('"
      ~ _fq() ~ "." ~ name ~ "', 'table'))"
  ) %}
  {% do return(t.rows | map(attribute=0) | map('upper') | sort | list) %}
{% endmacro %}

{% macro _table_field(name, field) %}
  {% do return(_scalar(
      "select " ~ field ~ " from " ~ target.database ~ ".information_schema.tables "
      ~ "where table_schema ilike '" ~ target.schema ~ "' and table_name ilike '" ~ name ~ "'"
  )) %}
{% endmacro %}

{% macro _max_data_extension(name) %}
  {% do run_query("show parameters like 'MAX_DATA_EXTENSION_TIME_IN_DAYS' in table " ~ _fq() ~ "." ~ name) %}
  {% do return(_scalar('select "value" from table(result_scan(last_query_id()))')) %}
{% endmacro %}


{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% do adapter.execute("CREATE OR REPLACE TABLE " ~ _fq() ~ ".source_table AS "
      ~ "SELECT 1 AS id, 'alice' AS value UNION ALL SELECT 2, 'bob' UNION ALL SELECT 3, 'carol'") %}
{% endmacro %}

{% macro verify() %}
  {# recovery: --full-refresh rebuilds what CREATE OR ALTER could not express #}
  {% set row = _show_dt('err_reorder') %}
  {{ _check("err_reorder: recovered with the reordered definition", 'select value, id from' in (row[1] | lower)) }}

  {% set row = _show_dt('err_transient_drift') %}
  {{ _check("err_transient_drift: recovered with the evolved definition", _evolved(row)) }}
  {{ _check("err_transient_drift: recovered as non-transient", not _is_transient('err_transient_drift')) }}

  {# fail-fast: the config change was refused, nothing was applied #}
  {% set row = _show_dt('err_fail_fast') %}
  {{ _check("err_fail_fast: config change not applied", ((row[3] or '') | trim) == '') }}

  {# the other model in the failing run was still deployed #}
  {% set row = _show_dt('ok_evolve') %}
  {{ _check("ok_evolve: definition deployed despite sibling failures", _evolved(row)) }}
{% endmacro %}

{% macro _evolved(row) %}
  {% do return('id_x10' in (row[1] | lower)) %}
{% endmacro %}
