{# DIVERGENCE: fs-only. Decides how to rebuild an existing materialized view: 'replace' (CREATE OR REPLACE),
   'drop' (drop and create; only when the re-created view has no Compatibility Mode location), or 'refuse'. #}

{# Check ".agents/plans/completed/2026-09-28-databricks-mv-full-refresh-replace.md" for more details / cases #}
{#- Returns the nested field when the component is a mapping and the field has the expected shape, else none. -#}
{% macro _mv_strategy_field(config, component, field, kind) %}
  {%- set c = config[component] if (config is defined and config is not none) else none -%}
  {%- set v = c[field] if (c is mapping and field in c) else none -%}
  {%- if kind == 'properties' and v is mapping -%}
    {%- set ns = namespace(valid=true) -%}
    {%- for key, value in v.items() if value is not string -%}
      {%- set ns.valid = false -%}
    {%- endfor -%}
    {{ return(v if ns.valid else none) }}
  {%- elif kind == 'columns' and v is sequence and v is not string and v is not mapping -%}
    {{ return(v if (v | reject('string') | list | length) == 0 else none) }}
  {%- endif -%}
  {{ return(none) }}
{% endmacro %}

{% macro databricks__get_mv_replace_strategy(desired, existing) %}
  {%- set location_key = 'delta.universalFormat.compatibility.location' -%}
  {%- set fields = {
        'desired partitioned_by': _mv_strategy_field(desired, 'partitioned_by', 'partition_by', 'columns'),
        'existing partitioned_by': _mv_strategy_field(existing, 'partitioned_by', 'partition_by', 'columns'),
        'desired tblproperties': _mv_strategy_field(desired, 'tblproperties', 'tblproperties', 'properties'),
        'existing tblproperties': _mv_strategy_field(existing, 'tblproperties', 'tblproperties', 'properties'),
     } -%}
  {%- set ns = namespace(unreadable=none) -%}
  {%- for what, value in fields | dictsort if value is none -%}
    {%- if ns.unreadable is none -%}
      {%- set ns.unreadable = what -%}
    {%- endif -%}
  {%- endfor -%}
  {%- if ns.unreadable is not none -%}
    {{ return({'strategy': 'refuse', 'reason': "could not read the " ~ ns.unreadable ~ " configuration, so dbt cannot tell whether dropping the view would lose it. No materialized-view replacement or drop was attempted. Please report this as a dbt bug."}) }}
  {%- endif -%}

  {%- set desired_props = fields['desired tblproperties'] -%}
  {%- set existing_props = fields['existing tblproperties'] -%}
  {%- set desired_location = desired_props[location_key] if location_key in desired_props else none -%}
  {%- set existing_location = existing_props[location_key] if location_key in existing_props else none -%}
  {%- set empty_side = 'desired' if (desired_location is not none and desired_location | trim == '')
                       else 'existing' if (existing_location is not none and existing_location | trim == '') else none -%}
  {%- if empty_side is not none -%}
    {{ return({'strategy': 'refuse', 'reason': "the " ~ empty_side ~ " '" ~ location_key ~ "' is set but empty. Set it to a Unity Catalog location, or remove the property to disable Compatibility Mode. No materialized-view replacement or drop was attempted."}) }}
  {%- endif -%}
  {%- set desired_parts = fields['desired partitioned_by'] | map('lower') | map('replace', '`', '') | list -%}
  {%- set existing_parts = fields['existing partitioned_by'] | map('lower') | map('replace', '`', '') | list -%}

  {%- if desired_location and existing_location and desired_location != existing_location -%}
    {{ return({'strategy': 'refuse', 'reason': "changing the Compatibility Mode location from '" ~ existing_location ~ "' to '" ~ desired_location ~ "' can't be done in place (CREATE OR REPLACE fails with DELTA_UNIFORM_COMPATIBILITY_LOCATION_CANNOT_BE_CHANGED), and dbt won't drop the view to move it: the re-create could fail with LOCATION_OVERLAP and leave no view. Drop the view manually and rebuild it at a location that doesn't overlap '" ~ existing_location ~ "', or after its 7-day reservation expires."}) }}
  {%- elif desired_parts == existing_parts -%}
    {{ return({'strategy': 'replace'}) }}
  {%- elif not desired_parts -%}
    {%- if not desired_location -%}
      {{ return({'strategy': 'drop'}) }}
    {%- endif -%}
    {{ return({'strategy': 'refuse', 'reason': "removing partition_by needs a drop (CREATE OR REPLACE keeps the existing partitioning), and dbt won't drop a view whose re-create has a Compatibility Mode location ('" ~ desired_location ~ "'): the create could fail with LOCATION_OVERLAP and leave no view. Remove the Compatibility Mode properties and partition_by (dbt run --full-refresh), then add the properties back with a location that doesn't overlap '" ~ desired_location ~ "': the drop reserves it for 7 days. Or drop the view manually and rebuild."}) }}
  {%- elif desired_location and existing_location -%}
    {{ return({'strategy': 'refuse', 'reason': "changing partition_by on a Compatibility Mode materialized view fails in place ('Delta does not support partition evolution'), and dbt won't drop a view whose re-create has a Compatibility Mode location ('" ~ desired_location ~ "'): the create could fail with LOCATION_OVERLAP and leave no view. Remove the Compatibility Mode properties and change partition_by (dbt run --full-refresh), then add the properties back (dbt run); the same location works."}) }}
  {%- else -%}
    {{ return({'strategy': 'replace'}) }}
  {%- endif -%}
{% endmacro %}
