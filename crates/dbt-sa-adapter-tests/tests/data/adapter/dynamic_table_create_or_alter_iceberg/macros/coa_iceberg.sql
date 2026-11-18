{#-
    Helpers for the managed dynamic Iceberg `snowflake_dynamic_table_create_or_alter` record/replay
    test. `verify` prints one `[PASS]`/`[FAIL]` line per expectation: a failing `run-operation` is
    not a test failure by itself, so the stdout goldie pins the outcome.
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

{# Row layout: created_on, text, refresh_mode, target_lag, is_iceberg #}
{% macro _show_dt(name) %}
  {% do run_query("show dynamic tables like '" ~ name ~ "' in schema " ~ _fq()) %}
  {% set t = run_query(
      'select "created_on"::varchar, "text", "refresh_mode", "target_lag", "is_iceberg" '
      ~ 'from table(result_scan(last_query_id())) where upper("name") = upper(\'' ~ name ~ '\')'
  ) %}
  {% if t.rows | length == 0 %}{% do return(none) %}{% endif %}
  {% do return(t.rows[0]) %}
{% endmacro %}

{% macro _base_location(name) %}
  {% do run_query("show iceberg tables like '" ~ name ~ "' in schema " ~ _fq()) %}
  {% do return(_scalar('select "base_location" from table(result_scan(last_query_id())) where upper("name") = upper(\'' ~ name ~ '\')')) %}
{% endmacro %}

{% macro _names() %}
  {% do return(["ice_evolve", "ice_idempotent", "ice_refresh_mode_replace", "ice_to_native", "native_to_ice", "ice_identity_change"]) %}
{% endmacro %}

{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% do adapter.execute("CREATE OR REPLACE TABLE " ~ _fq() ~ ".source_table AS "
      ~ "SELECT 1 AS id, 'alice' AS value UNION ALL SELECT 2, 'bob' UNION ALL SELECT 3, 'carol'") %}
  {% do adapter.execute("CREATE OR REPLACE TABLE " ~ _fq() ~ ".coa_marks (name VARCHAR, created_on VARCHAR)") %}
{% endmacro %}

{#- Record created_on (to tell an in-place sync from a rebuild) and check the starting formats. -#}
{% macro prepare() %}
  {% for name in _names() %}
    {% set row = _show_dt(name) %}
    {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".coa_marks VALUES ('" ~ name ~ "', '" ~ row[0] ~ "')") %}
  {% endfor %}
  {{ _check("setup: ice_evolve starts as Iceberg", _show_dt('ice_evolve')[4] | string | lower == 'true') }}
  {{ _check("setup: ice_to_native starts as Iceberg", _show_dt('ice_to_native')[4] | string | lower == 'true') }}
  {{ _check("setup: native_to_ice starts native", _show_dt('native_to_ice')[4] | string | lower == 'false') }}
  {{ _check("setup: ice_evolve starts with a 1 hour lag", _show_dt('ice_evolve')[3] == '1 hour') }}
{% endmacro %}

{% macro _created_on_unchanged(name, row) %}
  {% do return(_scalar("select count(*) from " ~ _fq() ~ ".coa_marks where name = '" ~ name ~ "' and created_on = '" ~ row[0] ~ "'") | int == 1) %}
{% endmacro %}

{% macro _evolved(row) %}
  {% do return('id_x10' in (row[1] | lower)) %}
{% endmacro %}

{% macro verify() %}
  {# query + target lag change: synced in place on a managed Iceberg table #}
  {% set row = _show_dt('ice_evolve') %}
  {{ _check("ice_evolve: still Iceberg", row[4] | string | lower == 'true') }}
  {{ _check("ice_evolve: definition deployed", _evolved(row)) }}
  {{ _check("ice_evolve: target lag applied", row[3] == '2 hours') }}
  {{ _check("ice_evolve: synced in place (created_on unchanged)", _created_on_unchanged('ice_evolve', row)) }}

  {% set row = _show_dt('ice_idempotent') %}
  {{ _check("ice_idempotent: not rebuilt (created_on unchanged)", _created_on_unchanged('ice_idempotent', row)) }}

  {# refresh_mode change: dbt rebuilds with CREATE OR REPLACE #}
  {% set row = _show_dt('ice_refresh_mode_replace') %}
  {{ _check("ice_refresh_mode_replace: now INCREMENTAL", row[2] | upper == 'INCREMENTAL') }}
  {{ _check("ice_refresh_mode_replace: rebuilt (created_on changed)", not _created_on_unchanged('ice_refresh_mode_replace', row)) }}
  {{ _check("ice_refresh_mode_replace: still Iceberg", row[4] | string | lower == 'true') }}

  {# format conversions: CREATE OR ALTER cannot convert, so dbt rebuilds #}
  {% set row = _show_dt('ice_to_native') %}
  {{ _check("ice_to_native: now native", row[4] | string | lower == 'false') }}
  {{ _check("ice_to_native: rebuilt (created_on changed)", not _created_on_unchanged('ice_to_native', row)) }}
  {{ _check("ice_to_native: definition deployed", _evolved(row)) }}

  {% set row = _show_dt('native_to_ice') %}
  {{ _check("native_to_ice: now Iceberg", row[4] | string | lower == 'true') }}
  {{ _check("native_to_ice: rebuilt (created_on changed)", not _created_on_unchanged('native_to_ice', row)) }}
  {{ _check("native_to_ice: definition deployed", _evolved(row)) }}

  {# identity change: rejected by CREATE OR ALTER, recovered by --full-refresh #}
  {% set row = _show_dt('ice_identity_change') %}
  {{ _check("ice_identity_change: recovered with the new base location", 'coa_ice_v2' in (_base_location('ice_identity_change') | string)) }}
  {{ _check("ice_identity_change: recovered with the evolved definition", _evolved(row)) }}
  {{ _check("ice_identity_change: rebuilt (created_on changed)", not _created_on_unchanged('ice_identity_change', row)) }}
{% endmacro %}
