{#-
    Helpers for the `snowflake_dynamic_table_create_or_alter` record/replay test.

    The test runs these `run-operation` macros between `dbt run` turns:
      set_up   -- source table, marks table, governance objects
      prepare  -- record every dynamic table's `created_on`, then set attributes out of band
      verify   -- read the dynamic tables back and print one `[PASS]`/`[FAIL]` line per expectation

    A failing `run-operation` is not a test failure by itself (the harness only compares output),
    so each expectation is printed and the stdout goldie pins the outcome.
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

{% macro _dynamic_table_names() %}
  {% do return(["dm_full_disable", "dm_full_enable", "dm_incremental_disable", "dm_incremental_enable", "dm_auto_disable", "dm_auto_enable", "transient_flip", "transient_preserved", "refresh_mode_replace", "refresh_mode_to_auto", "idempotent_noop", "inc_noop", "inc_props", "cfg_cluster_by", "cfg_warehouse", "cfg_init_warehouse", "continue_skips_both", "attr_masking_policy", "attr_row_access_policy", "attr_tag", "attr_projection_policy", "attr_aggregation_policy", "attr_max_data_extension", "attr_data_retention", "attr_comment", "attr_multi"]) %}
{% endmacro %}

{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% do adapter.execute("CREATE OR REPLACE TABLE " ~ _fq() ~ ".source_table AS "
      ~ "SELECT 1 AS id, 'alice' AS value UNION ALL SELECT 2, 'bob' UNION ALL SELECT 3, 'carol'") %}
  {% do adapter.execute("CREATE OR REPLACE TABLE " ~ _fq() ~ ".coa_marks (name VARCHAR, created_on VARCHAR)") %}
  {% do adapter.execute("CREATE MASKING POLICY " ~ _fq() ~ ".mp AS (v VARCHAR) RETURNS VARCHAR -> '***'") %}
  {% do adapter.execute("CREATE ROW ACCESS POLICY " ~ _fq() ~ ".rap AS (i INT) RETURNS BOOLEAN -> TRUE") %}
  {% do adapter.execute("CREATE TAG " ~ _fq() ~ ".mytag") %}
  {% do adapter.execute("CREATE PROJECTION POLICY " ~ _fq() ~ ".pp AS () RETURNS PROJECTION_CONSTRAINT -> PROJECTION_CONSTRAINT(ALLOW => TRUE)") %}
  {% do adapter.execute("CREATE AGGREGATION POLICY " ~ _fq() ~ ".ap AS () RETURNS AGGREGATION_CONSTRAINT -> AGGREGATION_CONSTRAINT(MIN_GROUP_SIZE => 2)") %}
{% endmacro %}

{#- Record created_on (to tell an in-place sync from a rebuild) and set attributes that dbt has no
    config surface for. Each is checked right away so a bad setup can't masquerade as a result. -#}
{% macro prepare() %}
  {% for name in _dynamic_table_names() %}
    {% set row = _show_dt(name) %}
    {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".coa_marks VALUES ('" ~ name ~ "', '" ~ row[0] ~ "')") %}
  {% endfor %}

  {% set fq = _fq() %}
  {% do adapter.execute("ALTER TABLE " ~ fq ~ ".attr_masking_policy MODIFY COLUMN value SET MASKING POLICY " ~ fq ~ ".mp") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_row_access_policy ADD ROW ACCESS POLICY " ~ fq ~ ".rap ON (id)") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_tag SET TAG " ~ fq ~ ".mytag = 'v1'") %}
  {% do adapter.execute("ALTER TABLE " ~ fq ~ ".attr_projection_policy MODIFY COLUMN value SET PROJECTION POLICY " ~ fq ~ ".pp") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_aggregation_policy SET AGGREGATION POLICY " ~ fq ~ ".ap") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_max_data_extension SET MAX_DATA_EXTENSION_TIME_IN_DAYS = 5") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_data_retention SET DATA_RETENTION_TIME_IN_DAYS = 3") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_comment SET COMMENT = 'keepme'") %}
  {% do adapter.execute("ALTER TABLE " ~ fq ~ ".attr_multi MODIFY COLUMN value SET MASKING POLICY " ~ fq ~ ".mp") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_multi SET TAG " ~ fq ~ ".mytag = 'v1'") %}
  {% do adapter.execute("ALTER DYNAMIC TABLE " ~ fq ~ ".attr_multi SET DATA_RETENTION_TIME_IN_DAYS = 3") %}

  {{ _check("setup: attr_masking_policy has MP", 'MP' in _policies('attr_masking_policy')) }}
  {{ _check("setup: attr_row_access_policy has RAP", 'RAP' in _policies('attr_row_access_policy')) }}
  {{ _check("setup: attr_tag has MYTAG", 'MYTAG' in _tags('attr_tag')) }}
  {{ _check("setup: attr_projection_policy has PP", 'PP' in _policies('attr_projection_policy')) }}
  {{ _check("setup: attr_aggregation_policy has AP", 'AP' in _policies('attr_aggregation_policy')) }}
  {{ _check("setup: attr_max_data_extension is 5", _max_data_extension('attr_max_data_extension') | string == '5') }}
  {{ _check("setup: attr_data_retention is 3", _table_field('attr_data_retention', 'retention_time') | string == '3') }}
  {{ _check("setup: attr_comment is keepme", _table_field('attr_comment', 'comment') == 'keepme') }}
  {{ _check("setup: attr_multi has MP", 'MP' in _policies('attr_multi')) }}
  {{ _check("setup: attr_multi has MYTAG", 'MYTAG' in _tags('attr_multi')) }}
  {{ _check("setup: attr_multi retention is 3", _table_field('attr_multi', 'retention_time') | string == '3') }}
{% endmacro %}

{#- The refresh actions Snowflake recorded for a dynamic table, oldest first (INCREMENTAL / FULL /
    NO_DATA / REINITIALIZE). CREATE OR ALTER on an incremental table reinitializes it on a query
    change; a no-op or a property-only change must not. -#}
{% macro _refresh_actions(name) %}
  {% set t = run_query(
      "select refresh_action from table(" ~ target.database ~ ".information_schema.dynamic_table_refresh_history("
      ~ "name => '" ~ ((_fq() ~ '.' ~ name) | upper) ~ "')) order by data_timestamp, refresh_start_time"
  ) %}
  {% do return(t.rows | map(attribute=0) | list) %}
{% endmacro %}

{% macro _created_on_unchanged(name, row) %}
  {% do return(_scalar("select count(*) from " ~ _fq() ~ ".coa_marks where name = '" ~ name ~ "' and created_on = '" ~ row[0] ~ "'") | int == 1) %}
{% endmacro %}

{% macro _evolved(row) %}
  {% do return('id_x10' in (row[1] | lower)) %}
{% endmacro %}

{% macro verify() %}
  {# deploy matrix: a SQL-only edit is deployed in place for every refresh_mode x scheduler #}
  {% for name in ['dm_full_disable', 'dm_full_enable', 'dm_incremental_disable', 'dm_incremental_enable', 'dm_auto_disable', 'dm_auto_enable'] %}
    {% set row = _show_dt(name) %}
    {{ _check(name ~ ": definition deployed", _evolved(row)) }}
    {{ _check(name ~ ": synced in place (created_on unchanged)", _created_on_unchanged(name, row)) }}
  {% endfor %}

  {# reinitialization: a query change reinitializes an incremental table; a FULL table just recomputes #}
  {% set actions = _refresh_actions('dm_incremental_disable') %}
  {% do log("dm_incremental_disable: refresh actions " ~ actions, info=True) %}
  {{ _check("dm_incremental_disable: query change reinitialized the table", 'REINITIALIZE' in actions) }}
  {% set row = _show_dt('dm_auto_disable') %}
  {% set actions = _refresh_actions('dm_auto_disable') %}
  {% do log("dm_auto_disable: refresh_mode=" ~ row[2] ~ " refresh actions " ~ actions, info=True) %}
  {{ _check("dm_auto_disable: reinitialized exactly when AUTO resolved to incremental", ('REINITIALIZE' in actions) == ((row[2] | upper) in ['INCREMENTAL', 'ADAPTIVE'])) }}
  {% set actions = _refresh_actions('dm_full_disable') %}
  {% do log("dm_full_disable: refresh actions " ~ actions, info=True) %}
  {{ _check("dm_full_disable: FULL table is recomputed, not reinitialized", 'REINITIALIZE' not in actions and 'FULL' in actions) }}

  {# incremental, no reinitialization: a no-op and a property-only change #}
  {% set row = _show_dt('inc_noop') %}
  {% set actions = _refresh_actions('inc_noop') %}
  {% do log("inc_noop: refresh actions " ~ actions, info=True) %}
  {{ _check("inc_noop: not rebuilt (created_on unchanged)", _created_on_unchanged('inc_noop', row)) }}
  {{ _check("inc_noop: no reinitialization", 'REINITIALIZE' not in actions) }}

  {% set row = _show_dt('inc_props') %}
  {% set actions = _refresh_actions('inc_props') %}
  {% do log("inc_props: refresh actions " ~ actions, info=True) %}
  {{ _check("inc_props: cluster key applied", 'ID' in ((row[3] or '') | upper)) }}
  {{ _check("inc_props: warehouse applied", (row[4] | upper) == 'FUSION_SLT_WAREHOUSE') }}
  {{ _check("inc_props: synced in place (created_on unchanged)", _created_on_unchanged('inc_props', row)) }}
  {{ _check("inc_props: properties change did not reinitialize", 'REINITIALIZE' not in actions) }}

  {# routing: transient / refresh_mode changes are rebuilt, everything else is synced in place #}
  {% set row = _show_dt('transient_flip') %}
  {{ _check("transient_flip: now non-transient", not _is_transient('transient_flip')) }}
  {{ _check("transient_flip: rebuilt (created_on changed)", not _created_on_unchanged('transient_flip', row)) }}

  {% set row = _show_dt('transient_preserved') %}
  {{ _check("transient_preserved: definition deployed", _evolved(row)) }}
  {{ _check("transient_preserved: still transient", _is_transient('transient_preserved')) }}
  {{ _check("transient_preserved: synced in place (created_on unchanged)", _created_on_unchanged('transient_preserved', row)) }}

  {% set row = _show_dt('refresh_mode_replace') %}
  {{ _check("refresh_mode_replace: now FULL", 'FULL' in (row[2] | upper)) }}
  {{ _check("refresh_mode_replace: rebuilt (created_on changed)", not _created_on_unchanged('refresh_mode_replace', row)) }}

  {% set row = _show_dt('refresh_mode_to_auto') %}
  {% do log("refresh_mode_to_auto: refresh_mode=" ~ row[2] ~ " configured_refresh_mode=" ~ row[6], info=True) %}
  {{ _check("refresh_mode_to_auto: applied in place (created_on unchanged)", _created_on_unchanged('refresh_mode_to_auto', row)) }}
  {{ _check("refresh_mode_to_auto: no longer INCREMENTAL (AUTO re-resolved)", (row[2] | upper) != 'INCREMENTAL') }}

  {% set row = _show_dt('idempotent_noop') %}
  {{ _check("idempotent_noop: unchanged definition", not _evolved(row)) }}
  {{ _check("idempotent_noop: not rebuilt (created_on unchanged)", _created_on_unchanged('idempotent_noop', row)) }}

  {# config changes are applied in place #}
  {% set row = _show_dt('cfg_cluster_by') %}
  {{ _check("cfg_cluster_by: cluster key applied", 'ID' in ((row[3] or '') | upper)) }}
  {{ _check("cfg_cluster_by: synced in place (created_on unchanged)", _created_on_unchanged('cfg_cluster_by', row)) }}

  {% set row = _show_dt('cfg_warehouse') %}
  {{ _check("cfg_warehouse: warehouse applied", (row[4] | upper) == 'FUSION_SLT_WAREHOUSE') }}
  {{ _check("cfg_warehouse: synced in place (created_on unchanged)", _created_on_unchanged('cfg_warehouse', row)) }}

  {% set row = _show_dt('cfg_init_warehouse') %}
  {{ _check("cfg_init_warehouse: initialization warehouse applied", ((row[5] or '') | upper) == 'FUSION_SLT_WAREHOUSE') }}
  {{ _check("cfg_init_warehouse: synced in place (created_on unchanged)", _created_on_unchanged('cfg_init_warehouse', row)) }}

  {# on_configuration_change: continue skips a config change AND the coincident SQL edit #}
  {% set row = _show_dt('continue_skips_both') %}
  {{ _check("continue_skips_both: SQL edit skipped", not _evolved(row)) }}
  {{ _check("continue_skips_both: config change skipped", ((row[3] or '') | trim) == '') }}
  {{ _check("continue_skips_both: not rebuilt (created_on unchanged)", _created_on_unchanged('continue_skips_both', row)) }}

  {# attributes: governance is preserved, operational attributes reset to default #}
  {% for name in ['attr_masking_policy', 'attr_row_access_policy', 'attr_tag', 'attr_projection_policy', 'attr_aggregation_policy', 'attr_max_data_extension', 'attr_data_retention', 'attr_comment', 'attr_multi'] %}
    {{ _check(name ~ ": definition deployed", _evolved(_show_dt(name))) }}
  {% endfor %}
  {{ _check("attr_masking_policy: masking policy preserved", 'MP' in _policies('attr_masking_policy')) }}
  {{ _check("attr_row_access_policy: row access policy preserved", 'RAP' in _policies('attr_row_access_policy')) }}
  {{ _check("attr_tag: tag preserved", 'MYTAG' in _tags('attr_tag')) }}
  {{ _check("attr_projection_policy: projection policy preserved", 'PP' in _policies('attr_projection_policy')) }}
  {{ _check("attr_aggregation_policy: aggregation policy preserved", 'AP' in _policies('attr_aggregation_policy')) }}
  {{ _check("attr_max_data_extension: reset to default", _max_data_extension('attr_max_data_extension') | string != '5') }}
  {{ _check("attr_data_retention: reset to default", _table_field('attr_data_retention', 'retention_time') | string != '3') }}
  {{ _check("attr_comment: reset to default", _table_field('attr_comment', 'comment') != 'keepme') }}

  {# one COA, independent outcomes #}
  {% set row = _show_dt('attr_multi') %}
  {{ _check("attr_multi: masking policy preserved", 'MP' in _policies('attr_multi')) }}
  {{ _check("attr_multi: tag preserved", 'MYTAG' in _tags('attr_multi')) }}
  {{ _check("attr_multi: cluster key kept", 'ID' in ((row[3] or '') | upper)) }}
  {{ _check("attr_multi: retention reset to default", _table_field('attr_multi', 'retention_time') | string != '3') }}
{% endmacro %}
